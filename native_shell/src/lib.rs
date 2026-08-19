mod av_scan;
mod category;
mod checksum;
mod clipboard;
mod connection_parts;
mod curl_import;
mod drop_target;
mod file_dialog;
mod sleep_inhibit;
#[cfg(feature = "supervisor")]
mod core_client;
mod core_ipc;
mod core_runtime;
mod core_server;
mod core_service;
#[cfg(feature = "supervisor")]
mod core_spawn;
mod credentials;
mod crypto_lite;
mod download_worker;
mod duplicate;
mod ftp_engine;
mod harvest;
mod http_engine;
mod instance;
mod link_file;
mod media;
mod metalink;
mod power_action;
mod migrate;
mod mirrors;
mod motw;
mod native_host;
mod net_policy;
mod ole_drag;
mod output_path;
mod playback;
mod player;
#[cfg(feature = "supervisor")]
mod protocol;
mod recognize;
mod cast;
mod sftp_engine;
mod sftp_live;
mod site_rules;
mod startup;
#[cfg(feature = "supervisor")]
mod surfaces;
#[cfg(feature = "supervisor")]
mod task_list;
mod torrent_engine;
mod tray;
mod updater;
mod v6_contract;
mod v6_store;
mod window_util;

#[cfg(all(windows, feature = "supervisor"))]
pub mod win32;

#[cfg(feature = "supervisor")]
pub use core_client::CoreClient;
pub use core_ipc::{
    default_core_bind, hello_request, serve_tcp_listener, tcp_loopback_enabled, v6_pipe_name,
    CoreIpcClient, CorePipeRequest, CorePipeResponse, V6_PIPE_NAME, V6_TCP_PORT,
};
#[cfg(windows)]
pub use core_ipc::{NamedPipeClient, NamedPipeServer};
pub use core_runtime::{CoreRuntime, EventEnvelope};
pub use core_server::CoreServer;
pub use core_service::PersistentCore;
#[cfg(feature = "supervisor")]
pub use core_spawn::{
    download_import_route, install_root, locate_core_executable, locate_desktop_executable,
    spawn_core, spawn_desktop_ui,
};
pub use credentials::{apply_replay_json, apply_replay_json_for, with_replay_json, CredentialVault};
pub use download_worker::{CoreCoordinator, CoreSettings, TaskPaths};
pub use http_engine::{
    fetch_bytes, finish_job, load_job, run_job, run_queued_job, EngineError, EXIT_CANCEL,
    EXIT_ERROR, EXIT_OK, EXIT_PAUSE, EXIT_RANGE_UNSUPPORTED,
};
pub use instance::claim_v6_instance;
pub use player::PLAYER_WINDOW_TITLE;
pub use window_util::{begin_caption_drag, os_reduce_motion, window_handle_by_title};
pub use ole_drag::{begin_file_drag, completed_file_drag, hdrop_bytes};
pub use native_host::run as run_native_host;
pub use clipboard::{
    all_urls as clipboard_all_urls, first_url as clipboard_first_url, looks_like_download_url,
    read_text as read_clipboard, write_files as write_clipboard_files,
    write_text as write_clipboard,
};
pub use connection_parts::{paint_file_map, paint_from_progress, sample_cells, summarize as summarize_parts};
pub use curl_import::{parse_curl_command, CurlDownload};
pub use file_dialog::{pick_export_path, pick_import_paths};
pub use drop_target::attach_file_drop;
pub use site_rules::{format_site_rules, parse_site_rules, upsert_site_rule, SiteRule};
pub use torrent_engine::{torrent_session, BuiltinTorrentEngine, TorrentSession};
pub use tray::{completion_sound, show_notification, spawn_tray, TrayAction};
#[cfg(feature = "supervisor")]
pub use protocol::{
    decode_frame, encode_frame, paint_snapshot, MAX_FRAME_BYTES, PAINT_KEYS, PROTOCOL_NAME,
    PROTOCOL_VERSION,
};
#[cfg(feature = "supervisor")]
pub use surfaces::{OverlayWindow, ResidentShell, Snapshot, Windows};
#[cfg(feature = "supervisor")]
pub use task_list::{FileCategory, StatusFilter, TaskList, TaskRow};
pub use recognize::{classify_url, kind_label, probe_url};
pub use metalink::{looks_like_metalink, parse_metalink};
pub use harvest::{harvest_html, harvest_html_filtered, HarvestLink};
pub use updater::{check_for_update, is_newer_version, CURRENT_VERSION};
pub use migrate::{maybe_migrate_from_5x, migrate_from_5x};
pub use v6_store::{default_v6_database_path, V6Store, V6_SCHEMA_VERSION};
pub use v6_contract::{
    CastDeviceInfo, ConnectionPart, CoreCommand, CoreEvent, HarvestCandidate, LEGAL_TERMS_VERSION,
    ResourceKind, ResourceOffer, StreamVariant, TaskSnapshot, TaskSpec, V6_PROTOCOL_NAME,
    V6_PROTOCOL_VERSION,
};
