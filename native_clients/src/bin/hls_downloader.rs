#![cfg_attr(
    all(windows, not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]

use std::{
    env,
    path::Path,
    process::{Command, Stdio},
};

fn main() {
    if let Err(error) = launch() {
        #[cfg(windows)]
        if !env::args_os().any(|arg| arg == "--self-test") {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    MessageBoxW, MB_ICONERROR, MB_OK,
                };
                let title: Vec<u16> = "HLS Downloader\0".encode_utf16().collect();
                let message: Vec<u16> = format!("{error}\0").encode_utf16().collect();
                MessageBoxW(
                    std::ptr::null_mut(),
                    message.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn launch() -> Result<(), String> {
    let executable = env::current_exe().map_err(|e| e.to_string())?;
    let root = executable.parent().ok_or("无法确定安装目录")?;
    let resources = root.join("app/resources");
    let presenter = resources.join("HLSDownloaderPresenter.exe");
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("HLS Downloader {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    verify_payload(&resources)?;
    if args.iter().any(|arg| arg == "--self-test") {
        return Ok(());
    }
    let mut command = Command::new(presenter);
    command
        .current_dir(resources)
        .args(args)
        .arg("--workbench")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x01000000);
    }
    command
        .spawn()
        .map_err(|e| format!("无法启动原生界面：{e}"))?;
    Ok(())
}

fn verify_payload(resources: &Path) -> Result<(), String> {
    for name in [
        "HLSDownloaderPresenter.exe",
        "HLSDownloaderEngine.exe",
        "HLSDownloaderNativeHost.exe",
        "HLSDownloaderUpdater.exe",
        "HLSDownloaderCli.exe",
    ] {
        if !resources.join(name).is_file() {
            return Err(format!("安装组件缺失：{name}，请修复安装"));
        }
    }
    Ok(())
}
