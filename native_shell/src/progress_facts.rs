//! Filesystem facts a progress event is drawn from, gathered *outside* the
//! global Core lock.
//!
//! `CoreRuntime::progress_event` turns a byte count into a full snapshot:
//! media segment counts from `progress.json`, whether a preview playlist or a
//! playable HTTP checkpoint exists, and the connection map from the range
//! checkpoints. Those reads used to happen while holding the global
//! `Mutex<PersistentCore>`: a `preview.m3u8` stat, an output-file stat, a
//! checkpoint stat/parse and (for local copies) two `canonicalize` calls per
//! sample. On Windows each of them may pass through filter drivers, and a
//! slow or disconnected volume stalled every IPC command.
//!
//! Running tasks now register their paths here ([`register`], RAII). Before a
//! worker takes the Core lock or posts to the progress relay it calls
//! [`refresh`], which gathers [`ProgressFacts`] on the worker thread and
//! caches them by task directory. `progress_event` only looks the facts up
//! ([`fresh`]); the painting that remains under the lock is pure computation.
//!
//! Tasks that are not running in this process (commands replayed by tests,
//! one-shot state changes of queued tasks) have no fresh facts and keep the
//! previous behaviour: the facts are gathered at the call site.

use crate::{ResourceKind, TaskPaths, TaskSpec};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Cached facts older than this are gathered again (only reachable when a
/// queued relay sample waited this long for the Core lock).
const FRESH_FOR: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProgressFacts {
    /// `(completed_segments, total_segments)` from a media progress file.
    pub media_counts: Option<(u64, u64)>,
    /// A preview playlist or a complete playable HTTP checkpoint exists.
    pub playable: bool,
    /// Completed (end-exclusive, merged) and active checkpoint ranges, read
    /// only for byte-range transfers without media counts.
    pub ranges: Option<crate::connection_parts::PaintRanges>,
}

impl ProgressFacts {
    /// Reads every fact `progress_event` needs. Touches the filesystem; never
    /// call it while holding the Core lock on a hot path.
    pub(crate) fn gather(paths: &TaskPaths, spec: &TaskSpec) -> Self {
        let media_counts = crate::task_control::read_progress_file(&paths.progress)
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| {
                Some((
                    value.get("completed_segments")?.as_u64()?,
                    value.get("total_segments")?.as_u64()?,
                ))
            });
        let playable = crate::download_worker::playable_task_playlist(paths).is_some()
            || crate::download_worker::playable_http_total(paths, spec).is_some();
        let ranges = (media_counts.is_none()
            && !matches!(
                spec.resource_kind,
                ResourceKind::Hls | ResourceKind::Dash | ResourceKind::Live
            ))
        .then(|| crate::connection_parts::load_paint_ranges(&paths.progress));
        Self {
            media_counts,
            playable,
            ranges,
        }
    }
}

struct Registered {
    paths: TaskPaths,
    spec: Arc<TaskSpec>,
}

#[derive(Default)]
struct State {
    /// `(Core identity, task id)` → paths of a task running in this process.
    running: HashMap<(usize, String), Registered>,
    /// Task directory → last gathered facts.
    facts: HashMap<PathBuf, (Instant, ProgressFacts)>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|error| error.into_inner())
}

/// Identity of a Core instance (the address of its shared mutex).
pub(crate) fn core_key<T>(core: &Arc<T>) -> usize {
    Arc::as_ptr(core) as *const () as usize
}

/// Keeps a running task's paths registered; dropping it forgets the task and
/// its cached facts.
pub(crate) struct Registration {
    key: (usize, String),
    task_dir: PathBuf,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut state = lock();
        state.running.remove(&self.key);
        state.facts.remove(&self.task_dir);
    }
}

/// Registers a task that runs in this process so its workers can gather
/// progress facts before taking the Core lock.
pub(crate) fn register(
    core: usize,
    task_id: &str,
    paths: TaskPaths,
    spec: &TaskSpec,
) -> Registration {
    let task_dir = paths.task_dir();
    let key = (core, task_id.to_string());
    lock().running.insert(
        key.clone(),
        Registered {
            paths,
            spec: Arc::new(spec.clone()),
        },
    );
    Registration { key, task_dir }
}

/// Gathers and caches the facts of a registered task. Call it on the worker
/// thread, without the Core lock, right before posting or applying progress.
pub(crate) fn refresh(core: usize, task_id: &str) {
    let registered = {
        let state = lock();
        state
            .running
            .get(&(core, task_id.to_string()))
            .map(|entry| (entry.paths.clone(), Arc::clone(&entry.spec)))
    };
    let Some((paths, spec)) = registered else {
        return;
    };
    // Filesystem access happens here, outside both the Core lock and this
    // module's lock.
    let facts = ProgressFacts::gather(&paths, &spec);
    let task_dir = paths.task_dir();
    let mut state = lock();
    // Only keep facts of a task that is still registered.
    if state.running.contains_key(&(core, task_id.to_string())) {
        state.facts.insert(task_dir, (Instant::now(), facts));
    }
}

/// Registers a task only for the duration of the returned guard and gathers
/// its facts now (the spec is read under a brief Core lock, every filesystem
/// access happens after it is released). Used for the final state event of a
/// worker whose run-time registration already ended.
pub(crate) fn prepare_for(
    core: &Arc<Mutex<crate::PersistentCore>>,
    task_id: &str,
) -> Option<Registration> {
    let spec = core.lock().ok()?.task_spec(task_id).cloned()?;
    let paths = TaskPaths::for_task(task_id, &spec).ok()?;
    let key = core_key(core);
    let registration = register(key, task_id, paths, &spec);
    refresh(key, task_id);
    Some(registration)
}

