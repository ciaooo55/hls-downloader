//! Durable command boundary shared by the UI and protocol front-ends.

use crate::{
    AvScanStatus, CoreCommand, CoreRuntime, CoreStore, EventEnvelope, MirrorStatus, TaskFailure,
    TaskSnapshot, TaskSpec,
};
use std::path::Path;

pub struct PersistentCore {
    runtime: CoreRuntime,
    store: CoreStore,
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
        Ok(events)
    }

    pub fn emit(&mut self, event: crate::CoreEvent) -> Result<Vec<EventEnvelope>, String> {
        let before = self.runtime.clone();
        let events = self.runtime.emit(event);
        if let Err(error) = self.store.apply_events_and_spec(&events, None) {
            self.runtime = before;
            return Err(error);
        }
        Ok(events)
    }

    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.runtime.list_tasks()
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

    pub fn events_after(&self, sequence: u64, limit: usize) -> Vec<EventEnvelope> {
        self.runtime.events_after(sequence, limit)
    }

    pub fn store(&self) -> &CoreStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut CoreStore {
        &mut self.store
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
        Ok(events)
    }

    fn from_store(store: CoreStore) -> Result<Self, String> {
        let snapshots = store.load_tasks()?;
        let specs = store.load_task_specs()?;
        let sequence = store.latest_sequence()?;
        let mut runtime = CoreRuntime::from_state(snapshots, specs, sequence);
        for encoded in store.load_handoffs()? {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&encoded) {
                if let Ok(request) =
                    serde_json::from_value::<crate::MediaPushRequest>(value.clone())
                {
                    runtime.restore_pending_media_push(request);
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
        Ok(Self { runtime, store })
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CoreCommand, ResourceKind, TaskSpec};

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
