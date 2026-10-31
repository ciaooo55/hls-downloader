//! Versioned Rust-side contract for the HLS Downloader Core.
//!
//! The browser extension, resident Core, native presenter and protocol workers all
//! exchange these semantic messages.  Secrets stay behind `credential_ref`;
//! this module intentionally carries only bounded metadata and state.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Active v7 product contract.
pub const V7_PROTOCOL_NAME: &str = "hls-downloader-v7-core";
pub const V7_PROTOCOL_VERSION: u32 = 1;
pub const LEGAL_TERMS_VERSION: &str = "2026-08-06-cn-1";
pub const DEFAULT_QUEUE_ID: &str = "default";

/// Sequenced event shared by the resident Core and IPC-only clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub sequence: u64,
    pub event: CoreEvent,
}

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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AvScanStatus {
    pub state: String,
    pub engine: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MirrorStatus {
    pub url: String,
    #[serde(default)]
    pub final_url: String,
    pub state: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub ranges: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceOffer {
    pub url: String,
    pub resource_kind: ResourceKind,
    #[serde(default)]
    pub local_source_path: Option<String>,
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
    pub mime_type: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub preferred_bandwidth: u64,
    #[serde(default)]
    pub preferred_height: u32,
}

impl Default for ResourceOffer {
    fn default() -> Self {
        Self {
            url: String::new(),
            resource_kind: ResourceKind::File,
            local_source_path: None,
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
            mime_type: String::new(),
            size: 0,
            preferred_bandwidth: 0,
            preferred_height: 0,
        }
    }
}

/// Durable browser offer metadata shared by admission and Native Messaging.
#[cfg(feature = "full-core")]
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct BrowserHandoff {
    pub id: String,
    pub offer: ResourceOffer,
    pub filename: String,
    pub title: String,
    pub mime_type: String,
    pub size: u64,
    pub status: String,
    pub presentation: String,
    pub task_id: Option<String>,
    pub created_at_ms: u64,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub suppression: Option<Value>,
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
    #[serde(default)]
    pub peer_count: u32,
    #[serde(default)]
    pub seed_count: u32,
    #[serde(default)]
    pub uploaded_bytes: u64,
    #[serde(default)]
    pub upload_speed_bytes_per_sec: u64,
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
    pub error_stage: String,
    #[serde(default)]
    pub error_url: String,
    #[serde(default)]
    pub error_hint: String,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub error_attempt: u32,
    #[serde(default)]
    pub queue_index: i64,
    #[serde(default = "default_queue_id")]
    pub queue_id: String,
    #[serde(default)]
    pub output_missing: bool,
    #[serde(default)]
    pub output_path: String,
    #[serde(default)]
    pub connection_hint: String,
    #[serde(default)]
    pub connection_parts: Vec<ConnectionPart>,
    #[serde(default)]
    pub log_tail: Vec<String>,
    #[serde(default)]
    pub speed_history: Vec<u64>,
    #[serde(default)]
    pub mirror_status: Vec<MirrorStatus>,
    #[serde(default = "default_method")]
    pub request_method: String,
    #[serde(default)]
    pub download_dir: String,
    #[serde(default)]
    pub speed_limit_kib: u32,
    #[serde(default)]
    pub expected_checksum: String,
    #[serde(default)]
    pub checksum_algorithm: String,
    #[serde(default)]
    pub checksum_actual: String,
    #[serde(default)]
    pub checksum_verified: Option<bool>,
    #[serde(default)]
    pub av_scan: Option<AvScanStatus>,
    #[serde(default)]
    pub max_workers: u32,
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub scheduled_start_at: String,
    #[serde(default)]
    pub scheduled_stop_at: String,
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
            peer_count: 0,
            seed_count: 0,
            uploaded_bytes: 0,
            upload_speed_bytes_per_sec: 0,
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
            error_stage: String::new(),
            error_url: String::new(),
            error_hint: String::new(),
            http_status: None,
            error_attempt: 0,
            queue_index: 0,
            queue_id: default_queue_id(),
            output_missing: false,
            output_path: String::new(),
            connection_hint: String::new(),
            connection_parts: Vec::new(),
            log_tail: Vec::new(),
            speed_history: Vec::new(),
            mirror_status: Vec::new(),
            request_method: default_method(),
            download_dir: String::new(),
            speed_limit_kib: 0,
            expected_checksum: String::new(),
            checksum_algorithm: String::new(),
            checksum_actual: String::new(),
            checksum_verified: None,
            av_scan: None,
            max_workers: 0,
            mirrors: Vec::new(),
            scheduled_start_at: String::new(),
            scheduled_stop_at: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TaskFailure {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub url: String,
    pub hint: String,
    pub http_status: Option<u16>,
    pub attempt: u32,
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
pub struct ZipEntryPreview {
    pub path: String,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub method: u16,
    pub encrypted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZipPreview {
    pub entries: Vec<ZipEntryPreview>,
    pub archive_bytes: u64,
    pub metadata_bytes_read: u64,
    pub zip64: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    #[serde(default)]
    pub origin_boundary: String,
    #[serde(default)]
    pub crawl_resource_key: String,
    /// Create without scheduling a worker; resume is a separate explicit action.
    #[serde(default)]
    pub start_paused: bool,
    pub url: String,
    pub resource_kind: ResourceKind,
    #[serde(default)]
    pub local_source_path: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub download_dir: String,
    #[serde(default)]
    pub work_dir: String,
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
    #[serde(default = "default_queue_id")]
    pub queue_id: String,
    #[serde(default)]
    pub torrent_selection: Vec<TorrentFileSelection>,
    #[serde(default)]
    pub torrent_piece_count: u64,
}

impl Default for TaskSpec {
    fn default() -> Self {
        Self {
            start_paused: false,
            crawl_resource_key: String::new(),
            origin_boundary: String::new(),
            url: String::new(),
            resource_kind: ResourceKind::File,
            local_source_path: None,
            title: String::new(),
            filename: String::new(),
            mime_type: String::new(),
            download_dir: String::new(),
            work_dir: String::new(),
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
            queue_id: default_queue_id(),
            torrent_selection: Vec::new(),
            torrent_piece_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueProfile {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_queue_profile_max")]
    pub max_active: u32,
    #[serde(default)]
    pub speed_limit_kib: u64,
    #[serde(default)]
    pub schedule_enabled: bool,
    #[serde(default = "default_queue_start")]
    pub start_time: String,
    #[serde(default = "default_queue_stop")]
    pub stop_time: String,
    #[serde(default = "default_queue_days")]
    pub active_days: String,
    #[serde(default)]
    pub completion_action: String,
}

impl Default for QueueProfile {
    fn default() -> Self {
        Self {
            id: default_queue_id(),
            name: "默认队列".into(),
            enabled: true,
            priority: 0,
            max_active: default_queue_profile_max(),
            speed_limit_kib: 0,
            schedule_enabled: false,
            start_time: default_queue_start(),
            stop_time: default_queue_stop(),
            active_days: default_queue_days(),
            completion_action: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFileEntry {
    pub index: u32,
    pub path: String,
    pub size: u64,
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFileSelection {
    pub index: u32,
    pub path: String,
    pub selected: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPushRequest {
    pub id: String,
    pub push_kind: String,
    pub url: String,
    pub title: String,
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub location: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoreCommand {
    CreateTask {
        spec: TaskSpec,
    },
    ImportCurl {
        command: String,
        #[serde(default)]
        options: TaskSpec,
    },
    /// 按 task_id 取单个任务的快照。UI 侧此前只有 CorePipeRequest::Snapshot 一条路读任务状态，
    /// 那一条在千任务库里要付 1.4MB（snapshot.tasks[] 每项约 1.5KB）。有了这一条，
    /// 任务操作成功后可以只重取变化的那一个任务，而不是整个列表。
    GetTask {
        task_id: String,
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
        #[serde(default)]
        trusted_ui: bool,
    },
    RejectHandoff {
        handoff_id: String,
        #[serde(default)]
        suppress_site_kind: bool,
    },
    PresentHandoff {
        handoff_id: String,
        ok: bool,
        #[serde(default)]
        presenter_id: String,
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
    CheckUpdate {
        silent: bool,
    },
    DownloadUpdate,
    InstallUpdate {
        workbench_pid: u32,
    },
    ProbeUrl {
        url: String,
        #[serde(default)]
        spec: Option<TaskSpec>,
    },
    RefreshTaskRequest {
        task_id: String,
        url: String,
        #[serde(default)]
        cookie: String,
        #[serde(default)]
        auto_resume: bool,
    },
    ProbeTorrent {
        source: String,
    },
    SelectTorrentFiles {
        source: String,
        selections: Vec<TorrentFileSelection>,
    },
    GetTaskTorrentFiles {
        task_id: String,
    },
    SetTaskTorrentFiles {
        task_id: String,
        selections: Vec<TorrentFileSelection>,
    },
    DiscoverCastDevices {
        mode: String,
    },
    CastToDevice {
        task_id: String,
        device_id: String,
    },
    ShareMedia {
        path: String,
        url: String,
        title: String,
        device_id: String,
    },
    RequestMediaPush {
        request: MediaPushRequest,
    },
    ResolveMediaPush {
        request_id: String,
        status: String,
        message: String,
        location: String,
    },
    OpenCompleted {
        task_id: String,
        folder: bool,
    },
    ConfirmPowerAction,
    CancelPowerAction,
    ClearCompleted,
    SaveSiteProfile {
        task_id: String,
    },
    ImportPaths {
        paths: Vec<String>,
    },
    ExportTasks {
        #[serde(default)]
        task_ids: Vec<String>,
        format: String,
    },
    PlaceQueue {
        task_id: String,
        before_id: String,
    },
    AssignQueue {
        task_ids: Vec<String>,
        queue_id: String,
    },
    HarvestPage {
        url: String,
        #[serde(default)]
        referer: String,
        #[serde(default)]
        probe_urls: Vec<String>,
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
pub struct PlayerTrack {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub language: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoreEvent {
    Ready {
        protocol: String,
        version: u32,
    },
    SettingsChanged {
        keys: Vec<String>,
    },
    ClipboardOffer {
        urls: Vec<String>,
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
    TorrentProbeResult {
        source: String,
        name: String,
        total_size: u64,
        files: Vec<TorrentFileEntry>,
        magnet: bool,
    },
    TorrentSelectionResult {
        source: String,
        selections: Vec<TorrentFileSelection>,
        total_size: u64,
    },
    TaskTorrentFiles {
        task_id: String,
        source: String,
        files: Vec<TorrentFileEntry>,
        selections: Vec<TorrentFileSelection>,
        total_size: u64,
    },
    CastDevices {
        devices: Vec<CastDeviceInfo>,
    },
    UpdateAvailable {
        current: String,
        latest: String,
        notes: String,
        release_url: String,
        installer_name: String,
        installer_size: u64,
        sha256_verified: bool,
    },
    UpdateCurrent {
        current: String,
    },
    UpdateReady {
        latest: String,
        installer_path: String,
        sha256: String,
        product_name: String,
        product_version: String,
        upgrade_code: String,
    },
    UpdateInstallStarted {
        latest: String,
        install_log: String,
        result_path: String,
    },
    UpdateInstallResult {
        latest: String,
        status: String,
        exit_code: i32,
        message: String,
        install_log: String,
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
    HarvestProbeResult {
        url: String,
        links: Vec<HarvestCandidate>,
    },
    TaskLog {
        task_id: String,
        lines: Vec<String>,
    },
    TaskExport {
        format: String,
        data: String,
        task_count: usize,
    },
    BrowserStatus {
        connected: bool,
        version: String,
        browser: String,
        message: String,
    },
    MediaPushRequested {
        request: MediaPushRequest,
    },
    MediaPushResolved {
        request: MediaPushRequest,
    },
    PowerActionPending {
        action: String,
        title: String,
        delay_seconds: u64,
    },
    CastSession {
        active: bool,
        title: String,
        device: String,
        status: String,
        task_id: String,
        media_url: String,
        device_kind: String,
        supported_actions: Vec<String>,
        playing: bool,
        paused: bool,
        position_seconds: u64,
        duration_seconds: u64,
        position_available: bool,
    },
    PlayerSession {
        active: bool,
        title: String,
        #[serde(default)]
        task_id: String,
        status: String,
        #[serde(default)]
        paused: bool,
        #[serde(default = "default_player_speed")]
        speed: f64,
        #[serde(default)]
        position_seconds: f64,
        #[serde(default)]
        duration_seconds: f64,
        #[serde(default)]
        position_available: bool,
        #[serde(default)]
        audio_tracks: u32,
        #[serde(default)]
        subtitle_tracks: u32,
        #[serde(default)]
        volume: f64,
        #[serde(default)]
        muted: bool,
        #[serde(default)]
        buffered_end_seconds: f64,
        #[serde(default)]
        buffering: bool,
        #[serde(default)]
        tracks: Vec<PlayerTrack>,
    },
}

fn default_method() -> String {
    "GET".to_string()
}

fn default_queue_id() -> String {
    DEFAULT_QUEUE_ID.into()
}

fn default_true() -> bool {
    true
}

fn default_queue_profile_max() -> u32 {
    3
}

fn default_queue_start() -> String {
    "00:00".into()
}

fn default_queue_stop() -> String {
    "23:59".into()
}

fn default_queue_days() -> String {
    "1,2,3,4,5,6,7".into()
}

fn default_player_speed() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_roundtrip_keeps_bounded_metadata() {
        let offer = ResourceOffer {
            url: "https://cdn.example/video.m3u8".into(),
            resource_kind: ResourceKind::Hls,
            local_source_path: None,
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
            mime_type: "application/vnd.apple.mpegurl".into(),
            size: 42,
            preferred_bandwidth: 2_500_000,
            preferred_height: 720,
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
                peer_count: 0,
                seed_count: 0,
                uploaded_bytes: 0,
                upload_speed_bytes_per_sec: 0,
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
                error_stage: String::new(),
                error_url: String::new(),
                error_hint: String::new(),
                http_status: None,
                error_attempt: 0,
                queue_index: 0,
                queue_id: DEFAULT_QUEUE_ID.into(),
                output_missing: false,
                output_path: String::new(),
                connection_hint: String::new(),
                connection_parts: Vec::new(),
                log_tail: Vec::new(),
                speed_history: Vec::new(),
                mirror_status: Vec::new(),
                request_method: "GET".into(),
                download_dir: String::new(),
                speed_limit_kib: 0,
                expected_checksum: String::new(),
                checksum_algorithm: String::new(),
                checksum_actual: String::new(),
                checksum_verified: None,
                av_scan: None,
                max_workers: 1,
                mirrors: Vec::new(),
                scheduled_start_at: String::new(),
                scheduled_stop_at: String::new(),
            },
        };
        // 与下面那些测试保持同一种序列化方式：直接 to_value，不给 CoreEvent 另造一个
        // “专用序列化入口”。管道上发事件本来就是 serde 直接序列化这个枚举，
        // 所以这条泄漏断言守护的就是生产路径本身，不是某个只为测试存在的包装。
        let payload = serde_json::to_value(&event).unwrap();
        assert!(payload.get("cookie").is_none());
        assert!(payload.get("authorization").is_none());
    }

    #[test]
    fn cast_session_exposes_lan_url() {
        let event = CoreEvent::CastSession {
            active: true,
            title: "video.mp4".into(),
            device: "局域网".into(),
            status: "PUBLISHED".into(),
            task_id: "task-1".into(),
            media_url: "http://192.168.1.8:49152/media/token/video.mp4".into(),
            device_kind: "lan".into(),
            supported_actions: vec!["stop".into()],
            playing: false,
            paused: false,
            position_seconds: 0,
            duration_seconds: 0,
            position_available: false,
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap()["media_url"],
            "http://192.168.1.8:49152/media/token/video.mp4"
        );
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
        let encoded = serde_json::to_value(CoreCommand::CheckUpdate { silent: true }).unwrap();
        assert_eq!(encoded["kind"], "check_update");
        assert_eq!(encoded["silent"], true);
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored, CoreCommand::CheckUpdate { silent: true });
    }

    #[test]
    fn install_update_command_keeps_the_workbench_process_identity() {
        let encoded = serde_json::to_value(CoreCommand::InstallUpdate {
            workbench_pid: 4242,
        })
        .unwrap();
        assert_eq!(encoded["kind"], "install_update");
        assert_eq!(encoded["workbench_pid"], 4242);
        assert_eq!(
            serde_json::from_value::<CoreCommand>(encoded).unwrap(),
            CoreCommand::InstallUpdate {
                workbench_pid: 4242
            }
        );
    }

    #[test]
    fn present_handoff_command_roundtrips() {
        let encoded = serde_json::to_value(CoreCommand::PresentHandoff {
            handoff_id: "handoff-1".into(),
            ok: false,
            presenter_id: "presenter-1".into(),
        })
        .unwrap();
        assert_eq!(encoded["kind"], "present_handoff");
        assert_eq!(encoded["ok"], false);
        assert_eq!(encoded["presenter_id"], "presenter-1");
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            restored,
            CoreCommand::PresentHandoff {
                handoff_id: "handoff-1".into(),
                ok: false,
                presenter_id: "presenter-1".into(),
            }
        );
    }

    #[test]
    fn probe_and_cast_commands_roundtrip() {
        let encoded = serde_json::to_value(CoreCommand::ProbeUrl {
            url: "https://cdn.test/a.m3u8".into(),
            spec: None,
        })
        .unwrap();
        assert_eq!(encoded["kind"], "probe_url");
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert!(matches!(restored, CoreCommand::ProbeUrl { .. }));
        let tvbox_discovery = CoreCommand::DiscoverCastDevices {
            mode: "tvbox".into(),
        };
        assert_eq!(
            serde_json::to_value(&tvbox_discovery).unwrap()["mode"],
            "tvbox"
        );
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

    #[test]
    fn request_refresh_roundtrips_without_putting_credentials_in_task_events() {
        let command = CoreCommand::RefreshTaskRequest {
            task_id: "task-1".into(),
            url: "https://cdn.test/new.bin".into(),
            cookie: "session=private".into(),
            auto_resume: true,
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["kind"], "refresh_task_request");
        assert_eq!(
            serde_json::from_value::<CoreCommand>(encoded).unwrap(),
            command
        );
        let snapshot = TaskSnapshot {
            task_id: "task-1".into(),
            url: "https://cdn.test/new.bin".into(),
            ..TaskSnapshot::default()
        };
        let event = serde_json::to_value(CoreEvent::TaskUpdated { snapshot }).unwrap();
        assert!(event.get("cookie").is_none());
        assert!(event.to_string().find("session=private").is_none());
    }

    #[test]
    fn share_media_command_roundtrips_without_losing_source() {
        let command = CoreCommand::ShareMedia {
            path: r"C:\Media\clip.mp4".into(),
            url: String::new(),
            title: "clip".into(),
            device_id: "dlna:living-room".into(),
        };
        let encoded = serde_json::to_value(&command).unwrap();
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored, command);
    }

    #[test]
    fn torrent_probe_and_selection_contract_roundtrip() {
        let selection = TorrentFileSelection {
            index: 2,
            path: "media/clip.mp4".into(),
            selected: true,
        };
        let command = CoreCommand::SelectTorrentFiles {
            source: "C:/Downloads/demo.torrent".into(),
            selections: vec![selection.clone()],
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["kind"], "select_torrent_files");
        let restored: CoreCommand = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored, command);
        let event = CoreEvent::TorrentProbeResult {
            source: "demo.torrent".into(),
            name: "demo".into(),
            total_size: 42,
            files: vec![TorrentFileEntry {
                index: 2,
                path: "media/clip.mp4".into(),
                size: 42,
                offset: 0,
            }],
            magnet: false,
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["kind"], "torrent_probe_result");
        assert_eq!(value["files"][0]["index"], 2);
        let task_command = CoreCommand::SetTaskTorrentFiles {
            task_id: "task-7".into(),
            selections: vec![selection.clone()],
        };
        let task_value = serde_json::to_value(&task_command).unwrap();
        assert_eq!(task_value["kind"], "set_task_torrent_files");
        assert_eq!(
            serde_json::from_value::<CoreCommand>(task_value).unwrap(),
            task_command
        );
    }
}
