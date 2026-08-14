use hls_native_shell::{PROTOCOL_NAME, PROTOCOL_VERSION, decode_frame, encode_frame, paint_snapshot};
use serde_json::json;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--self-test") {
        let snapshot = paint_snapshot(&json!({
            "id": "self-test",
            "filename": "setup.exe",
            "url": "https://cdn.test/setup.exe",
            "size": 8
        }));
        let frame = match encode_frame(&json!({
            "protocol": PROTOCOL_NAME,
            "version": PROTOCOL_VERSION,
            "kind": "handoff",
            "presentable": true,
            "snapshot": snapshot
        })) {
            Ok(frame) => frame,
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
        };
        match decode_frame(&frame) {
            Ok(message) if message["snapshot"]["filename"] == "setup.exe" => {
                println!("{PROTOCOL_NAME}/{PROTOCOL_VERSION} ok");
                ExitCode::SUCCESS
            }
            Ok(_) => {
                eprintln!("self-test snapshot mismatch");
                ExitCode::from(1)
            }
            Err(err) => {
                eprintln!("{err}");
                ExitCode::from(1)
            }
        }
    } else {
        println!(
            "{PROTOCOL_NAME} {PROTOCOL_VERSION} — protocol codec only; Windows tray/HWND is not in this binary yet"
        );
        ExitCode::SUCCESS
    }
}
