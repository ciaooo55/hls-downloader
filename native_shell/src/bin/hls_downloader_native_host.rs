#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(hls_native_shell::run_native_host() as u8)
}
