//! Background analysis queue for uploaded files.
//!
//! Uploading used to mean waiting for the analysis: the upload handler wrote the
//! bytes and then ran face detection inline, so a browser sending 500 photos held
//! a request open through 500 model runs and only called the import "done" at the
//! end of the last one. The bytes were safe on disk long before that.
//!
//! Here the handler hands the path to a per-library worker and answers straight
//! away. The worker runs the same [`Scanner::process_file`] the upload handler
//! used to call, one file at a time, and signals the organizer whenever a file
//! actually landed — so files still reach their person folders, just after the
//! upload instead of during it.
//!
//! Nothing is persisted: a queue that does not survive a restart is fine here
//! because the files themselves are, and the startup scan picks up anything that
//! was still waiting.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use serde::Serialize;
use tracing::{error, info};
use utoipa::ToSchema;

use crate::organizer::Organizer;
use crate::scanner::Scanner;

/// What the queue is doing for one library, for the import UI to poll.
///
/// [`completed`] and [`failed`] count since the server started, not since the
/// current upload: the client knows how many files it sent, and the pair of
/// numbers it cares about — how many are still coming and whether any broke —
/// are both derivable from these without the server tracking upload sessions.
#[derive(Serialize, ToSchema, Debug, Default, PartialEq, Eq)]
pub struct IngestStatus {
    /// Files waiting for the worker.
    pub queued: usize,
    /// Files being analyzed right now (0 or 1 — one worker per library).
    pub analyzing: usize,
    /// Files analyzed since startup.
    pub completed: u64,
    /// Files whose analysis failed since startup. They stay on disk.
    pub failed: u64,
}

struct Job {
    path: PathBuf,
    scanner: Arc<Scanner>,
}

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Job>,
    analyzing: usize,
    completed: u64,
    failed: u64,
    stopped: bool,
}

struct Worker {
    library_root: PathBuf,
    organizer: Arc<Organizer>,
    /// Shared with the file watcher; see [`IngestQueue::analysis_lock`].
    analysis: Arc<Mutex<()>>,
    state: Mutex<QueueState>,
    cvar: Condvar,
}

