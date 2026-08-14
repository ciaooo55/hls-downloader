mod core_client;
mod protocol;
mod surfaces;
mod task_list;

#[cfg(windows)]
pub mod win32;

pub use core_client::CoreClient;
pub use protocol::{
    decode_frame, encode_frame, paint_snapshot, MAX_FRAME_BYTES, PAINT_KEYS, PROTOCOL_NAME,
    PROTOCOL_VERSION,
};
pub use surfaces::{OverlayWindow, ResidentShell, Snapshot, Windows};
pub use task_list::{FileCategory, StatusFilter, TaskList, TaskRow};
