#[cfg(feature = "full-core")]
mod atomic_replace;
#[cfg(feature = "full-core")]
mod av_scan;
#[cfg(feature = "full-core")]
pub mod cast;
#[cfg(feature = "full-core")]
mod category;
#[cfg(feature = "full-core")]
mod checksum;
#[cfg(feature = "full-core")]
mod clipboard;
#[cfg(feature = "full-core")]
mod connection_parts;
use hls_client::core_ipc;
use hls_protocol::contract;
#[cfg(feature = "full-core")]
mod core_runtime;
#[cfg(feature = "full-core")]
mod core_server;
#[cfg(feature = "full-core")]
mod core_service;
use hls_client::core_spawn;
#[cfg(feature = "full-core")]
mod crawl;
use hls_protocol::crawl_contract;
#[cfg(feature = "full-core")]
mod credentials;
#[cfg(feature = "full-core")]
mod crypto_lite;
#[cfg(feature = "full-core")]
mod curl_import;
#[cfg(feature = "full-core")]
mod download_worker;
#[cfg(feature = "full-core")]
mod duplicate;
#[cfg(feature = "full-core")]
mod ftp_engine;
#[cfg(feature = "full-core")]
mod harvest;
#[cfg(feature = "full-core")]
mod http_engine;
use hls_client::instance;
#[cfg(feature = "full-core")]
mod link_file;
#[cfg(feature = "full-core")]
mod local_time;
#[cfg(feature = "full-core")]
mod media;
use hls_client::media_ext;
#[cfg(feature = "full-core")]
mod media_proxy;
#[cfg(feature = "full-core")]
mod metalink;
#[cfg(feature = "full-core")]
mod mirrors;
#[cfg(feature = "full-core")]
mod motw;
#[cfg(all(test, feature = "full-core"))]
#[path = "../../native_clients/src/native_host.rs"]
mod native_host;
#[cfg(all(test, feature = "full-core"))]
pub use native_host::run as run_native_host;
#[cfg(feature = "full-core")]
mod native_host_registration;
#[cfg(feature = "full-core")]
mod net_policy;
#[cfg(feature = "full-core")]
mod output_path;
#[cfg(feature = "full-core")]
mod playback;
#[cfg(feature = "full-core")]
mod player;
#[cfg(feature = "full-core")]
mod player_thumbnail;
#[cfg(feature = "full-core")]
mod power_action;
use hls_client::profile_paths;
#[cfg(feature = "full-core")]
mod recognize;
use hls_client::runtime_log;
#[cfg(feature = "full-core")]
mod sftp_engine;
#[cfg(feature = "full-core")]
mod sftp_live;
#[cfg(feature = "full-core")]
mod site_rules;
#[cfg(feature = "full-core")]
mod sleep_inhibit;
#[cfg(feature = "full-core")]
mod startup;
#[cfg(feature = "full-core")]
mod store;
#[cfg(feature = "full-core")]
mod subtitle_audio_source;
#[cfg(feature = "full-core")]
mod subtitle_translate;
#[cfg(feature = "full-core")]
mod task_export;
#[cfg(feature = "full-core")]
mod torrent_engine;
use hls_client::tray;
#[cfg(feature = "full-core")]
mod updater;
#[cfg(feature = "full-core")]
mod win_reg;
use hls_client::window_util;
#[cfg(feature = "full-core")]
mod zip_preview;

