#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--player-process") {
        return ExitCode::from(hls_native_shell::run_player_process() as u8);
    }
    if std::env::args().any(|arg| arg == "--self-test") {
        match hls_native_shell::CoreServer::in_memory() {
            Ok(server) => match server.coordinator().tasks() {
                Ok(tasks) if tasks.is_empty() => {
                    println!("HLSDownloaderEngine 7.0.0 self-test ok");
                    ExitCode::SUCCESS
                }
                Ok(_) => ExitCode::from(1),
                Err(error) => {
                    eprintln!("engine self-test failed: {error}");
                    ExitCode::from(1)
                }
            },
            Err(error) => {
                eprintln!("engine self-test failed: {error}");
                ExitCode::from(1)
            }
        }
    } else {
        if let Err(error) = hls_native_shell::claim_v7_instance() {
            if error.contains("already running") {
                return ExitCode::SUCCESS;
            }
            eprintln!("download engine instance startup failed: {error}");
            return ExitCode::from(1);
        }
        let server = match hls_native_shell::CoreServer::open_default() {
            Ok(server) => server,
            Err(error) => {
                eprintln!("download engine startup failed: {error}");
                return ExitCode::from(1);
            }
        };
        match server.bind_local() {
            Ok((address, worker)) => {
                eprintln!("download engine listening on {address}");
                match worker.join() {
                    Ok(Ok(())) => ExitCode::SUCCESS,
                    Ok(Err(error)) => {
                        eprintln!("{error}");
                        ExitCode::from(1)
                    }
                    Err(_) => ExitCode::from(1),
                }
            }
            Err(error) => {
                eprintln!("download engine bind failed: {error}");
                ExitCode::from(1)
            }
        }
    }
}
