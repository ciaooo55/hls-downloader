//! Durable command boundary shared by the UI and protocol front-ends.

use crate::{CoreCommand, CoreRuntime, CoreStore, EventEnvelope, TaskSnapshot, TaskSpec};
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
        if let Err(error) = self.sync_handoff_rows(&events) {
            self.runtime = before;
            return Err(error);
        }
        if let Err(error) = self.sync_media_push_rows(&events) {
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
        if let Err(error) = self.sync_handoff_rows(&events) {
            self.runtime = before;
            return Err(error);
        }
        if let Err(error) = self.sync_media_push_rows(&events) {
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

    pub fn task_spec(&self, task_id: &str) -> Option<&TaskSpec> {
        self.runtime.task_spec(task_id)
    }

    pub fn replace_spec(&mut self, task_id: &str, spec: TaskSpec) -> Result<(), String> {
        self.runtime.replace_spec(task_id, spec.clone());
        self.store.save_spec(task_id, &spec)
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

    fn from_store(store: CoreStore) -> Result<Self, String> {
        let snapshots = store.load_tasks()?;
        let specs = store.load_task_specs()?;
        let sequence = store.latest_sequence()?;
        let mut runtime = CoreRuntime::from_state(snapshots, specs, sequence);
        for encoded in store.load_handoffs()? {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&encoded) {
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

    fn sync_handoff_rows(&mut self, events: &[EventEnvelope]) -> Result<(), String> {
        for envelope in events {
            let crate::CoreEvent::HandoffResolved {
                handoff_id,
                task_id,
            } = &envelope.event
            else {
                continue;
            };
            let status = if task_id.is_some() {
                "accepted"
            } else {
                "rejected"
            };
            let mut patched = false;
            for encoded in self.store.load_handoffs()? {
                let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&encoded) else {
                    continue;
                };
                if value.get("id").and_then(serde_json::Value::as_str) != Some(handoff_id.as_str())
                {
                    continue;
                }
                if let Some(object) = value.as_object_mut() {
                    object.insert("status".into(), serde_json::Value::String(status.into()));
                    object.insert(
                        "task_id".into(),
                        task_id
                            .clone()
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    );
                }
                let json = serde_json::to_string(&value)
                    .map_err(|error| format!("encode resolved handoff {handoff_id}: {error}"))?;
                let created = value
                    .get("created_at_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                self.store
                    .save_handoff(handoff_id, &json, status, task_id.as_deref(), created)?;
                patched = true;
                break;
            }
            let _ = patched;
            if !patched {
                let json = serde_json::json!({
                    "id": handoff_id,
                    "status": status,
                    "task_id": task_id,
                    "created_at_ms": 0
                })
                .to_string();
                self.store
                    .save_handoff(handoff_id, &json, status, task_id.as_deref(), 0)?;
            }
        }
        Ok(())
    }

    fn sync_media_push_rows(&mut self, events: &[EventEnvelope]) -> Result<(), String> {
        for envelope in events {
            let request = match &envelope.event {
                crate::CoreEvent::MediaPushRequested { request }
                | crate::CoreEvent::MediaPushResolved { request } => request,
                _ => continue,
            };
            let json = serde_json::to_string(request)
                .map_err(|error| format!("encode media push {}: {error}", request.id))?;
            self.store.save_handoff(
                &request.id,
                &json,
                &request.status,
                None,
                request.created_at_ms,
            )?;
        }
        Ok(())
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
