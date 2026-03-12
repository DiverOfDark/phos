use crate::ai::AiPipeline;
use crate::scanner::{is_media_file, Scanner};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// The kind of action to perform on a debounced path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileAction {
    /// The file was created or modified and should be (re-)processed.
    Upsert,
    /// The file was removed and should be deleted from the DB.
    Remove,
}

/// Duration to wait after the last event before processing the batch.
const DEBOUNCE_DURATION: Duration = Duration::from_secs(2);

/// Start watching the given `library_path` for file system changes.
///
/// This function spawns a background thread that:
/// 1. Creates a `notify::RecommendedWatcher` on the library path.
/// 2. Collects events into a debounce buffer.
/// 3. After [`DEBOUNCE_DURATION`] of silence, deduplicates paths and processes
///    them using the [`Scanner`].
///
/// The function returns the [`RecommendedWatcher`] handle so that the caller
/// can keep it alive for the lifetime of the application. Dropping the handle
/// stops watching.
pub fn start_watcher(
    library_path: PathBuf,
    db_path: PathBuf,
    ai: Option<AiPipeline>,
) -> anyhow::Result<RecommendedWatcher> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            // Send the event to the processing thread; ignore send errors
            // (they happen when the receiver is dropped, i.e. shutdown).
            let _ = tx.send(res);
        },
        notify::Config::default(),
    )?;

    watcher.watch(&library_path, RecursiveMode::Recursive)?;
    info!("File watcher started on {:?}", library_path);

    // Spawn the debounce + processing thread.
    std::thread::Builder::new()
        .name("phos-file-watcher".into())
        .spawn(move || {
            run_watcher_loop(rx, &db_path, ai.as_ref());
        })?;

    Ok(watcher)
}

/// The main event-processing loop that runs on a dedicated thread.
///
/// It reads from the channel with a timeout equal to [`DEBOUNCE_DURATION`].
/// When the timeout fires (meaning no new events for that period), it
/// processes all accumulated events in one batch.
fn run_watcher_loop(
    rx: mpsc::Receiver<notify::Result<Event>>,
    db_path: &Path,
    ai: Option<&AiPipeline>,
) {
    // Maps each path to the action that should be taken and the instant of the
    // last event for that path.
    let mut pending: HashMap<PathBuf, (FileAction, Instant)> = HashMap::new();

    loop {
        // If there are pending events, use a timeout so we can flush them.
        // Otherwise, block indefinitely waiting for the next event.
        let event_result = if pending.is_empty() {
            match rx.recv() {
                Ok(ev) => Some(ev),
                Err(_) => {
                    info!("File watcher channel closed, shutting down watcher loop");
                    return;
                }
            }
        } else {
            match rx.recv_timeout(DEBOUNCE_DURATION) {
                Ok(ev) => Some(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Flush remaining events before exiting.
                    if !pending.is_empty() {
                        flush_pending(&mut pending, db_path, ai);
                    }
                    info!("File watcher channel closed, shutting down watcher loop");
                    return;
                }
            }
        };

        if let Some(event_result) = event_result {
            match event_result {
                Ok(event) => collect_event(&mut pending, &event),
                Err(e) => {
                    warn!("File watcher error: {}", e);
                }
            }

            // Check if enough time has passed since the earliest pending event.
            let should_flush = pending
                .values()
                .any(|(_, ts)| ts.elapsed() >= DEBOUNCE_DURATION);
            if should_flush {
                flush_pending(&mut pending, db_path, ai);
            }
        } else {
            // Timeout fired -- flush all pending events.
            flush_pending(&mut pending, db_path, ai);
        }
    }
}

/// Translate a single `notify::Event` into pending actions.
fn collect_event(pending: &mut HashMap<PathBuf, (FileAction, Instant)>, event: &Event) {
    let action = match &event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => Some(FileAction::Upsert),
        EventKind::Remove(_) => Some(FileAction::Remove),
        _ => None,
    };

    if let Some(action) = action {
        for path in &event.paths {
            // Only care about media files, skip .phos* directories (thumbnails, db)
            if path.is_dir() || !is_media_file(path) {
                continue;
            }
            if path.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .map(|s| s.starts_with(".phos") || s == ".duplicates")
                    .unwrap_or(false)
            }) {
                continue;
            }
            debug!("Watcher event: {:?} on {:?}", action, path);
            pending.insert(path.clone(), (action.clone(), Instant::now()));
        }
    }
}