/// The cached facts of a task directory, if gathered recently. Pure memory
/// lookup; safe under the Core lock.
pub(crate) fn fresh(task_dir: &Path) -> Option<ProgressFacts> {
    let state = lock();
    state
        .facts
        .get(task_dir)
        .filter(|(at, _)| at.elapsed() <= FRESH_FOR)
        .map(|(_, facts)| facts.clone())
}

#[cfg(test)]
thread_local! {
    static GATHERED_UNDER_LOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts a fallback gather inside `progress_event` (test diagnostics).
pub(crate) fn note_gathered_in_place() {
    #[cfg(test)]
    GATHERED_UNDER_LOCK.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(crate) fn gathered_in_place() -> u64 {
    GATHERED_UNDER_LOCK.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, PersistentCore};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hls-progress-facts-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spec(dir: &Path, kind: ResourceKind, filename: &str) -> TaskSpec {
        TaskSpec {
            url: format!("https://example.test/{filename}"),
            resource_kind: kind,
            title: filename.into(),
            filename: filename.into(),
            download_dir: dir.to_string_lossy().into_owned(),
            request_method: "GET".into(),
            concurrency: 4,
            ..Default::default()
        }
    }

    fn progress(task_id: &str, bytes: u64) -> CoreCommand {
        CoreCommand::UpdateProgress {
            task_id: task_id.into(),
            downloaded_bytes: bytes,
            total_bytes: Some(1 << 20),
            speed_bytes_per_sec: 1,
            stage: "transfer".into(),
            status: "downloading".into(),
        }
    }

    #[test]
    fn registered_task_progress_reads_no_files_under_the_core_lock() {
        let dir = temp_dir("registered");
        let spec = spec(&dir, ResourceKind::Hls, "show.mp4");
        let core = Arc::new(Mutex::new(PersistentCore::in_memory().unwrap()));
        core.lock()
            .unwrap()
            .handle(CoreCommand::CreateTask { spec: spec.clone() })
            .unwrap();
        let paths = TaskPaths::for_task("task-1", &spec).unwrap();
        std::fs::create_dir_all(paths.task_dir()).unwrap();
        std::fs::write(paths.task_dir().join("preview.m3u8"), "#EXTM3U\n").unwrap();
        std::fs::write(
            &paths.progress,
            r#"{"completed_segments":3,"total_segments":9}"#,
        )
        .unwrap();
        let key = core_key(&core);
        let registration = register(key, "task-1", paths.clone(), &spec);
        refresh(key, "task-1");
        // Once gathered, the files can disappear: the event under the lock is
        // drawn from memory only.
        std::fs::remove_dir_all(&dir).unwrap();
        let before = gathered_in_place();
        let events = core.lock().unwrap().handle(progress("task-1", 64)).unwrap();
        assert_eq!(
            gathered_in_place(),
            before,
            "no filesystem gather under the lock"
        );
        let crate::CoreEvent::TaskProgress { snapshot } = &events[0].event else {
            panic!("progress event expected");
        };
        assert!(snapshot.playback_ready);
        assert_eq!((snapshot.completed_ranges, snapshot.total_ranges), (3, 9));
        drop(registration);
        assert!(
            fresh(&paths.task_dir()).is_none(),
            "facts forgotten with the task"
        );
    }

    #[test]
    fn unregistered_or_stale_facts_fall_back_to_the_previous_gather() {
        let dir = temp_dir("fallback");
        let spec = spec(&dir, ResourceKind::Hls, "show.mp4");
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: spec.clone() })
            .unwrap();
        let paths = TaskPaths::for_task("task-1", &spec).unwrap();
        std::fs::create_dir_all(paths.task_dir()).unwrap();
        std::fs::write(paths.task_dir().join("preview.m3u8"), "#EXTM3U\n").unwrap();
        let before = gathered_in_place();
        let events = core.handle(progress("task-1", 64)).unwrap();
        assert_eq!(gathered_in_place(), before + 1);
        let crate::CoreEvent::TaskProgress { snapshot } = &events[0].event else {
            panic!("progress event expected");
        };
        assert!(snapshot.playback_ready, "same result as before the change");
        // An unregistered task never gets cached facts from refresh.
        refresh(0, "task-1");
        assert!(fresh(&paths.task_dir()).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gathered_facts_match_for_http_checkpoints() {
        let dir = temp_dir("http");
        let spec = spec(&dir, ResourceKind::File, "clip.bin");
        let paths = TaskPaths::for_task("task-7", &spec).unwrap();
        std::fs::create_dir_all(paths.task_dir()).unwrap();
        std::fs::write(
            paths.task_dir().join("native-engine.active.json"),
            r#"{"ranges":[[4,6]]}"#,
        )
        .unwrap();
        let facts = ProgressFacts::gather(&paths, &spec);
        assert_eq!(facts.media_counts, None);
        assert!(!facts.playable);
        assert_eq!(facts.ranges, Some((Vec::new(), vec![(4, 6)])));
        let (parts, workers) = crate::connection_parts::paint_from_ranges(
            facts.ranges.clone().unwrap().0,
            facts.ranges.unwrap().1,
            2,
            8,
            true,
        );
        let (expected, expected_workers) =
            crate::connection_parts::paint_from_progress(&paths.progress, 2, 8, true);
        assert_eq!((parts, workers), (expected, expected_workers));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
