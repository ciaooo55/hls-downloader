//! Versioned Rust-side contract for the Python-free v6 core.
//!
//! The browser extension, resident shell, native UI and protocol workers all
//! exchange these semantic messages.  Secrets stay behind `credential_ref`;
//! this module intentionally carries only bounded metadata and state.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const V6_PROTOCOL_NAME: &str = "hls-downloader-v6-core";
pub const V6_PROTOCOL_VERSION: u32 = 1;
pub const LEGAL_TERMS_VERSION: &str = "2026-08-06-cn-1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    #[default]
    File,
    Hls,
    Dash,
    Live,
    Ftp,
    Sftp,
    Torrent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceOffer {
    pub url: String,
    pub resource_kind: ResourceKind,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub source_page_url: String,
    #[serde(default)]
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub replay_context_ref: Option<String>,
    #[serde(default = "default_method")]
    pub request_method: String,
    #[serde(default)]
    pub handoff_id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub size: u64,
}

impl Default for ResourceOffer {
    fn default() -> Self {
        Self {
            url: String::new(),
            resource_kind: ResourceKind::File,
            owner: String::new(),
            evidence: Vec::new(),
            confidence: 0.0,
            source_page_url: String::new(),
            credential_ref: None,
            replay_context_ref: None,
            request_method: default_method(),
            handoff_id: String::new(),
            filename: String::new(),
            title: String::new(),
            size: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub resource_kind: ResourceKind,
    pub status: String,
    pub stage: String,
    pub title: String,
    pub filename: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: Option<u64>,
    pub active_workers: u32,
    pub completed_ranges: u64,
    pub total_ranges: u64,
    pub playback_ready: bool,
    pub is_live: bool,
    pub available_actions: Vec<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub queue_index: i64,
    #[serde(default)]
    pub output_missing: bool,
    #[serde(default)]
    pub connection_hint: String,
    #[serde(default)]
    pub connection_parts: Vec<ConnectionPart>,
    #[serde(default)]
    pub log_tail: Vec<String>,
    #[serde(default)]
    pub speed_history: Vec<u64>,
    #[serde(default)]
    pub mirror_status: String,
}

impl Default for TaskSnapshot {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            resource_kind: ResourceKind::File,
            status: String::new(),
            stage: String::new(),
            title: String::new(),
            filename: String::new(),
            downloaded_bytes: 0,
            total_bytes: None,
            speed_bytes_per_sec: 0,
            eta_seconds: None,
            active_workers: 0,
            completed_ranges: 0,
            total_ranges: 0,
            playback_ready: false,
            is_live: false,
            available_actions: Vec::new(),
            url: String::new(),
            error_code: None,
            error_message: None,
            queue_index: 0,
            output_missing: false,
            connection_hint: String::new(),
            connection_parts: Vec::new(),
            log_tail: Vec::new(),
            speed_history: Vec::new(),
            mirror_status: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ConnectionPart {
    #[serde(default)]
    pub start: u64,
    #[serde(default)]
    pub end: u64,
    #[serde(default)]
    pub done: u64,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HarvestCandidate {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub extension: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub url: String,
    pub resource_kind: ResourceKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub download_dir: String,
    #[serde(default = "default_method")]
    pub request_method: String,
    #[serde(default)]
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub replay_context_ref: Option<String>,
    #[serde(default)]
    pub concurrency: u32,
    #[serde(default)]
    pub checksum: Option<String>,
    #[serde(default)]
    pub expected_size: Option<u64>,
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub last_modified: String,
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub proxy: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub speed_limit_kib: u32,
    #[serde(default)]
    pub body_path: String,
    #[serde(default)]
    pub harvest: bool,
    #[serde(default)]
    pub preferred_bandwidth: u64,
    #[serde(default)]
    pub preferred_height: u32,
    #[serde(default)]
    pub preferred_audio: String,
    #[serde(default)]
    pub allow_duplicate: bool,
    #[serde(default)]
    pub scheduled_start_at: String,
    #[serde(default)]
    pub scheduled_stop_at: String,
    #[serde(default)]
    pub completion_action: String,
}

impl Default for TaskSpec {
    fn default() -> Self {
        Self {
            url: String::new(),
            resource_kind: ResourceKind::File,
            title: String::new(),
            filename: String::new(),
            download_dir: String::new(),
            request_method: default_method(),
            credential_ref: None,
            replay_context_ref: None,
            concurrency: 0,
            checksum: None,
            expected_size: None,
            etag: String::new(),
            last_modified: String::new(),
            mirrors: Vec::new(),
            proxy: String::new(),
            headers: std::collections::BTreeMap::new(),
            speed_limit_kib: 0,
            body_path: String::new(),
            harvest: false,
            preferred_bandwidth: 0,
            preferred_height: 0,
            preferred_audio: String::new(),
            allow_duplicate: false,
            scheduled_start_at: String::new(),
            scheduled_stop_at: String::new(),
            completion_action: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamVariant {
    pub label: String,
    pub bandwidth: u64,
    pub height: u32,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastDeviceInfo {
    pub id: String,
    pub label: String,
    pub location: String,
    pub control_url: String,
    pub service_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoreCommand {
    CreateTask {
        spec: TaskSpec,
    },
    TaskAction {
        task_id: String,
        action: String,
    },
    UpdateProgress {
        task_id: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        speed_bytes_per_sec: u64,
        stage: String,
        status: String,
    },
    OfferResource {
        offer: ResourceOffer,
    },
    OpenMain,
    HideMain,
    Shutdown,
    Ping,
    SetSetting {
        key: String,
        value: Value,
    },
    AcceptHandoff {
        handoff_id: String,
        filename: String,
        download_dir: String,
    },
    RejectHandoff {
        handoff_id: String,
    },
    PresentHandoff {
        handoff_id: String,
        ok: bool,
    },
    PlayTask {
        task_id: String,
    },
    CastTask {
        task_id: String,
    },
    PlayerControl {
        action: String,
    },
    ReorderQueue {
        task_id: String,
        delta: i32,
    },
    CheckUpdate,
    DownloadUpdate,
    ProbeUrl {
        url: String,
    },
    DiscoverCastDevices,
    CastToDevice {
        task_id: String,
        device_id: String,
    },
    OpenCompleted {
        task_id: String,
        folder: bool,
    },
    CancelPowerAction,
    ClearCompleted,
    SaveSiteProfile {
        task_id: String,
    },
    ImportPaths {
        paths: Vec<String>,
    },
    PlaceQueue {
        task_id: String,
        before_id: String,
    },
    HarvestPage {
        url: String,
    },
    GetTaskLog {
        task_id: String,
    },
    BrowserHello {
        version: String,
        browser: String,
    },
    ControlCast {
        action: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoreEvent {
    Ready {
        protocol: String,
        version: u32,
    },
    TaskCreated {
        snapshot: TaskSnapshot,
    },
    TaskUpdated {
        snapshot: TaskSnapshot,
    },
    TaskProgress {
        snapshot: TaskSnapshot,
    },
    TaskDeleted {
        task_id: String,
    },
    HandoffOffered {
        offer: ResourceOffer,
    },
    HandoffResolved {
        handoff_id: String,
        task_id: Option<String>,
    },
    UiShow {
        surface: String,
    },
    ProbeResult {
        url: String,
        resource_kind: ResourceKind,
        label: String,
        variants: Vec<StreamVariant>,
    },
    CastDevices {
        devices: Vec<CastDeviceInfo>,
    },
    Error {
        code: String,
        message: String,
    },
    DuplicateOffered {
        task_id: String,
        action: String,
        output_missing: bool,
        message: String,
    },
    Toast {
        level: String,
        message: String,
    },
    HarvestResult {
        url: String,
        links: Vec<HarvestCandidate>,
    },
    TaskLog {
        task_id: String,
        lines: Vec<String>,
    },
    BrowserStatus {
        connected: bool,
        version: String,
        browser: String,
        message: String,
    },
    CastSession {
        active: bool,
        title: String,
        device: String,
        status: String,
    },
}

impl CoreEvent {
    pub fn sequence_key(&self) -> Option<&str> {
        match self {
            Self::TaskCreated { snapshot }
            | Self::TaskUpdated { snapshot }
            | Self::TaskProgress { snapshot } => Some(snapshot.task_id.as_str()),
            Self::TaskDeleted { task_id } => Some(task_id.as_str()),
            Self::HandoffResolved { handoff_id, .. } => Some(handoff_id.as_str()),
            Self::HandoffOffered { offer } => Some(offer.owner.as_str()),
            Self::DuplicateOffered { task_id, .. } => Some(task_id.as_str()),
            Self::TaskLog { task_id, .. } => Some(task_id.as_str()),
            Self::Ready { .. }
            | Self::Error { .. }
            | Self::UiShow { .. }
            | Self::ProbeResult { .. }
            | Self::CastDevices { .. }
            | Self::Toast { .. }
            | Self::HarvestResult { .. }
            | Self::BrowserStatus { .. }
            | Self::CastSession { .. } => None,
        }
    }

    /// Convert a high-frequency event into a compact JSON payload for the UI.
    /// This deliberately avoids passing request headers, cookies or bodies.
    pub fn ui_payload(&self) -> Value {
        serde_json::to_value(self).expect("core event is serializable")
    }
}

fn default_method() -> String {
    "GET".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_roundtrip_keeps_bounded_metadata() {
        let offer = ResourceOffer {
            url: "https://cdn.example/video.m3u8".into(),
            resource_kind: ResourceKind::Hls,
            owner: "media-element:2".into(),
            evidence: vec!["manifest_mime".into(), "current_src".into()],
            confidence: 0.98,
            source_page_url: "https://example.test/watch".into(),
            credential_ref: Some("cred-1".into()),
            replay_context_ref: Some("replay-1".into()),
            request_method: "GET".into(),
            handoff_id: "handoff-1".into(),
            filename: "video.m3u8".into(),
            title: "Video".into(),
            size: 42,
        };
        let encoded = serde_json::to_vec(&offer).unwrap();
        let restored: ResourceOffer = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored, offer);
    }

    #[test]
    fn event_payload_does_not_invent_secret_fields() {
        let event = CoreEvent::TaskProgress {
            snapshot: TaskSnapshot {
                task_id: "task-1".into(),
                resource_kind: ResourceKind::File,
                status: "downloading".into(),
                stage: "transfer".into(),
                title: "demo".into(),
                filename: "demo.bin".into(),
                downloaded_bytes: 4,
                total_bytes: Some(8),
                speed_bytes_per_sec: 4,
                eta_seconds: Some(1),
                active_workers: 1,
                completed_ranges: 1,
                total_ranges: 2,
                playback_ready: false,
                is_live: false,
                available_actions: vec!["pause".into()],
                url: String::new(),
                error_code: None,
                error_message: None,
                queue_index: 0,
                output_missing: false,
                connection_hint: String::new(),
                connection_parts: Vec::new(),
                log_tail: Vec::new(),
                speed_history: Vec::new(),
                mirror_status: String::new(),
            },
        };
        let payload = event.ui_payload();
        assert!(payload.get("cookie").is_none());
        assert!(payload.get("authorization").is_none());
        assert_eq!(event.sequence_key(), Some("task-1"));
    }

    #[test]
    fn default_method_is_get() {
        let spec: TaskSpec = serde_json::from_value(serde_json::json!({
            "url": "https://example.test/file.bin",
            "resource_kind": "file"
        }))
        .unwrap();
        assert_eq!(spec.request_method, "GET");
        assert!(spec.scheduled_start_at.is_empty());
        assert!(spec.completion_action.is_empty());
    }

    #[test]
    fn check_update_command_roundtrips() {
        let encoded = serde_json::to_value(CoreCommand::CheckUpdate).unwrap();
        assert_eq!(encoded["kind"], "check_update");
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored, CoreCommand::CheckUpdate);
    }

    #[test]
    fn present_handoff_command_roundtrips() {
        let encoded = serde_json::to_value(CoreCommand::PresentHandoff {
            handoff_id: "handoff-1".into(),
            ok: false,
        })
        .unwrap();
        assert_eq!(encoded["kind"], "present_handoff");
        assert_eq!(encoded["ok"], false);
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            restored,
            CoreCommand::PresentHandoff {
                handoff_id: "handoff-1".into(),
                ok: false,
            }
        );
    }

    #[test]
    fn probe_and_cast_commands_roundtrip() {
        let encoded = serde_json::to_value(CoreCommand::ProbeUrl {
            url: "https://cdn.test/a.m3u8".into(),
        })
        .unwrap();
        assert_eq!(encoded["kind"], "probe_url");
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert!(matches!(restored, CoreCommand::ProbeUrl { .. }));
        let devices = serde_json::to_value(CoreEvent::CastDevices {
            devices: vec![CastDeviceInfo {
                id: "dlna:http://192.168.1.8/ctrl".into(),
                label: "TV".into(),
                location: "http://192.168.1.8/desc".into(),
                control_url: "http://192.168.1.8/ctrl".into(),
                service_type: "urn:schemas-upnp-org:service:AVTransport:1".into(),
            }],
        })
        .unwrap();
        assert_eq!(devices["kind"], "cast_devices");
    }
}
