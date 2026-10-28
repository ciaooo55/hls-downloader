//! Durable command boundary shared by the UI and protocol front-ends.

use crate::{
    AvScanStatus, CoreCommand, CoreRuntime, CoreStore, EventEnvelope, MirrorStatus, TaskFailure,
    TaskSnapshot, TaskSpec,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

pub struct PersistentCore {
    runtime: CoreRuntime,
    store: CoreStore,
    event_notify: Arc<(Mutex<u64>, Condvar)>,
}

impl PersistentCore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::from_store(CoreStore::open(path)?)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::from_store(CoreStore::in_memory()?)
    }

    pub fn handle(&mut self, command: CoreCommand) -> Result<Vec<EventEnvelope>, String> {
        if let CoreCommand::GetTaskLog { task_id } = &command {
            let mut lines = self.store.load_task_log(task_id, 500)?;
            if lines.is_empty() {
                lines = self
                    .runtime
                    .snapshot(task_id)
                    .map(|snapshot| snapshot.log_tail.clone())
                    .unwrap_or_default();
            }
            return self.emit(crate::CoreEvent::TaskLog {
                task_id: task_id.clone(),
                lines,
            });
        }
        if let CoreCommand::UpdateProgress {
            task_id,
            downloaded_bytes,
            total_bytes,
            speed_bytes_per_sec,
            stage,
            status,
        } = &command
        {
            let state_changed = self.runtime.snapshot(task_id)
                .is_some_and(|previous| previous.stage != *stage || previous.status != *status);
            let event = self.runtime.progress_event(
                task_id,
                *downloaded_bytes,
                *total_bytes,
                *speed_bytes_per_sec,
                stage,
                status,
            );
            let events = vec![EventEnvelope {
                sequence: self.runtime.latest_sequence() + 1,
                event,
            }];
            // 失败时运行态和事件环尚未改变；成功后才发布同一序号并唤醒订阅者。
            self.store.apply_events(&events)?;
            self.runtime.commit_progress(events[0].event.clone());
            self.notify_committed(&events);
            if state_changed {
                crate::record_runtime_log("task_state_changed", serde_json::json!({
                    "task_id": task_id, "stage": stage, "status": status,
                    "downloaded_bytes": downloaded_bytes, "total_bytes": total_bytes,
                }));
            }
            return Ok(events);
        }
        if let CoreCommand::RequestMediaPush { request } = &command {
            let active = self
                .store
                .query_handoffs(None, None, true)?
                .into_iter()
                .filter_map(|encoded| {
                    serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok()
                })
                .find(|item| item.status == "pending");
            if let Some(active) = active {
                if active.id == request.id {
                    return Ok(Vec::new());
                }
                let mut rejected = request.clone();
                rejected.status = "failed".into();
                rejected.message = "已有投送请求正在等待设备选择".into();
                rejected.location.clear();
                return self.emit(crate::CoreEvent::MediaPushResolved { request: rejected });
            }
        }
        if let CoreCommand::ResolveMediaPush {
            request_id, status, ..
        } = &command
        {
            if matches!(status.as_str(), "done" | "failed" | "canceled") {
                let persisted = self
                    .store
                    .query_handoffs(Some(request_id), None, false)?
                    .into_iter()
                    .filter_map(|encoded| {
                        serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok()
                    })
                    .find(|item| item.id == *request_id);
                match persisted {
                    Some(item) if item.status != "pending" => return Ok(Vec::new()),
                    Some(item) => self.runtime.restore_pending_media_push(item),
                    None => {}
                }
            }
        }
        let before = self.runtime.clone();
        let created_spec = match &command {
            CoreCommand::CreateTask { spec } => Some(spec.clone()),
            _ => None,
        };
        let events = self.runtime.handle(command);
        let spec = created_spec.or_else(|| {
            events.iter().find_map(|envelope| match &envelope.event {
                crate::CoreEvent::TaskCreated { snapshot } => {
                    self.runtime.task_spec(&snapshot.task_id).cloned()
                }
                _ => None,
            })
        });
        if let Err(error) = self.store.apply_events_and_spec(&events, spec.as_ref()) {
            self.runtime = before;
            return Err(error);
        }
        self.notify_committed(&events);
        Ok(events)
    }

    pub fn assign_queue_and_set_settings(
        &mut self,
        task_ids: Vec<String>,
        queue_id: String,
        values: &BTreeMap<String, serde_json::Value>,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let mut events = self
            .runtime
            .handle(CoreCommand::AssignQueue { task_ids, queue_id });
        events.extend(self.runtime.emit(crate::CoreEvent::SettingsChanged {
            keys: values.keys().cloned().collect(),
        }));
        if let Err(error) = self.store.apply_events_and_settings(&events, values) {
            self.runtime = before;
            return Err(error);
        }
        self.notify_committed(&events);
        Ok(events)
    }

    pub fn emit(&mut self, event: crate::CoreEvent) -> Result<Vec<EventEnvelope>, String> {
        let envelope = EventEnvelope {
            sequence: self.runtime.latest_sequence() + 1,
            event,
        };
        // 单事件发布只改变事件环和序号；先提交数据库，无需复制整个运行态来回滚。
        self.store.apply_events(std::slice::from_ref(&envelope))?;
        let events = self.runtime.emit(envelope.event);
        self.notify_committed(&events);
        Ok(events)
    }

    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.runtime.list_tasks()
    }

    pub(crate) fn snapshot(&self, task_id: &str) -> Option<&TaskSnapshot> {
        self.runtime.snapshot(task_id)
    }

    pub fn mark_output_missing(
        &mut self,
        task_id: &str,
        missing: bool,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.mark_output_missing(task_id, missing);
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.notify_committed(&events);
        Ok(events)
    }

    pub fn report_failure(
        &mut self,
        task_id: &str,
        failure: TaskFailure,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.report_failure(task_id, failure);
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.notify_committed(&events);
        Ok(events)
    }

    pub fn set_output_path(
        &mut self,
        task_id: &str,
        path: String,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_output_path(task_id, path);
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.notify_committed(&events);
        Ok(events)
    }

    pub fn set_checksum_result(
        &mut self,
        task_id: &str,
        algorithm: String,
        actual: String,
        verified: bool,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime
            .set_checksum_result(task_id, algorithm, actual, verified);
        self.persist_runtime_events(before, sequence)
    }

    pub fn set_av_scan_result(
        &mut self,
        task_id: &str,
        result: AvScanStatus,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_av_scan_result(task_id, result);
        self.persist_runtime_events(before, sequence)
    }

    pub fn set_mirror_result(
        &mut self,
        task_id: &str,
        statuses: Vec<MirrorStatus>,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_mirror_result(task_id, statuses);
        self.persist_runtime_events(before, sequence)
    }

    pub fn set_torrent_telemetry(
        &mut self,
        task_id: &str,
        peer_count: u32,
        seed_count: u32,
        uploaded_bytes: u64,
        upload_speed_bytes_per_sec: u64,
    ) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.set_torrent_telemetry(
            task_id,
            peer_count,
            seed_count,
            uploaded_bytes,
            upload_speed_bytes_per_sec,
        );
        self.persist_runtime_events(before, sequence)
    }

    pub fn task_spec(&self, task_id: &str) -> Option<&TaskSpec> {
        self.runtime.task_spec(task_id)
    }

    pub fn replace_spec(&mut self, task_id: &str, spec: TaskSpec) -> Result<(), String> {
        let before = self.runtime.clone();
        let sequence = self.runtime.latest_sequence();
        self.runtime.replace_spec(task_id, spec.clone());
        let events = self.runtime.events_after(sequence, 16);
        if let Err(error) = self.store.apply_events_and_spec(&events, Some(&spec)) {
            self.runtime = before;
            return Err(error);
        }
        self.notify_committed(&events);
        Ok(())
    }

    pub fn pending_handoff(&self, handoff_id: &str) -> Option<crate::ResourceOffer> {
        self.runtime.pending_handoff(handoff_id).cloned()
    }

    pub fn take_pending_handoff(&mut self, handoff_id: &str) -> Option<crate::ResourceOffer> {
        self.runtime.take_pending_handoff(handoff_id)
    }

    pub fn latest_sequence(&self) -> u64 {
        self.runtime.latest_sequence()
    }

    pub fn browser_connected(&self) -> bool {
        self.runtime.browser_connected()
    }

    pub fn events_after(&self, sequence: u64, limit: usize) -> Vec<EventEnvelope> {
        self.runtime.events_after(sequence, limit)
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
    }

    pub(crate) fn event_notify(&self) -> Arc<(Mutex<u64>, Condvar)> {
        Arc::clone(&self.event_notify)
    }

    fn notify_committed(&self, events: &[EventEnvelope]) {
        let Some(last) = events.last() else {
            return;
        };
        if let Ok(mut sequence) = self.event_notify.0.lock() {
            *sequence = last.sequence;
            self.event_notify.1.notify_all();
        }
    }

    fn persist_runtime_events(
        &mut self,
        before: CoreRuntime,
        sequence: u64,
    ) -> Result<Vec<EventEnvelope>, String> {
        let events = self.runtime.events_after(sequence, 16);
        if events.is_empty() {
            return Ok(events);
        }
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        self.notify_committed(&events);
        Ok(events)
    }

    fn from_store(mut store: CoreStore) -> Result<Self, String> {
        let snapshots = store.load_tasks()?;
        let specs = store.load_task_specs()?;
        let sequence = store.latest_sequence()?;
        let mut runtime = CoreRuntime::from_state(snapshots, specs, sequence);
        let mut restored_media_push = false;
        for encoded in store.query_handoffs(None, None, true)? {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&encoded) {
                if let Ok(mut request) =
                    serde_json::from_value::<crate::MediaPushRequest>(value.clone())
                {
                    if request.status == "pending" && !request.id.trim().is_empty() {
                        if restored_media_push {
                            request.status = "failed".into();
                            request.message =
                                "升级恢复时检测到已有更早的投送请求等待设备选择".into();
                            request.location.clear();
                            let normalized = serde_json::to_string(&request).map_err(|error| {
                                format!("encode normalized media push {}: {error}", request.id)
                            })?;
                            store.save_handoff(
                                &request.id,
                                &normalized,
                                &request.status,
                                None,
                                request.created_at_ms,
                            )?;
                        } else {
                            restored_media_push = true;
                            runtime.restore_pending_media_push(request);
                        }
                    }
                    continue;
                }
                let status = value
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if status != "pending" {
                    continue;
                }
                if let Some(offer_value) = value.get("offer") {
                    if let Ok(mut offer) =
                        serde_json::from_value::<crate::ResourceOffer>(offer_value.clone())
                    {
                        if offer.handoff_id.trim().is_empty() {
                            offer.handoff_id = value
                                .get("id")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string();
                        }
                        runtime.restore_pending_handoff(offer);
                    }
                }
            }
        }
        Ok(Self {
            event_notify: Arc::new((Mutex::new(runtime.latest_sequence()), Condvar::new())),
            runtime,
            store,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, ResourceKind, TaskSpec};

    #[test]
    fn failed_commit_restores_snapshot_spec_sequence_and_notification() {
        let root = std::env::temp_dir().join(format!(
            "hls-core-rollback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("core.db");
        let mut core = PersistentCore::open(&path).unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        // 填满历史环；失败的进度不能挤掉最早事件或推进订阅游标。
        for index in 0..4096 {
            core.runtime.emit(crate::CoreEvent::Toast {
                level: "info".into(),
                message: index.to_string(),
            });
        }
        core.emit(crate::CoreEvent::Toast {
            level: "info".into(),
            message: "history ready".into(),
        })
        .unwrap();
        let tasks = core.tasks();
        let spec = core.task_spec("task-1").unwrap().clone();
        let sequence = core.latest_sequence();
        let events = core.events_after(0, 4096);
        let progress = CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 1024,
            total_bytes: Some(4096),
            speed_bytes_per_sec: 512,
            stage: "transfer".into(),
            status: "downloading".into(),
        };
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_task_write BEFORE INSERT ON tasks BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();

        assert!(core.handle(progress.clone()).is_err());
        assert!(core
            .set_output_path("task-1", "changed.bin".into())
            .is_err());
        let mut changed = spec.clone();
        changed.url = "https://example.test/changed.bin".into();
        assert!(core.replace_spec("task-1", changed).is_err());
        assert_eq!(core.tasks(), tasks);
        assert_eq!(core.store().load_tasks().unwrap(), tasks);
        assert_eq!(core.task_spec("task-1"), Some(&spec));
        assert_eq!(core.store().load_task_specs().unwrap()[0].1, spec);
        assert_eq!(core.latest_sequence(), sequence);
        assert_eq!(core.events_after(0, 4096), events);
        assert_eq!(*core.event_notify.0.lock().unwrap(), sequence);

        connection
            .execute_batch("DROP TRIGGER reject_task_write")
            .unwrap();
        // 在快照和日志已写入事务后再失败，二者仍必须与事件检查点一起回滚。
        connection.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON event_checkpoints BEGIN SELECT RAISE(ABORT, 'injected checkpoint failure'); END;").unwrap();
        assert!(core.handle(progress.clone()).is_err());
        let player_event = serde_json::from_value(serde_json::json!({
            "kind": "player_session", "active": true, "title": "Video",
            "task_id": "task-1", "status": "PAUSED", "paused": true,
            "position_seconds": 12.5, "duration_seconds": 100.0
        }))
        .unwrap();
        assert!(core.emit(player_event).is_err());
        assert!(core
            .emit(crate::CoreEvent::MediaPushRequested {
                request: crate::MediaPushRequest {
                    id: "must-not-persist".into(),
                    push_kind: "tvbox".into(),
                    url: "https://example.test/video.mp4".into(),
                    title: "Video".into(),
                    status: "pending".into(),
                    message: String::new(),
                    location: String::new(),
                    created_at_ms: 1,
                },
            })
            .is_err());
        assert!(core.store().load_handoffs().unwrap().is_empty());
        assert_eq!(core.tasks(), tasks);
        assert_eq!(core.store().load_tasks().unwrap(), tasks);
        assert!(core.store().load_task_log("task-1", 500).unwrap().is_empty());
        assert_eq!(core.store().latest_sequence().unwrap(), sequence);
        assert_eq!(core.latest_sequence(), sequence);
        assert_eq!(core.events_after(0, 4096), events);
        assert_eq!(*core.event_notify.0.lock().unwrap(), sequence);
        connection
            .execute_batch("DROP TRIGGER reject_checkpoint")
            .unwrap();
        let committed = core.handle(progress).unwrap();
        assert_eq!(committed[0].sequence, sequence + 1);
        assert_eq!(core.events_after(sequence, 1), committed);
        assert_eq!(core.snapshot("task-1").unwrap().downloaded_bytes, 1024);
        assert_eq!(core.store().load_tasks().unwrap(), core.tasks());
        core.set_output_path("task-1", "committed.bin".into())
            .unwrap();
        assert_eq!(*core.event_notify.0.lock().unwrap(), core.latest_sequence());
        assert_eq!(core.events_after(sequence, 1)[0].sequence, sequence + 1);
        drop(connection);
        drop(core);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_progress_preserves_runtime_lifecycle_and_other_tasks() {
        let mut core = PersistentCore::in_memory().unwrap();
        let mut runtime = CoreRuntime::new();
        let progress = |task_id: &str, bytes, status: &str, stage: &str| {
            CoreCommand::UpdateProgress {
                task_id: task_id.into(),
                downloaded_bytes: bytes,
                total_bytes: Some(4096),
                speed_bytes_per_sec: 512,
                stage: stage.into(),
                status: status.into(),
            }
        };
        for command in [
            CoreCommand::CreateTask { spec: test_spec() },
            CoreCommand::CreateTask { spec: test_spec() },
            progress("task-1", 512, "downloading", "transfer"),
            CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "pause".into(),
            },
            progress("task-1", 768, "downloading", "transfer"),
            CoreCommand::TaskAction {
                task_id: "task-1".into(),
                action: "resume".into(),
            },
            progress("task-1", 2048, "downloading", "transfer"),
            progress("task-1", 4096, "completed", "completed"),
            progress("task-1", 3072, "downloading", "transfer"),
            progress("missing-task", 512, "downloading", "transfer"),
        ] {
            let expected = runtime.handle(command.clone());
            let actual = core.handle(command).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(core.tasks(), runtime.list_tasks());
            assert_eq!(core.store().load_tasks().unwrap(), core.tasks());
            assert_eq!(core.latest_sequence(), runtime.latest_sequence());
            assert_eq!(*core.event_notify.0.lock().unwrap(), core.latest_sequence());
        }
    }

    #[test]
    fn worker_commit_wakes_subscriber_without_an_ipc_command() {
        let core = Arc::new(Mutex::new(PersistentCore::in_memory().unwrap()));
        let notify = core.lock().unwrap().event_notify();
        let guard = notify.0.lock().unwrap();
        let worker_core = Arc::clone(&core);
        let worker = std::thread::spawn(move || {
            worker_core
                .lock()
                .unwrap()
                .emit(crate::CoreEvent::Toast {
                    level: "info".into(),
                    message: "worker completed".into(),
                })
                .unwrap();
        });
        let (sequence, timeout) = notify
            .1
            .wait_timeout_while(guard, std::time::Duration::from_secs(5), |sequence| {
                *sequence == 0
            })
            .unwrap();
        assert!(!timeout.timed_out());
        assert_eq!(*sequence, 1);
        drop(sequence);
        worker.join().unwrap();
        assert_eq!(core.lock().unwrap().events_after(0, 256).len(), 1);
    }

    #[test]
    fn service_persists_commands_through_the_shared_boundary() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask {
            spec: TaskSpec {
                url: "https://example.test/file.bin".into(),
                resource_kind: ResourceKind::File,
                title: "File".into(),
                filename: "file.bin".into(),
                download_dir: String::new(),
                request_method: "GET".into(),
                credential_ref: None,
                replay_context_ref: None,
                concurrency: 8,
                checksum: None,
                expected_size: None,
                etag: String::new(),
                last_modified: String::new(),
                ..Default::default()
            },
        })
        .unwrap();
        assert_eq!(core.tasks().len(), 1);
        assert_eq!(core.store().load_tasks().unwrap(), core.tasks());
        assert_eq!(
            core.store().load_task_specs().unwrap()[0].1.url,
            "https://example.test/file.bin"
        );
    }

    #[test]
    fn spec_updates_refresh_snapshot_and_task_log() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        let mut spec = core.task_spec("task-1").unwrap().clone();
        spec.speed_limit_kib = 256;
        core.replace_spec("task-1", spec).unwrap();
        assert_eq!(core.tasks()[0].speed_limit_kib, 256);
        assert_eq!(core.store().load_tasks().unwrap()[0].speed_limit_kib, 256);

        core.handle(CoreCommand::UpdateProgress {
            task_id: "task-1".into(),
            downloaded_bytes: 1024,
            total_bytes: Some(4096),
            speed_bytes_per_sec: 512,
            stage: "transfer".into(),
            status: "downloading".into(),
        })
        .unwrap();
        let events = core
            .handle(CoreCommand::GetTaskLog {
                task_id: "task-1".into(),
            })
            .unwrap();
        assert!(matches!(
            &events[0].event,
            crate::CoreEvent::TaskLog { lines, .. }
                if lines.last().is_some_and(|line| line.contains("1024/4096"))
        ));
    }

    #[test]
    fn file_store_restores_tasks_after_core_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v6-core-restart-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::CreateTask { spec: test_spec() })
                .unwrap();
            core.handle(CoreCommand::AssignQueue {
                task_ids: vec!["task-1".into()],
                queue_id: "night-media".into(),
            })
            .unwrap();
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(reopened.tasks().len(), 1);
        assert_eq!(reopened.tasks()[0].task_id, "task-1");
        assert_eq!(reopened.tasks()[0].queue_id, "night-media");
        assert_eq!(
            reopened.runtime.task_spec("task-1").unwrap().url,
            "https://example.test/restart.bin"
        );
        assert_eq!(
            reopened.runtime.task_spec("task-1").unwrap().queue_id,
            "night-media"
        );
        assert_eq!(reopened.store().latest_sequence().unwrap(), 2);
        let events = reopened.handle(CoreCommand::Ping).unwrap();
        assert_eq!(events[0].sequence, 3);
        // Windows 上无法删除仍被 SQLite 持有的文件：先释放 owner 再清理，
        // 否则 `remove_file` 静默失败并在 %TEMP% 堆积测试数据库。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn file_store_new_task_after_restart_never_reuses_a_restored_task_id() {
        // 重启后 next_task 必须从已恢复的 task-N 最大值继续（core_runtime.rs:54 的 max 折叠）。
        // 这条不变式没有任何测试钉住：一旦 counter 归零，新任务会拿到 task-1，
        // 与恢复出来的 task-1 撞号——同 id 两条不同任务，静默污染 store。
        let path = std::env::temp_dir().join(format!(
            "hls-v7-core-restart-idcollision-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::CreateTask { spec: test_spec() })
                .unwrap();
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(reopened.tasks().len(), 1);
        assert_eq!(reopened.tasks()[0].task_id, "task-1");
        // 重启后再建一条：必须拿到 task-2，绝不能回到 task-1。
        reopened
            .handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        let ids: Vec<String> = reopened
            .tasks()
            .iter()
            .map(|task| task.task_id.clone())
            .collect();
        assert_eq!(ids, vec!["task-1".to_string(), "task-2".to_string()]);
        // 再建一条，确认 counter 不是只跳过一次。
        reopened
            .handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        assert_eq!(reopened.tasks().len(), 3);
        assert_eq!(reopened.tasks()[2].task_id, "task-3");
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn queue_profile_removal_migrates_tasks_and_settings_together() {
        let mut core = PersistentCore::in_memory().unwrap();
        core.handle(CoreCommand::CreateTask { spec: test_spec() })
            .unwrap();
        core.handle(CoreCommand::AssignQueue {
            task_ids: vec!["task-1".into()],
            queue_id: "removed".into(),
        })
        .unwrap();
        let profiles = serde_json::json!([{
            "id": "default",
            "name": "默认队列"
        }]);
        let values = BTreeMap::from([("queue_profiles".into(), profiles.clone())]);

        core.assign_queue_and_set_settings(
            vec!["task-1".into()],
            crate::DEFAULT_QUEUE_ID.into(),
            &values,
        )
        .unwrap();

        assert_eq!(core.tasks()[0].queue_id, crate::DEFAULT_QUEUE_ID);
        assert_eq!(
            core.store().setting_string("queue_profiles", "").unwrap(),
            profiles.to_string()
        );
    }

    #[test]
    fn concurrent_media_push_is_failed_without_replacing_active_request() {
        let mut core = PersistentCore::in_memory().unwrap();
        let first = crate::MediaPushRequest {
            id: "push-first".into(),
            push_kind: "cast".into(),
            url: "https://example.test/first.mp4".into(),
            title: "First".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 1,
        };
        let second = crate::MediaPushRequest {
            id: "push-second".into(),
            push_kind: "tvbox".into(),
            url: "https://example.test/second.mp4".into(),
            title: "Second".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 2,
        };

        let admitted = core
            .handle(CoreCommand::RequestMediaPush {
                request: first.clone(),
            })
            .unwrap();
        assert!(admitted.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushRequested { request } if request.id == first.id
        )));

        let duplicate = core
            .handle(CoreCommand::RequestMediaPush {
                request: first.clone(),
            })
            .unwrap();
        assert!(duplicate.is_empty());

        let rejected = core
            .handle(CoreCommand::RequestMediaPush {
                request: second.clone(),
            })
            .unwrap();
        assert!(rejected.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushResolved { request }
                if request.id == second.id
                    && request.status == "failed"
                    && request.message.contains("已有投送请求")
        )));
        assert!(!rejected.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushRequested { request } if request.id == second.id
        )));

        let rows: Vec<crate::MediaPushRequest> = core
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|row| serde_json::from_str(&row).ok())
            .collect();
        assert!(rows
            .iter()
            .any(|request| request.id == first.id && request.status == "pending"));
        assert!(rows
            .iter()
            .any(|request| request.id == second.id && request.status == "failed"));

        core.handle(CoreCommand::ResolveMediaPush {
            request_id: first.id.clone(),
            status: "done".into(),
            message: "ok".into(),
            location: String::new(),
        })
        .unwrap();
        let third = crate::MediaPushRequest {
            id: "push-third".into(),
            created_at_ms: 3,
            ..second
        };
        let next = core
            .handle(CoreCommand::RequestMediaPush { request: third })
            .unwrap();
        assert!(next
            .iter()
            .any(|event| matches!(event.event, crate::CoreEvent::MediaPushRequested { .. })));
    }

    #[test]
    fn media_push_terminal_resolution_rehydrates_missing_runtime_request() {
        let mut core = PersistentCore::in_memory().unwrap();
        let request = crate::MediaPushRequest {
            id: "push-runtime-rehydrate".into(),
            push_kind: "cast".into(),
            url: "https://example.test/runtime-rehydrate.mp4".into(),
            title: "Runtime rehydrate".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 8,
        };
        core.handle(CoreCommand::RequestMediaPush {
            request: request.clone(),
        })
        .unwrap();
        core.runtime = CoreRuntime::new();

        let events = core
            .handle(CoreCommand::ResolveMediaPush {
                request_id: request.id.clone(),
                status: "done".into(),
                message: "sent".into(),
                location: "http://192.168.1.9/media/runtime-rehydrate".into(),
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushResolved { request }
                if request.id == "push-runtime-rehydrate" && request.status == "done"
        )));
        let persisted = core
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok())
            .find(|item| item.id == request.id)
            .unwrap();
        assert_eq!(persisted.status, "done");
        assert_eq!(persisted.message, "sent");
        assert_eq!(
            persisted.location,
            "http://192.168.1.9/media/runtime-rehydrate"
        );
    }

    #[test]
    fn media_push_terminal_resolution_retry_is_idempotent_after_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-media-push-terminal-retry-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let request = crate::MediaPushRequest {
            id: "push-terminal-retry".into(),
            push_kind: "cast".into(),
            url: "https://example.test/retry.mp4".into(),
            title: "Retry media".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 7,
        };
        let terminal = CoreCommand::ResolveMediaPush {
            request_id: request.id.clone(),
            status: "done".into(),
            message: "sent".into(),
            location: "http://192.168.1.8/media/retry".into(),
        };
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::RequestMediaPush {
                request: request.clone(),
            })
            .unwrap();
            let first = core.handle(terminal.clone()).unwrap();
            assert!(first.iter().any(|event| matches!(
                &event.event,
                crate::CoreEvent::MediaPushResolved { request }
                    if request.id == "push-terminal-retry" && request.status == "done"
            )));
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        assert!(reopened.handle(terminal).unwrap().is_empty());
        let conflicting_retry = CoreCommand::ResolveMediaPush {
            request_id: request.id.clone(),
            status: "failed".into(),
            message: "late failure".into(),
            location: String::new(),
        };
        assert!(reopened.handle(conflicting_retry).unwrap().is_empty());
        let persisted = reopened
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|encoded| serde_json::from_str::<crate::MediaPushRequest>(&encoded).ok())
            .find(|item| item.id == request.id)
            .unwrap();
        assert_eq!(persisted.status, "done");
        assert_eq!(persisted.message, "sent");
        assert_eq!(persisted.location, "http://192.168.1.8/media/retry");
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn file_store_restores_pending_media_push_after_core_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-media-push-restart-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let request = crate::MediaPushRequest {
            id: "push-restart".into(),
            push_kind: "tvbox".into(),
            url: "https://example.test/media.mp4".into(),
            title: "Restart media".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 1,
        };
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.handle(CoreCommand::RequestMediaPush {
                request: request.clone(),
            })
            .unwrap();
        }
        let mut reopened = PersistentCore::open(&path).unwrap();
        let events = reopened
            .handle(CoreCommand::ResolveMediaPush {
                request_id: request.id.clone(),
                status: "done".into(),
                message: "ok".into(),
                location: "http://127.0.0.1:9978".into(),
            })
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event.event,
            crate::CoreEvent::MediaPushResolved { ref request } if request.id == "push-restart" && request.status == "done"
        )));
        // 见 `file_store_restores_tasks_after_core_restart`：先释放 owner 再删除。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn restart_normalizes_legacy_overlapping_media_pushes() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-media-push-normalize-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = crate::MediaPushRequest {
            id: "push-legacy-first".into(),
            push_kind: "cast".into(),
            url: "https://example.test/legacy-first.mp4".into(),
            title: "Legacy first".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 1,
        };
        let second = crate::MediaPushRequest {
            id: "push-legacy-second".into(),
            push_kind: "tvbox".into(),
            url: "https://example.test/legacy-second.mp4".into(),
            title: "Legacy second".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 2,
        };
        {
            let mut core = PersistentCore::open(&path).unwrap();
            for request in [&first, &second] {
                let encoded = serde_json::to_string(request).unwrap();
                core.store_mut()
                    .save_handoff(
                        &request.id,
                        &encoded,
                        &request.status,
                        None,
                        request.created_at_ms,
                    )
                    .unwrap();
            }
        }

        let mut reopened = PersistentCore::open(&path).unwrap();
        let rows: Vec<crate::MediaPushRequest> = reopened
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .filter_map(|row| serde_json::from_str(&row).ok())
            .collect();
        assert!(rows.iter().any(|request| {
            request.id == first.id && request.status == "pending" && request.message.is_empty()
        }));
        assert!(rows.iter().any(|request| {
            request.id == second.id
                && request.status == "failed"
                && request.message.contains("更早的投送请求")
        }));

        let resolved = reopened
            .handle(CoreCommand::ResolveMediaPush {
                request_id: first.id.clone(),
                status: "done".into(),
                message: "ok".into(),
                location: String::new(),
            })
            .unwrap();
        assert!(resolved.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushResolved { request }
                if request.id == first.id && request.status == "done"
        )));

        let next_request = crate::MediaPushRequest {
            id: "push-after-normalize".into(),
            push_kind: "cast".into(),
            url: "https://example.test/after-normalize.mp4".into(),
            title: "After normalize".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 3,
        };
        let admitted = reopened
            .handle(CoreCommand::RequestMediaPush {
                request: next_request.clone(),
            })
            .unwrap();
        assert!(admitted.iter().any(|event| matches!(
            &event.event,
            crate::CoreEvent::MediaPushRequested { request } if request.id == next_request.id
        )));

        // 见 `file_store_restores_tasks_after_core_restart`：先释放 owner 再删除。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn side_rows_and_checkpoint_commit_and_restore_together() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-side-row-transaction-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let handoff_id = "handoff-transaction";
        let push = crate::MediaPushRequest {
            id: "push-transaction".into(),
            push_kind: "tvbox".into(),
            url: "https://example.test/media.mp4".into(),
            title: "Media".into(),
            status: "pending".into(),
            message: String::new(),
            location: String::new(),
            created_at_ms: 2,
        };
        {
            let mut core = PersistentCore::open(&path).unwrap();
            core.store_mut()
                .save_handoff(
                    handoff_id,
                    &serde_json::json!({
                        "id": handoff_id,
                        "status": "pending",
                        "created_at_ms": 1
                    })
                    .to_string(),
                    "pending",
                    None,
                    1,
                )
                .unwrap();
            core.emit(crate::CoreEvent::HandoffResolved {
                handoff_id: handoff_id.into(),
                task_id: None,
            })
            .unwrap();
            core.emit(crate::CoreEvent::MediaPushRequested {
                request: push.clone(),
            })
            .unwrap();
            assert_eq!(core.store().latest_sequence().unwrap(), 2);
        }

        let reopened = PersistentCore::open(&path).unwrap();
        assert_eq!(reopened.latest_sequence(), 2);
        assert!(reopened.pending_handoff(handoff_id).is_none());
        let rows: Vec<serde_json::Value> = reopened
            .store()
            .load_handoffs()
            .unwrap()
            .into_iter()
            .map(|row| serde_json::from_str(&row).unwrap())
            .collect();
        assert!(rows.iter().any(|row| {
            row.get("id").and_then(serde_json::Value::as_str) == Some(handoff_id)
                && row.get("status").and_then(serde_json::Value::as_str) == Some("rejected")
        }));
        assert!(rows.iter().any(|row| {
            row.get("id").and_then(serde_json::Value::as_str) == Some(push.id.as_str())
                && row.get("status").and_then(serde_json::Value::as_str) == Some("pending")
        }));
        // 见 `file_store_restores_tasks_after_core_restart`：先释放 owner 再删除。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn verification_and_mirror_results_survive_core_restart() {
        let path = std::env::temp_dir().join(format!(
            "hls-v7-verification-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut core = PersistentCore::open(&path).unwrap();
            let mut spec = test_spec();
            spec.checksum = Some("sha256:abc".into());
            spec.mirrors = vec!["https://mirror.example.test/restart.bin".into()];
            core.handle(CoreCommand::CreateTask { spec }).unwrap();
            core.set_checksum_result("task-1", "SHA-256".into(), "def".into(), false)
                .unwrap();
            core.set_av_scan_result(
                "task-1",
                AvScanStatus {
                    state: "clean".into(),
                    engine: "Windows Defender".into(),
                    detail: "scan completed".into(),
                },
            )
            .unwrap();
            core.set_mirror_result(
                "task-1",
                vec![MirrorStatus {
                    url: "https://mirror.example.test/restart.bin".into(),
                    final_url: "https://cdn.example.test/restart.bin".into(),
                    state: "active".into(),
                    detail: "下载已使用此地址".into(),
                    ranges: true,
                }],
            )
            .unwrap();
            core.set_torrent_telemetry("task-1", 18, 4, 0, 0).unwrap();
        }
        let reopened = PersistentCore::open(&path).unwrap();
        let task = &reopened.tasks()[0];
        assert_eq!(task.checksum_algorithm, "SHA-256");
        assert_eq!(task.checksum_actual, "def");
        assert_eq!(task.checksum_verified, Some(false));
        assert_eq!(task.av_scan.as_ref().unwrap().state, "clean");
        assert_eq!(task.mirror_status[0].state, "active");
        assert!(task.mirror_status[0].ranges);
        assert_eq!((task.peer_count, task.seed_count), (18, 4));
        assert_eq!(
            (task.uploaded_bytes, task.upload_speed_bytes_per_sec),
            (0, 0)
        );
        // 见 `file_store_restores_tasks_after_core_restart`：先释放 owner 再删除。
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    fn test_spec() -> TaskSpec {
        TaskSpec {
            url: "https://example.test/restart.bin".into(),
            resource_kind: ResourceKind::File,
            title: "Restart".into(),
            filename: "restart.bin".into(),
            download_dir: String::new(),
            request_method: "GET".into(),
            credential_ref: None,
            replay_context_ref: None,
            concurrency: 4,
            checksum: None,
            expected_size: None,
            etag: String::new(),
            last_modified: String::new(),
            ..Default::default()
        }
    }
}