#[cfg(feature = "full-core")]
pub use clipboard::{
    all_urls as clipboard_all_urls, first_url as clipboard_first_url, looks_like_download_url,
    read_text as read_clipboard, write_files as write_clipboard_files,
    write_text as write_clipboard,
};
pub use contract::{
    AvScanStatus, CastDeviceInfo, ConnectionPart, CoreCommand, CoreEvent, EventEnvelope,
    HarvestCandidate, MediaPushRequest, MirrorStatus, PlayerTrack, QueueProfile, ResourceKind,
    ResourceOffer, StreamVariant, TaskFailure, TaskSnapshot, TaskSpec, TorrentFileEntry,
    TorrentFileSelection, ZipEntryPreview, ZipPreview, DEFAULT_QUEUE_ID, LEGAL_TERMS_VERSION,
    V7_PROTOCOL_NAME, V7_PROTOCOL_VERSION,
};
pub use core_ipc::{
    default_core_bind, hello_request, serve_tcp_listener, tcp_loopback_enabled, v7_pipe_name,
    CoreIpcClient, CorePipeRequest, CorePipeResponse, PlayerEmbed, V7_PIPE_NAME, V7_TCP_PORT,
};
#[cfg(windows)]
pub use core_ipc::{NamedPipeClient, NamedPipeServer};
#[cfg(feature = "full-core")]
pub use core_runtime::CoreRuntime;
#[cfg(feature = "full-core")]
pub use core_server::CoreServer;
#[cfg(feature = "full-core")]
pub use core_service::PersistentCore;
pub use core_spawn::{install_root, locate_core_executable, spawn_core};
pub use core_spawn::{locate_desktop_executable, spawn_desktop_ui};
pub use crawl_contract::{CrawlProject, CrawlRequest, CrawlResource, CrawlSpec};
#[cfg(feature = "full-core")]
pub use credentials::{
    apply_replay_json, apply_replay_json_for, with_replay_json, CredentialVault,
};
#[cfg(feature = "full-core")]
pub use curl_import::{parse_curl_command, CurlDownload};
#[cfg(feature = "full-core")]
pub use download_worker::{CoreCoordinator, CoreSettings, TaskPaths};
#[cfg(feature = "full-core")]
pub use harvest::{harvest_html, harvest_html_filtered, HarvestLink};
#[cfg(feature = "full-core")]
pub use hls_client::file_transfer::{begin_file_drag, completed_file_drag, hdrop_bytes};
#[cfg(feature = "full-core")]
pub use http_engine::{run_job, run_job_report, EngineError, HttpMirrorReport, HttpRunReport};
pub use instance::{claim_v7_instance, claim_v7_presenter_instance, is_already_running_error};
pub use media_ext::download_category;
#[cfg(feature = "full-core")]
pub use metalink::{looks_like_metalink, parse_metalink};
#[cfg(feature = "full-core")]
pub use native_host_registration::{
    prepare_packaged_native_host_manifests, register_packaged_native_host,
    register_packaged_native_host_for_sid, unregister_packaged_native_host,
    unregister_packaged_native_host_for_sid,
};
#[cfg(feature = "full-core")]
pub use player::{run_player_process, PLAYER_WINDOW_TITLE};
pub use profile_paths::{default_v7_database_path, default_v7_download_dir};
#[cfg(feature = "full-core")]
pub use recognize::{classify_url, kind_label, probe_url};
#[cfg(feature = "full-core")]
pub use runtime_log::{init_runtime_log, record_runtime_log};
#[cfg(feature = "full-core")]
pub use site_rules::{format_site_rules, parse_site_rules, upsert_site_rule, SiteRule};
#[cfg(feature = "full-core")]
pub use store::{CoreStore, CURRENT_SCHEMA_VERSION};
#[cfg(feature = "full-core")]
pub use task_export::export_tasks;
#[cfg(feature = "full-core")]
pub use torrent_engine::{torrent_session, BuiltinTorrentEngine, TorrentSession};
pub use tray::{completion_sound, show_notification, spawn_tray, TrayAction};
#[cfg(feature = "full-core")]
pub use updater::{check_for_update, is_newer_version, CURRENT_VERSION};
pub use window_util::{
    activate_window_by_title, begin_caption_drag, center_window_by_title,
    hide_window_from_taskbar_by_title, os_reduce_motion, window_handle_by_title,
    window_work_area_size_by_title,
};
