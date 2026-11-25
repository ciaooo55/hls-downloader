#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![cfg_attr(not(test), warn(clippy::undocumented_unsafe_blocks))]

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(hls_native_clients::run_native_host() as u8)
}
