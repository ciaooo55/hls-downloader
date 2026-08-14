//! Pre-created confirm / progress / complete / main surfaces.
//!
//! Windows are created once at boot, then shown and hidden. An offer never
//! creates a window and never fetches the handoff again.

use crate::protocol::{paint_snapshot, PROTOCOL_NAME, PROTOCOL_VERSION};
use crate::task_list::{FileCategory, StatusFilter, TaskList, TaskRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Instant;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub title: String,
    pub mime_type: String,
    pub size: i64,
    pub resource_kind: String,
    pub status: String,
    pub download_dir: String,
}

impl Snapshot {
    pub fn from_offer(value: &Value) -> Self {
        let painted = paint_snapshot(value);
        Self {
            id: painted["id"].as_str().unwrap_or("").to_string(),
            url: painted["url"].as_str().unwrap_or("").to_string(),
            filename: painted["filename"].as_str().unwrap_or("").to_string(),
            title: painted["title"].as_str().unwrap_or("").to_string(),
            mime_type: painted["mime_type"].as_str().unwrap_or("").to_string(),
            size: painted["size"].as_i64().unwrap_or(0),
            resource_kind: painted["resource_kind"]
                .as_str()
                .unwrap_or("file")
                .to_string(),
            status: painted["status"].as_str().unwrap_or("pending").to_string(),
            download_dir: painted["download_dir"].as_str().unwrap_or("").to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverlayWindow {
    pub created: bool,
    pub visible: bool,
    pub focusable: bool,
}

impl OverlayWindow {
    fn hidden(focusable: bool) -> Self {
        Self {
            created: true,
            visible: false,
            focusable,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResidentShell {
    pub protocol: String,
    pub version: u32,
    pub resident: bool,
    pub tray: bool,
    pub core_running: bool,
    pub main_open: bool,
    pub backend: String,
    pub windows: Windows,
    pub snapshot: Option<Snapshot>,
    pub progress_tasks: Vec<Value>,
    pub complete_item: Option<Value>,
    pub task_list: TaskList,
    pub last_show_ms: f64,
    pub created_at_boot: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Windows {
    pub handoff: OverlayWindow,
    pub progress: OverlayWindow,
    pub complete: OverlayWindow,
    pub main: OverlayWindow,
}

impl Default for ResidentShell {
    fn default() -> Self {
        Self {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            resident: false,
            tray: false,
            core_running: false,
            main_open: false,
            backend: "headless".to_string(),
            windows: Windows {
                handoff: OverlayWindow {
                    created: false,
                    visible: false,
                    focusable: true,
                },
                progress: OverlayWindow {
                    created: false,
                    visible: false,
                    focusable: false,
                },
                complete: OverlayWindow {
                    created: false,
                    visible: false,
                    focusable: true,
                },
                main: OverlayWindow {
                    created: false,
                    visible: false,
                    focusable: true,
                },
            },
            snapshot: None,
            progress_tasks: Vec::new(),
            complete_item: None,
            task_list: TaskList::default(),
            last_show_ms: 0.0,
            created_at_boot: false,
        }
    }
}

impl ResidentShell {
    pub fn boot(backend: &str) -> Self {
        Self {
            resident: true,
            tray: true,
            backend: backend.to_string(),
            windows: Windows {
                handoff: OverlayWindow::hidden(true),
                progress: OverlayWindow::hidden(false),
                complete: OverlayWindow::hidden(true),
                main: OverlayWindow::hidden(true),
            },
            created_at_boot: true,
            ..Self::default()
        }
        .with_backend(backend)
    }

    fn with_backend(mut self, backend: &str) -> Self {
        self.backend = backend.to_string();
        self.resident = true;
        self.tray = true;
        self.created_at_boot = true;
        self
    }

    pub fn is_ready(&self) -> bool {
        self.resident && self.windows.handoff.created && self.tray
    }

    pub fn offer(&mut self, handoff: &Value) -> Result<&Snapshot, String> {
        if !self.is_ready() {
            return Err("桌面界面尚未就绪".into());
        }
        if !self.windows.handoff.created {
            return Err("confirmation window was not pre-created".into());
        }
        let started = Instant::now();
        let snapshot = Snapshot::from_offer(handoff);
        if snapshot.id.is_empty() {
            return Err("handoff snapshot missing id".into());
        }
        self.core_running = true;
        self.snapshot = Some(snapshot);
        self.windows.handoff.visible = true;
        self.last_show_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(self.snapshot.as_ref().unwrap())
    }

    pub fn hide_handoff(&mut self) {
        self.windows.handoff.visible = false;
    }

    pub fn reject(&mut self) {
        self.hide_handoff();
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.status = "rejected".into();
        }
    }

    pub fn accept(&mut self) {
        self.hide_handoff();
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.status = "accepted".into();
        }
    }

    pub fn progress(&mut self, tasks: Vec<Value>) -> Result<(), String> {
        if !self.resident || !self.windows.progress.created {
            return Err("桌面界面尚未就绪".into());
        }
        self.progress_tasks = tasks;
        self.windows.progress.visible = !self.progress_tasks.is_empty();
        Ok(())
    }

    pub fn complete(&mut self, item: Value) -> Result<(), String> {
        if !self.resident || !self.windows.complete.created {
            return Err("桌面界面尚未就绪".into());
        }
        self.complete_item = Some(item);
        self.windows.complete.visible = true;
        Ok(())
    }

    pub fn open_main(&mut self) -> Result<(), String> {
        if !self.resident || !self.windows.main.created {
            return Err("桌面界面尚未就绪".into());
        }
        self.main_open = true;
        self.windows.main.visible = true;
        self.task_list.needs_refresh = true;
        Ok(())
    }

    pub fn hide_main(&mut self) {
        self.main_open = false;
        self.windows.main.visible = false;
    }

    pub fn replace_tasks(&mut self, items: Vec<Value>) {
        self.task_list.replace(items);
    }

    pub fn visible_tasks(&self) -> Vec<&TaskRow> {
        self.task_list.visible()
    }

    pub fn selected_task(&self) -> Option<&TaskRow> {
        self.task_list.selected()
    }

    pub fn set_status_filter(&mut self, filter: StatusFilter) {
        self.task_list.set_status_filter(filter);
    }

    pub fn set_category(&mut self, category: FileCategory) {
        self.task_list.set_category(category);
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.task_list.set_query(query);
    }

    pub fn shutdown(&mut self) {
        self.resident = false;
        self.tray = false;
        self.core_running = false;
        self.main_open = false;
        self.windows.handoff.visible = false;
        self.windows.progress.visible = false;
        self.windows.complete.visible = false;
        self.windows.main.visible = false;
    }

    pub fn apply_event(&mut self, event: &Value) -> Result<String, String> {
        let kind = event.get("kind").and_then(Value::as_str).unwrap_or("");
        match kind {
            "handoff" => {
                let snapshot = event
                    .get("snapshot")
                    .cloned()
                    .unwrap_or_else(|| event.clone());
                self.offer(&snapshot)?;
                Ok("handoff".into())
            }
            "progress" => {
                let tasks = event
                    .get("tasks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.task_list.upsert(&tasks);
                self.progress(tasks)?;
                Ok("progress".into())
            }
            "complete" => {
                let item = event
                    .get("item")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                self.task_list.upsert(std::slice::from_ref(&item));
                self.complete(item)?;
                Ok("complete".into())
            }
            "tasks" => {
                let tasks = event
                    .get("tasks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.replace_tasks(tasks);
                Ok("tasks".into())
            }
            "open_main" => {
                self.open_main()?;
                Ok("open_main".into())
            }
            "hide_main" => {
                self.hide_main();
                Ok("hide_main".into())
            }
            "shutdown" => {
                self.shutdown();
                Ok("shutdown".into())
            }
            "http_job" => Ok("http_job".into()),
            _ => Ok(kind.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn boot_precreates_hidden_overlays_and_tray() {
        let shell = ResidentShell::boot("headless");
        assert!(shell.resident);
        assert!(shell.tray);
        assert!(!shell.main_open);
        assert!(shell.windows.handoff.created && !shell.windows.handoff.visible);
        assert!(shell.windows.progress.created && !shell.windows.progress.focusable);
        assert!(shell.windows.complete.created && !shell.windows.complete.visible);
        assert!(shell.windows.main.created && !shell.windows.main.visible);
        assert!(shell.created_at_boot);
    }

    #[test]
    fn offer_before_boot_fails() {
        let mut shell = ResidentShell::default();
        let err = shell
            .offer(&json!({"id": "h1", "filename": "a.exe"}))
            .unwrap_err();
        assert!(err.contains("尚未就绪"));
    }

    #[test]
    fn offer_paints_snapshot_on_existing_window_without_recreate() {
        let mut shell = ResidentShell::boot("headless");
        assert!(shell.windows.handoff.created);
        let snapshot = shell
            .offer(&json!({
                "id": "h1",
                "filename": "setup.exe",
                "url": "https://cdn.test/setup.exe",
                "size": 4096,
                "cookie": "secret"
            }))
            .unwrap()
            .clone();
        assert_eq!(snapshot.filename, "setup.exe");
        assert_eq!(snapshot.size, 4096);
        assert!(shell.windows.handoff.created);
        assert!(shell.windows.handoff.visible);
        assert!(shell.last_show_ms < 20.0);
        assert!(shell.snapshot.as_ref().unwrap().filename == "setup.exe");
    }

    #[test]
    fn hide_main_keeps_resident_tray_and_warm_windows() {
        let mut shell = ResidentShell::boot("win32");
        shell.open_main().unwrap();
        assert!(shell.task_list.needs_refresh);
        shell.hide_main();
        assert!(shell.resident && shell.tray);
        assert!(!shell.main_open);
        assert!(shell.windows.handoff.created);
        assert!(shell.is_ready());
    }

    #[test]
    fn open_main_event_shows_filtered_native_list() {
        let mut shell = ResidentShell::boot("headless");
        shell.apply_event(&json!({"kind": "open_main"})).unwrap();
        assert!(shell.main_open && shell.windows.main.visible);
        shell
            .apply_event(&json!({
                "kind": "tasks",
                "tasks": [
                    {"id": "1", "filename": "film.mp4", "status": "downloading_segments", "progress_percent": 12},
                    {"id": "2", "filename": "pack.zip", "status": "done"}
                ]
            }))
            .unwrap();
        shell.set_status_filter(StatusFilter::Unfinished);
        let visible = shell.visible_tasks();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].filename, "film.mp4");
        shell.apply_event(&json!({"kind": "hide_main"})).unwrap();
        assert!(!shell.main_open);
        assert!(shell.resident && shell.tray);
        assert_eq!(shell.task_list.tasks.len(), 2);
    }

    #[test]
    fn http_job_event_does_not_show_overlays() {
        let mut shell = ResidentShell::boot("headless");
        let kind = shell
            .apply_event(&json!({
                "kind": "http_job",
                "job_path": "C:/tmp/native-engine.job.json",
                "presentable": false
            }))
            .unwrap();
        assert_eq!(kind, "http_job");
        assert!(!shell.windows.handoff.visible);
        assert!(!shell.windows.progress.visible);
        assert!(!shell.windows.complete.visible);
        assert!(!shell.main_open);
    }

    #[test]
    fn reject_hides_confirm_and_progress_uses_warm_window() {
        let mut shell = ResidentShell::boot("headless");
        shell
            .offer(&json!({"id": "h1", "filename": "a.bin"}))
            .unwrap();
        shell.reject();
        assert!(!shell.windows.handoff.visible);
        shell
            .progress(vec![json!({"id": "t1", "percent": 40})])
            .unwrap();
        shell
            .complete(json!({"id": "t1", "filename": "a.bin"}))
            .unwrap();
        assert!(shell.windows.progress.visible);
        assert!(shell.windows.complete.visible);
        assert!(shell.windows.progress.created && shell.windows.complete.created);
        shell.progress(vec![]).unwrap();
        assert!(!shell.windows.progress.visible);
    }
}
