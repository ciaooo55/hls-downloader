mod core_client;
mod protocol;
mod surfaces;

#[cfg(windows)]
pub mod win32;

pub use core_client::CoreClient;
pub use protocol::{
    decode_frame, encode_frame, paint_snapshot, PROTOCOL_NAME, PROTOCOL_VERSION, MAX_FRAME_BYTES,
    PAINT_KEYS,
};
pub use surfaces::{OverlayWindow, ResidentShell, Snapshot, Windows};
