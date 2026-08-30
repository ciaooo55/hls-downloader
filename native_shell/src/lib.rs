mod av_scan;
mod cast;
mod category;
mod checksum;
mod clipboard;
mod connection_parts;
mod contract;
mod core_ipc;
mod core_runtime;
mod core_server;
mod core_service;
mod core_spawn;
mod credentials;
mod crypto_lite;
mod curl_import;
mod download_worker;
mod drop_target;
mod duplicate;
mod file_dialog;
mod ftp_engine;
mod harvest;
mod http_engine;
mod instance;
mod link_file;
mod media;
mod metalink;
mod migrate;
mod mirrors;
mod motw;
mod native_host;
mod native_host_registration;
mod net_policy;
mod ole_drag;
mod output_path;
mod playback;
mod player;
mod power_action;
mod recognize;
mod sftp_engine;
mod sftp_live;
mod site_rules;
mod sleep_inhibit;
mod startup;
mod store;
mod task_export;
mod torrent_engine;
mod tray;
mod updater;
mod v6_migrate;
mod window_util;

pub use clipboard::{
    all_urls as clipboard_all_urls, first_url as clipboard_first_url, looks_like_download_url,
    read_text as read_clipboard, write_files as write_clipboard_files,
    write_text as write_clipboard,
};
pub use connection_parts::{
    active_worker_count, paint_file_map, paint_from_progress, sample_cells,
    summarize as summarize_parts,
};
pub use contract::{
    AvScanStatus, CastDeviceInfo, ConnectionPart, CoreCommand, CoreEvent, HarvestCandidate,
    MediaPushRequest, MirrorStatus, QueueProfile, ResourceKind, ResourceOffer, StreamVariant,
    TaskFailure, TaskSnapshot, TaskSpec, TorrentFileEntry, TorrentFileSelection, DEFAULT_QUEUE_ID,
    LEGAL_TERMS_VERSION, V6_PROTOCOL_NAME, V6_PROTOCOL_VERSION, V7_PROTOCOL_NAME,
    V7_PROTOCOL_VERSION,
};
pub use core_ipc::{
    default_core_bind, hello_request, serve_tcp_listener, tcp_loopback_enabled, v7_pipe_name,
    CoreIpcClient, CorePipeRequest, CorePipeResponse, V7_PIPE_NAME, V7_TCP_PORT,
};
#[cfg(windows)]
pub use core_ipc::{NamedPipeClient, NamedPipeServer};
pub use core_runtime::{CoreRuntime, EventEnvelope};
pub use core_server::CoreServer;
pub use core_service::PersistentCore;
pub use core_spawn::{install_root, locate_core_executable, spawn_core};
pub use core_spawn::{locate_desktop_executable, spawn_desktop_ui};
pub use credentials::{
    apply_replay_json, apply_replay_json_for, with_replay_json, CredentialVault,
};
pub use curl_import::{parse_curl_command, CurlDownload};
pub use download_worker::{CoreCoordinator, CoreSettings, TaskPaths};
pub use drop_target::attach_file_drop;
pub use file_dialog::{pick_export_path, pick_import_paths};
pub use harvest::{harvest_html, harvest_html_filtered, HarvestLink};
pub use http_engine::{
    fetch_bytes, finish_job, load_job, run_job, run_job_report, run_queued_job, EngineError,
    HttpMirrorReport, HttpRunReport, EXIT_CANCEL, EXIT_ERROR, EXIT_OK, EXIT_PAUSE,
    EXIT_RANGE_UNSUPPORTED,
};
pub use instance::{claim_v7_instance, claim_v7_presenter_instance};
pub use metalink::{looks_like_metalink, parse_metalink};
pub use migrate::{maybe_migrate_from_5x, migrate_from_5x};
pub use native_host::run as run_native_host;
pub use native_host_registration::{
    register_packaged_native_host, unregister_packaged_native_host,
};
pub use ole_drag::{begin_file_drag, completed_file_drag, hdrop_bytes};
pub use player::{run_player_process, PLAYER_WINDOW_TITLE};
pub use recognize::{classify_url, kind_label, probe_url};
pub use site_rules::{format_site_rules, parse_site_rules, upsert_site_rule, SiteRule};
pub use store::{
    default_v7_database_path, default_v7_download_dir, CoreStore, CURRENT_SCHEMA_VERSION,
};
pub use task_export::export_tasks;
pub use torrent_engine::{torrent_session, BuiltinTorrentEngine, TorrentSession};
pub use tray::{completion_sound, show_notification, spawn_tray, TrayAction};
pub use updater::{check_for_update, is_newer_version, run_update_helper, CURRENT_VERSION};
pub use window_util::{
    activate_window_by_title, begin_caption_drag, center_window_by_title,
    hide_window_from_taskbar_by_title, os_reduce_motion, window_handle_by_title,
};