/// Registry of per-library ingest workers, shared through `AppState`.
///
/// Per library rather than one global queue so that one user uploading a
/// holiday's worth of photos does not stall another user's single drag-and-drop
/// behind it — the same reason the organizer keeps a worker per library.
pub struct IngestQueue {
    workers: Mutex<HashMap<PathBuf, Arc<Worker>>>,
    /// One analysis lock per library, held by whoever is running
    /// [`Scanner::process_file`] against it. See [`IngestQueue::analysis_lock`].
    analysis_locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl IngestQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            workers: Mutex::new(HashMap::new()),
            analysis_locks: Mutex::new(HashMap::new()),
        })
    }

    /// Queue one uploaded file for analysis. Returns immediately.
    pub fn enqueue(
        &self,
        library_root: &Path,
        organizer: Arc<Organizer>,
        scanner: Arc<Scanner>,
        path: PathBuf,
    ) {
        let worker = self.worker_for(library_root, organizer);
        let mut state = worker.state.lock().unwrap();
        state.jobs.push_back(Job { path, scanner });
        worker.cvar.notify_all();
    }

    /// What is outstanding for `library_root`. An unknown library is idle.
    pub fn status(&self, library_root: &Path) -> IngestStatus {
        let workers = self.workers.lock().unwrap();
        let Some(worker) = workers.get(library_root) else {
            return IngestStatus::default();
        };
        let state = worker.state.lock().unwrap();
        IngestStatus {
            queued: state.jobs.len(),
            analyzing: state.analyzing,
            completed: state.completed,
            failed: state.failed,
        }
    }

    /// Block until nothing is queued or in flight for `library_root`.
    ///
    /// For the callers that genuinely need the analysis to be over before they
    /// start — finalizing an import clusters faces, and clustering half an
    /// upload produces people the rest of the upload then has to be merged into.
    pub fn wait_until_idle(&self, library_root: &Path) {
        let worker = {
            let workers = self.workers.lock().unwrap();
            match workers.get(library_root) {
                Some(w) => w.clone(),
                None => return,
            }
        };
        let mut state = worker.state.lock().unwrap();
        while !state.stopped && !(state.jobs.is_empty() && state.analyzing == 0) {
            state = worker.cvar.wait(state).unwrap();
        }
    }

    /// The lock that serializes analysis of one library.
    ///
    /// The file watcher sees the same uploads this queue does — they land in a
    /// watched directory — and both would call [`Scanner::process_file`] on the
    /// same path at once. Its "already indexed?" check is a plain SELECT, so two
    /// threads could both pass it and the loser died on `UNIQUE constraint
    /// failed: files.path`. Whoever analyzes a file takes this first, and the
    /// second one then sees the row and skips the file instead of failing on it.
    pub fn analysis_lock(&self, library_root: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.analysis_locks.lock().unwrap();
        locks
            .entry(library_root.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Stop every worker. Whatever is still queued is dropped — the files stay
    /// on disk and the next startup scan finds them.
    pub fn shutdown(&self) {
        let workers = self.workers.lock().unwrap();
        for worker in workers.values() {
            worker.state.lock().unwrap().stopped = true;
            worker.cvar.notify_all();
        }
    }

    fn worker_for(&self, library_root: &Path, organizer: Arc<Organizer>) -> Arc<Worker> {
        // Taken before the workers map: `analysis_lock` locks its own map, and
        // always acquiring the two in this order keeps the pair deadlock-free.
        let analysis = self.analysis_lock(library_root);
        let mut workers = self.workers.lock().unwrap();
        if let Some(worker) = workers.get(library_root) {
            return worker.clone();
        }
        let worker = Arc::new(Worker {
            library_root: library_root.to_path_buf(),
            organizer,
            analysis,
            state: Mutex::new(QueueState::default()),
            cvar: Condvar::new(),
        });
        let thread_worker = worker.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("phos-ingest".into())
            .spawn(move || run_worker_loop(&thread_worker))
        {
            error!("Failed to spawn ingest worker: {}", e);
        }
        workers.insert(library_root.to_path_buf(), worker.clone());
        worker
    }
}

fn run_worker_loop(worker: &Worker) {
    info!("Ingest worker started for {:?}", worker.library_root);
    loop {
        let job = {
            let mut state = worker.state.lock().unwrap();
            loop {
                if state.stopped {
                    info!("Ingest worker for {:?} stopped", worker.library_root);
                    return;
                }
                if let Some(job) = state.jobs.pop_front() {
                    state.analyzing += 1;
                    break job;
                }
                state = worker.cvar.wait(state).unwrap();
            }
        };

        let analyzed = analyze(worker, &job);

        let drained = {
            let mut state = worker.state.lock().unwrap();
            state.analyzing -= 1;
            if analyzed {
                state.completed += 1;
            } else {
                state.failed += 1;
            }
            // Wakes both the next `wait_until_idle` and anything waiting to enqueue.
            worker.cvar.notify_all();
            state.jobs.is_empty()
        };

        // Once the burst is over, clean up the boxes it produced. Detection can
        // put two rectangles on one face, and finding them by hand in the review
        // queue is the job nobody wants; doing it per file instead would mean
        // re-running the sweep dozens of times during one upload.
        if drained {
            dedupe_faces(worker, &job);
        }
    }
}

/// Collapse overlapping duplicate face boxes across the library.
///
/// Best-effort: a failure here costs the user a manual delete during review, not
/// their photos, so it is logged and the worker carries on.
fn dedupe_faces(worker: &Worker, job: &Job) {
    // Same lock the analysis takes: this deletes face rows that a watcher-driven
    // `process_file` may be inserting right now.
    let _analysis = worker.analysis.lock().unwrap_or_else(|e| e.into_inner());
    let mut conn = match job.scanner.open_db() {
        Ok(c) => c,
        Err(e) => {
            error!("Ingest: failed to open DB for face dedupe: {}", e);
            return;
        }
    };
    match job.scanner.dedupe_overlapping_faces(&mut conn) {
        Ok(0) => {}
        Ok(n) => info!("Ingest: removed {} duplicate face box(es)", n),
        Err(e) => error!("Ingest: face dedupe failed: {}", e),
    }
}

/// Runs one file through the scanner. `false` means it failed and was counted.
fn analyze(worker: &Worker, job: &Job) -> bool {
    let mut conn = match job.scanner.open_db() {
        Ok(c) => c,
        Err(e) => {
            error!("Ingest: failed to open DB for {:?}: {}", job.path, e);
            return false;
        }
    };

    // A fresh cache per file, exactly as the upload handler used to pass: it is
    // what decides whether a file joins an existing shot, and widening that here
    // would quietly change how uploads group.
    let dhash_cache = Mutex::new(Vec::new());
    let _analysis = worker.analysis.lock().unwrap();
    match job.scanner.process_file(&mut conn, &job.path, &dhash_cache) {
        Ok(true) => {
            worker.organizer.signal(&worker.library_root);
            true
        }
        // Already known — not an error, and nothing to reorganize for.
        Ok(false) => true,
        Err(e) => {
            error!("Ingest: failed to analyze {:?}: {}", job.path, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::files;
    use diesel::prelude::*;

    /// A library with one uploaded file already on disk, and the pieces the
    /// queue needs to analyze it.
    fn library() -> (tempfile::TempDir, Arc<Scanner>, PathBuf) {
        let tmp = tempfile::tempdir().expect("temp dir");
        let db_path = tmp.path().join(".phos.db");
        crate::db::init_and_migrate(&db_path).expect("schema");
        let scanner = Arc::new(Scanner::new(db_path, None));
        let uploaded = tmp.path().join("uploaded.jpg");
        std::fs::write(&uploaded, b"not really a jpeg").expect("write");
        (tmp, scanner, uploaded)
    }

    /// The point of the queue: the upload path hands over a file and the
    /// analysis happens afterwards, on the worker.
    #[test]
    fn an_enqueued_file_is_analyzed_in_the_background() {
        let (tmp, scanner, uploaded) = library();
        let queue = IngestQueue::new();

        queue.enqueue(tmp.path(), Organizer::new(), scanner.clone(), uploaded);
        queue.wait_until_idle(tmp.path());

        let status = queue.status(tmp.path());
        assert_eq!(1, status.completed);
        assert_eq!(0, status.failed);
        assert_eq!(0, status.queued);
        assert_eq!(0, status.analyzing);

        let mut conn = scanner.open_db().expect("connection");
        let indexed: i64 = files::table.count().get_result(&mut conn).expect("count");
        assert_eq!(1, indexed, "the worker must have indexed the upload");
    }

    /// A library nobody has uploaded to reports idle rather than spawning a
    /// worker — the import UI polls this before it has sent anything.
    #[test]
    fn an_untouched_library_is_idle() {
        let tmp = tempfile::tempdir().expect("temp dir");

        let queue = IngestQueue::new();

        assert_eq!(IngestStatus::default(), queue.status(tmp.path()));
        // Must not block: there is nothing to wait for.
        queue.wait_until_idle(tmp.path());
    }

    /// A file that cannot be analyzed is counted and does not wedge the queue —
    /// the next upload still gets processed.
    #[test]
    fn a_failed_file_is_counted_and_the_queue_continues() {
        let (tmp, scanner, uploaded) = library();
        let queue = IngestQueue::new();
        let organizer = Organizer::new();

        queue.enqueue(
            tmp.path(),
            organizer.clone(),
            scanner.clone(),
            tmp.path().join("never-written.jpg"),
        );
        queue.enqueue(tmp.path(), organizer, scanner.clone(), uploaded);
        queue.wait_until_idle(tmp.path());

        let status = queue.status(tmp.path());
        assert_eq!(1, status.failed);
        assert_eq!(1, status.completed);

        let mut conn = scanner.open_db().expect("connection");
        let indexed: i64 = files::table.count().get_result(&mut conn).expect("count");
        assert_eq!(1, indexed, "the good file must still have been indexed");
    }
}
