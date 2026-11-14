//! 多线程同时经 coordinator 下发命令时，store 必须一条不丢、id 一个不重。
//!
//! 使用公开派发入口和暂停任务，验证调度前的持久化，不启动网络下载。

use hls_native_shell::{CoreCommand, CoreCoordinator, PersistentCore, ResourceKind, TaskSpec};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn temp_db(tag: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "hls-v7-concurrent-dispatch-{tag}-{}-{stamp}.db",
        std::process::id()
    ))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn concurrent_create_task_never_duplicates_a_task_id_or_loses_a_task() {
    let path = temp_db("create");
    let coordinator = Arc::new(CoreCoordinator::new(PersistentCore::open(&path).unwrap()));
    coordinator
        .dispatch(CoreCommand::SetSetting {
            key: "legal_terms_accepted".into(),
            value: serde_json::json!(true),
        })
        .unwrap();
    coordinator
        .dispatch(CoreCommand::SetSetting {
            key: "download_dir".into(),
            value: serde_json::json!(std::env::temp_dir()),
        })
        .unwrap();

    let mut handles = Vec::new();
    for worker in 0..8u32 {
        let shared = Arc::clone(&coordinator);
        handles.push(std::thread::spawn(move || {
            for round in 0..8u32 {
                let spec = TaskSpec {
                    start_paused: true,
                    url: format!("https://example.test/race-{worker}-{round}.bin"),
                    resource_kind: ResourceKind::Hls,
                    filename: format!("race-{worker}-{round}.bin"),
                    download_dir: std::env::temp_dir().to_string_lossy().into_owned(),
                    ..Default::default()
                };
                shared.dispatch(CoreCommand::CreateTask { spec }).unwrap();
            }
        }));
    }
    for handle in handles {
        handle.join().expect("worker thread panicked");
    }

    let core = coordinator.core();
    let tasks = core.lock().unwrap().tasks();
    // 64 条一条不丢。
    assert_eq!(tasks.len(), 64, "concurrent create: tasks lost");
    // id 全部互异——撞号会让两条不同任务共用一个 id，静默污染 store。
    let ids: HashSet<&str> = tasks.iter().map(|task| task.task_id.as_str()).collect();
    assert_eq!(ids.len(), 64, "concurrent create: duplicate task id");
    // 而且必须是连续的 task-1..task-64（counter 单调，不跳不重）。
    let expected: BTreeSet<String> = (1..=64).map(|n| format!("task-{n}")).collect();
    let actual: BTreeSet<String> = ids.iter().map(|id| (*id).to_string()).collect();
    assert_eq!(
        actual, expected,
        "concurrent create: task id sequence not contiguous"
    );

    drop(core);
    drop(coordinator);
    cleanup(&path);
}
