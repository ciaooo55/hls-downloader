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
    let packaged_resources = root.join("app/resources");
    let resources = if packaged_resources.is_dir() {
        packaged_resources
    } else {
        root.to_path_buf()
    };
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("HLS Downloader {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let presenter = verify_payload(&resources)?;
    if args.iter().any(|arg| arg == "--self-test") {
        return Ok(());
    }
    let mut command = Command::new(&presenter);
    command
        .current_dir(&resources)
        .args(args)
        .arg("--workbench")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        match command.spawn() {
            Ok(_) => Ok(()),
            Err(error) if error.raw_os_error() == Some(5) => {
                command.creation_flags(CREATE_NO_WINDOW);
                command
                    .spawn()
                    .map_err(|e| format!("无法启动原生界面：{e}"))?;
                Ok(())
            }
            Err(e) => Err(format!("无法启动原生界面：{e}")),
        }
    }
    #[cfg(not(windows))]
    {
        command
            .spawn()
            .map_err(|e| format!("无法启动原生界面：{e}"))?;
        Ok(())
    }
}

fn verify_payload(resources: &Path) -> Result<std::path::PathBuf, String> {
    let presenter = ["HLSDownloaderPresenter.exe", "hls-downloader-presenter.exe"]
        .into_iter()
        .map(|name| resources.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| "安装组件缺失：HLSDownloaderPresenter.exe，请修复安装".to_string())?;
    let engine_exists = ["HLSDownloaderEngine.exe", "hls-downloader-engine.exe"]
        .into_iter()
        .any(|name| resources.join(name).is_file());
    if !engine_exists {
        return Err("安装组件缺失：HLSDownloaderEngine.exe，请修复安装".to_string());
    }
    for name in [
        "HLSDownloaderNativeHost.exe",
        "HLSDownloaderUpdater.exe",
        "HLSDownloaderCli.exe",
    ] {
        if !resources.join(name).is_file() {
            return Err(format!("安装组件缺失：{name}，请修复安装"));
        }
    }
    Ok(presenter)
}
