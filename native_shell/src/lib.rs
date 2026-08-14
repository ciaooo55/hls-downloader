mod core_client;
mod http_engine;
mod protocol;
mod surfaces;
mod task_list;

#[cfg(windows)]
pub mod win32;

pub use core_client::CoreClient;
pub use http_engine::{
    finish_job, load_job, run_job, run_queued_job, EngineError, EXIT_CANCEL, EXIT_ERROR, EXIT_OK,
    EXIT_PAUSE, EXIT_RANGE_UNSUPPORTED,
};
pub use protocol::{
    decode_frame, encode_frame, paint_snapshot, MAX_FRAME_BYTES, PAINT_KEYS, PROTOCOL_NAME,
    PROTOCOL_VERSION,
};
pub use surfaces::{OverlayWindow, ResidentShell, Snapshot, Windows};
pub use task_list::{FileCategory, StatusFilter, TaskList, TaskRow};
