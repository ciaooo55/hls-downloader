//! Bounded v7 little-endian JSON wire format.
use crate::{CoreCommand, EventEnvelope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const V7_PIPE_NAME: &str = r"\\.\pipe\HLSDownloader.v7";
pub const V7_PIPE_MAX_FRAME: usize = 4 * 1024 * 1024;
pub const V7_PAGE_PAYLOAD_BUDGET: usize = V7_PIPE_MAX_FRAME - 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskPage {
    pub tasks: Vec<crate::TaskSnapshot>,
    pub next: Option<String>,
    pub sequence: u64,
    pub browser_connected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventBatch {
    pub events: Vec<EventEnvelope>,
    pub latest_sequence: u64,
    pub resync_required: bool,
}

/// Physical client coordinates in the actual native UI window. A missing
/// parent detaches the video surface without reopening or stopping media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerEmbed {
    pub session_generation: u64,
    pub layout_generation: u64,
    pub parent_hwnd: Option<i64>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CorePipeRequest {
    CastControl {
        request_id: u64,
        session_generation: u64,
        action: String,
    },
    PlayerControl {
        request_id: u64,
        session_generation: u64,
        action: String,
    },
    PlayerEmbed {
        request_id: u64,
        embed: PlayerEmbed,
    },
    Crawl {
        request_id: u64,
        request: crate::CrawlRequest,
    },
    PreviewZip {
        request_id: u64,
        source: String,
    },
    Hello {
        protocol: String,
        version: u32,
    },
    Command {
        request_id: u64,
        command: CoreCommand,
    },
    Snapshot {
        request_id: u64,
    },
    ListTasks {
        request_id: u64,
        #[serde(default)]
        after_task_id: Option<String>,
        #[serde(default = "default_task_page_limit")]
        limit: usize,
    },
    WaitEventBatch {
        request_id: u64,
        after_sequence: u64,
        timeout_ms: u64,
    },
    Capabilities {
        request_id: u64,
    },
    WaitEvents {
        request_id: u64,
        after_sequence: u64,
        timeout_ms: u64,
    },
    StoreSetting {
        request_id: u64,
        key: String,
        value: Value,
    },
    StoreSettings {
        request_id: u64,
        values: std::collections::BTreeMap<String, Value>,
    },
    LoadSettings {
        request_id: u64,
    },
    SetDefaultCookie {
        request_id: u64,
        cookie: String,
    },
    SetSiteRuleCredential {
        request_id: u64,
        host: String,
        #[serde(default)]
        cookie: String,
        #[serde(default)]
        request_headers: std::collections::BTreeMap<String, String>,
        #[serde(default)]
        clear: bool,
    },
    StoreCredential {
        request_id: u64,
        credential_ref: String,
        protected_blob: String,
        kind: String,
    },
    LoadCredential {
        request_id: u64,
        credential_ref: String,
    },
    DeleteCredential {
        request_id: u64,
        credential_ref: String,
    },
    AdmitHandoff {
        request_id: u64,
        handoff_json: String,
    },
    SaveHandoff {
        request_id: u64,
        handoff_id: String,
        handoff_json: String,
        status: String,
        task_id: Option<String>,
        created_at_ms: u64,
    },
    LoadHandoffs {
        request_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handoff_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_request_id: Option<String>,
        #[serde(default)]
        pending_only: bool,
        #[serde(default)]
        paged: bool,
        #[serde(default)]
        after: Option<[i64; 2]>,
    },
    Subtitle {
        request_id: u64,
        request: Value,
    },
    PlayerThumbnail {
        request_id: u64,
        task_id: String,
        seconds: Option<f64>,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CorePipeResponse {
    TaskPage {
        request_id: u64,
        page: TaskPage,
    },
    EventBatch {
        request_id: u64,
        batch: EventBatch,
    },
    Crawl {
        request_id: u64,
        projects: Vec<crate::CrawlProject>,
    },
    ZipPreview {
        request_id: u64,
        preview: crate::ZipPreview,
    },
    Hello {
        protocol: String,
        version: u32,
        pid: u32,
    },
    Events {
        request_id: u64,
        events: Vec<EventEnvelope>,
    },
    Snapshot {
        request_id: u64,
        tasks: Vec<crate::TaskSnapshot>,
        #[serde(default)]
        latest_sequence: u64,
        #[serde(default)]
        browser_connected: bool,
    },
    Capabilities {
        request_id: u64,
        product_version: String,
        protocol_version: u32,
        commands: Vec<String>,
        settings: Vec<String>,
        max_frame_bytes: u64,
    },
    Settings {
        request_id: u64,
        takeover_enabled: bool,
        takeover_minimum_bytes: u64,
        legal_accepted: bool,
        speed_limit_kib: u64,
        #[serde(default)]
        hourly_quota_mib: u64,
        #[serde(default)]
        schedule_enabled: bool,
        #[serde(default = "default_schedule_start")]
        schedule_start: String,
        #[serde(default = "default_schedule_end")]
        schedule_end: String,
        #[serde(default)]
        schedule_kib: u64,
        #[serde(default)]
        auto_category: bool,
        #[serde(default)]
        category_dir_media: String,
        #[serde(default)]
        category_dir_program: String,
        #[serde(default)]
        category_dir_archive: String,
        #[serde(default)]
        category_dir_other: String,
        #[serde(default = "default_queue_max")]
        queue_max: u64,
        #[serde(default)]
        queue_profiles: Vec<crate::QueueProfile>,
        #[serde(default)]
        site_rules: String,
        #[serde(default)]
        av_scan_enabled: bool,
        #[serde(default)]
        av_scan_command: String,
        #[serde(default)]
        torrent_watch: String,
        #[serde(default)]
        torrent_watch_enabled: bool,
        #[serde(default)]
        download_dir: String,
        #[serde(default)]
        temp_dir: String,
        #[serde(default = "default_concurrency")]
        default_concurrency: u64,
        #[serde(default)]
        proxy_url: String,
        #[serde(default)]
        ffmpeg_path: String,
        #[serde(default)]
        clipboard_watch: bool,
        #[serde(default)]
        completion_sound_enabled: bool,
        #[serde(default = "default_true")]
        progress_window_enabled: bool,
        #[serde(default = "default_true")]
        complete_popup_enabled: bool,
        #[serde(default)]
        resume_interrupted: bool,
        #[serde(default)]
        auto_retry_max: u64,
        #[serde(default = "default_rename")]
        existing_file_policy: String,
        #[serde(default)]
        live_record_max_minutes: u64,
        #[serde(default = "default_true")]
        download_subtitles: bool,
        #[serde(default = "default_true")]
        skip_ad_segments: bool,
        #[serde(default)]
        keep_temp_files: bool,
        #[serde(default)]
        default_user_agent: String,
        #[serde(default)]
        tvbox_endpoint: String,
        #[serde(default)]
        dark_mode: bool,
        #[serde(default)]
        allow_duplicate: bool,
        #[serde(default)]
        queue_auto_start_enabled: bool,
        #[serde(default)]
        queue_auto_start_time: String,
        #[serde(default)]
        queue_auto_stop_enabled: bool,
        #[serde(default)]
        queue_auto_stop_time: String,
        #[serde(default)]
        default_referer: String,
        #[serde(default)]
        default_origin: String,
        #[serde(default)]
        allowed_hosts: String,
        #[serde(default = "default_chunk_mb")]
        http_chunk_size_mb: u64,
        #[serde(default = "default_none")]
        completion_power_action: String,
        #[serde(default)]
        start_on_login: bool,
        #[serde(default = "default_queue_days")]
        queue_active_days: String,
        #[serde(default = "default_proxy_mode")]
        proxy_mode: String,
        #[serde(default)]
        proxy_bypass: String,
        #[serde(default)]
        legal_terms_version: String,
        #[serde(default)]
        reduce_motion: bool,
        #[serde(default)]
        harvest_minimum_bytes: u64,
        #[serde(default = "default_true")]
        av_scan_fail_on_threat: bool,
        #[serde(default = "default_bt_upload_limit")]
        bt_upload_limit_kib: u64,
        #[serde(default = "default_bt_connections")]
        bt_max_connections: u64,
        #[serde(default = "default_true")]
        bt_enable_dht: bool,
        #[serde(default)]
        preferred_cast_device_id: String,
        #[serde(default)]
        task_column_layout: String,
        #[serde(default)]
        toolbar_actions: String,
        #[serde(default = "default_task_sort")]
        task_sort: String,
        #[serde(default)]
        default_cookie_configured: bool,
    },
    Credential {
        request_id: u64,
        protected_blob: Option<String>,
    },
    Handoffs {
        request_id: u64,
        items: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<[i64; 2]>,
    },
    Subtitle {
        request_id: u64,
        result: Value,
    },
    PlayerThumbnail {
        request_id: u64,
        jpeg_base64: String,
    },
    Error {
        request_id: Option<u64>,
        code: String,
        message: String,
    },
}

fn default_task_page_limit() -> usize {
    128
}

fn default_schedule_start() -> String {
    "22:00".into()
}

fn default_schedule_end() -> String {
    "08:00".into()
}

fn default_queue_max() -> u64 {
    3
}

fn default_concurrency() -> u64 {
    12
}

fn default_true() -> bool {
    true
}

fn default_rename() -> String {
    "rename".into()
}

fn default_chunk_mb() -> u64 {
    8
}

fn default_none() -> String {
    "none".into()
}

fn default_queue_days() -> String {
    "1,2,3,4,5,6,7".into()
}

fn default_proxy_mode() -> String {
    "system".into()
}

fn default_task_sort() -> String {
    "queue:asc".into()
}

fn default_bt_upload_limit() -> u64 {
    1024
}

fn default_bt_connections() -> u64 {
    200
}

pub fn encode_message<T: Serialize>(message: &T) -> Result<Vec<u8>, String> {
    let payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    if payload.len() > V7_PIPE_MAX_FRAME {
        return Err("v7 Core pipe frame too large".into());
    }
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_message<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> Result<T, String> {
    if frame.len() < 4 {
        return Err("Core pipe frame truncated".into());
    }
    let length = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
    if length > V7_PIPE_MAX_FRAME || frame.len() < length + 4 {
        return Err("v7 Core pipe frame invalid length".into());
    }
    serde_json::from_slice(&frame[4..4 + length])
        .map_err(|error| format!("v7 Core pipe JSON invalid: {error}"))
}