/// Process all pending file actions and clear the map.
fn flush_pending(
    pending: &mut HashMap<PathBuf, (FileAction, Instant)>,
    db_path: &Path,
    ai: Option<&AiPipeline>,
) {
    if pending.is_empty() {
        return;
    }

    info!("Processing {} debounced file watcher events", pending.len());

    let scanner = Scanner::new(db_path.to_path_buf(), None);

    let conn = match scanner.open_db() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to open DB for watcher processing: {}", e);
            pending.clear();
            return;
        }
    };

    // Build a dHash cache so new files can be grouped into existing shots.
    // Created once before the loop so that files in the same watcher batch
    // can still match against each other.
    let dhash_cache = std::sync::Mutex::new(Vec::<crate::scanner::DHashCacheEntry>::new());

    // Drain the map so we process each path exactly once.
    let actions: Vec<(PathBuf, FileAction)> = pending
        .drain()
        .map(|(path, (action, _))| (path, action))
        .collect();

    for (path, action) in actions {
        match action {
            FileAction::Upsert => {
                // File may have been moved/deleted between the event and now
                // (e.g. during reorganize). Skip silently.
                if !path.exists() {
                    debug!("Watcher: path {:?} no longer exists, skipping", path);
                    continue;
                }
                let _ = ai;
                if let Err(e) = scanner.process_file(&conn, &path, &dhash_cache) {
                    error!("Watcher: failed to process {:?}: {}", path, e);
                }
            }
            FileAction::Remove => {
                if let Err(e) = scanner.remove_file(&conn, &path) {
                    warn!("Watcher: failed to remove {:?}: {}", path, e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Instant;

    #[test]
    fn test_collect_event_create() {
        let mut pending = HashMap::new();
        let event = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![PathBuf::from("/tmp/photo.jpg")],
            attrs: Default::default(),
        };
        collect_event(&mut pending, &event);
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending.get(Path::new("/tmp/photo.jpg")).unwrap().0,
            FileAction::Upsert
        );
    }

    #[test]
    fn test_collect_event_remove() {
        let mut pending = HashMap::new();
        let event = Event {
            kind: EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![PathBuf::from("/tmp/photo.png")],
            attrs: Default::default(),
        };
        collect_event(&mut pending, &event);
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending.get(Path::new("/tmp/photo.png")).unwrap().0,
            FileAction::Remove
        );
    }

    #[test]
    fn test_collect_event_non_media_ignored() {
        let mut pending = HashMap::new();
        let event = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![PathBuf::from("/tmp/notes.txt")],
            attrs: Default::default(),
        };
        collect_event(&mut pending, &event);
        assert!(pending.is_empty());
    }

    #[test]
    fn test_collect_event_modify_overwrites_previous() {
        let mut pending = HashMap::new();
        let path = PathBuf::from("/tmp/photo.jpg");

        // First: create
        let event1 = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };
        collect_event(&mut pending, &event1);
        assert_eq!(pending[&path].0, FileAction::Upsert);

        // Then: remove -- should replace the action
        let event2 = Event {
            kind: EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };
        collect_event(&mut pending, &event2);
        assert_eq!(pending[&path].0, FileAction::Remove);
    }

    #[test]
    fn test_flush_pending_clears_map() {
        let mut pending: HashMap<PathBuf, (FileAction, Instant)> = HashMap::new();
        pending.insert(
            PathBuf::from("/nonexistent/photo.jpg"),
            (FileAction::Remove, Instant::now()),
        );
        // Use a non-existent DB path -- flush should handle the error gracefully
        // and still clear the map.
        flush_pending(&mut pending, Path::new("/tmp/nonexistent_test.db"), None);
        assert!(pending.is_empty());
    }
}
