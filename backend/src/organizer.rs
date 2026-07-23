//! Debounced background file reorganizer.
//!
//! API endpoints and the file watcher signal the organizer whenever
//! shot→person assignments change (confirm, reassign, merge, new files).
//! A per-library worker thread debounces those signals and runs
//! [`crate::import::run_reorganize`], so files on disk follow the clustering
//! shortly after a change instead of waiting for a restart. Each worker also
//! runs a periodic fallback pass every [`PERIODIC_INTERVAL`].
//!
//! All reorganize runs for a library — debounced, periodic, and explicit
//! ([`Organizer::run_now`]) — are serialized through a per-library lock so
//! two runs never race on renaming the same files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tracing::{error, info};

/// Quiet period after the last signal before a reorganize run starts.
const DEBOUNCE: Duration = Duration::from_secs(15);

/// Fallback interval between reorganize runs when no signals arrive.
const PERIODIC_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Default)]
struct WorkerState {
    /// `Some` while a reorganize request is pending; the instant of the most
    /// recent signal, used as the debounce anchor.
    last_signal: Option<Instant>,
    stopped: bool,
}

struct Worker {
    library_root: PathBuf,
    state: Mutex<WorkerState>,
    cvar: Condvar,
    /// Serializes actual `run_reorganize` executions (worker thread vs `run_now`).
    run_lock: Mutex<()>,
}

/// Registry of per-library reorganize workers. One instance is shared through
/// `AppState`; workers are spawned lazily per library root.
pub struct Organizer {
    workers: Mutex<HashMap<PathBuf, Arc<Worker>>>,
}

impl Organizer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            workers: Mutex::new(HashMap::new()),
        })
    }

    /// Ensure a worker exists for `library_root` so it gets periodic
    /// fallback runs even if no signals ever arrive.
    pub fn watch(&self, library_root: &Path) {
        self.worker_for(library_root);
    }

    /// Request a debounced reorganize of `library_root`. Cheap; safe to call
    /// from request handlers. Multiple signals coalesce into one run that
    /// starts [`DEBOUNCE`] after the last signal.
    pub fn signal(&self, library_root: &Path) {
        let worker = self.worker_for(library_root);
        let mut state = worker.state.lock().unwrap();
        state.last_signal = Some(Instant::now());
        worker.cvar.notify_all();
    }

    /// Run reorganize synchronously, serialized against the background
    /// worker. Clears any pending debounced request since this run covers it.
    pub fn run_now(&self, library_root: &Path) -> anyhow::Result<()> {
        let worker = self.worker_for(library_root);
        worker.state.lock().unwrap().last_signal = None;
        let _guard = worker.run_lock.lock().unwrap();
        crate::import::run_reorganize(library_root, false)
    }

    /// Stop all worker threads. In-flight reorganize runs finish first.
    pub fn shutdown(&self) {
        let workers = self.workers.lock().unwrap();
        for worker in workers.values() {
            worker.state.lock().unwrap().stopped = true;
            worker.cvar.notify_all();
        }
    }

    fn worker_for(&self, library_root: &Path) -> Arc<Worker> {
        let mut workers = self.workers.lock().unwrap();
        if let Some(worker) = workers.get(library_root) {
            return worker.clone();
        }
        let worker = Arc::new(Worker {
            library_root: library_root.to_path_buf(),
            state: Mutex::new(WorkerState::default()),
            cvar: Condvar::new(),
            run_lock: Mutex::new(()),
        });
        let thread_worker = worker.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("phos-organizer".into())
            .spawn(move || run_worker_loop(&thread_worker))
        {
            error!("Failed to spawn organizer worker: {}", e);
        }
        workers.insert(library_root.to_path_buf(), worker.clone());
        worker
    }
}

fn run_worker_loop(worker: &Worker) {
    info!("Organizer worker started for {:?}", worker.library_root);
    loop {
        // Wait until a pending signal has been quiet for DEBOUNCE, the
        // periodic interval elapses with no signal, or shutdown.
        let mut state = worker.state.lock().unwrap();
        loop {
            if state.stopped {
                info!("Organizer worker for {:?} stopped", worker.library_root);
                return;
            }
            match state.last_signal {
                Some(anchor) => {
                    let elapsed = anchor.elapsed();
                    if elapsed >= DEBOUNCE {
                        state.last_signal = None;
                        break;
                    }
                    let (guard, _) = worker
                        .cvar
                        .wait_timeout(state, DEBOUNCE - elapsed)
                        .unwrap();
                    state = guard;
                }
                None => {
                    let (guard, timeout) = worker
                        .cvar
                        .wait_timeout(state, PERIODIC_INTERVAL)
                        .unwrap();
                    state = guard;
                    if timeout.timed_out() && state.last_signal.is_none() && !state.stopped {
                        break;
                    }
                }
            }
        }
        drop(state);

        let _guard = worker.run_lock.lock().unwrap();
        // Fresh libraries have no DB yet — nothing to reorganize.
        if !worker.library_root.join(".phos.db").exists() {
            continue;
        }
        info!("Organizer: reorganizing {:?}", worker.library_root);
        if let Err(e) = crate::import::run_reorganize(&worker.library_root, false) {
            error!(
                "Organizer: reorganize failed for {:?}: {}",
                worker.library_root, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_coalesces_and_worker_spawns() {
        let organizer = Organizer::new();
        let root = Path::new("/nonexistent/phos-test-library");
        organizer.signal(root);
        organizer.signal(root);
        assert_eq!(organizer.workers.lock().unwrap().len(), 1);
        let worker = organizer.worker_for(root);
        assert!(worker.state.lock().unwrap().last_signal.is_some());
        organizer.shutdown();
    }

    #[test]
    fn test_watch_registers_worker_without_pending_signal() {
        let organizer = Organizer::new();
        let root = Path::new("/nonexistent/phos-test-library-2");
        organizer.watch(root);
        let worker = organizer.worker_for(root);
        assert!(worker.state.lock().unwrap().last_signal.is_none());
        organizer.shutdown();
    }
}
