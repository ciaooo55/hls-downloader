#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    match prepare_install() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("升级前退出失败: {error}");
            ExitCode::from(1)
        }
    }
}

fn prepare_install() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err("升级前退出需要唯一的原安装目录".into());
    }
    let root = std::fs::canonicalize(Path::new(&args[0]))
        .map_err(|error| format!("无法读取原安装目录: {error}"))?;
    if !root.join("HLSDownloader.exe").is_file()
        || !root.join("app/resources/HLSDownloaderEngine.exe").is_file()
    {
        return Err("原安装目录缺少程序标识，未停止任何进程".into());
    }
    // Binary 表提取的 EXE 可能位于受保护的 Windows\Installer，脚本写入当前用户临时目录。
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let script =
        std::env::temp_dir().join(format!("HLSInstaller-{}-{nonce}.ps1", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&script)
        .map_err(|error| format!("无法创建临时停机脚本: {error}"))?;
    // Windows PowerShell 5.1 需要 BOM 才能正确读取中文。
    let written = file
        .write_all(b"\xEF\xBB\xBF")
        .and_then(|_| file.write_all(include_bytes!("shutdown-running.ps1")));
    drop(file);
    let result = (|| {
        written.map_err(|error| error.to_string())?;
        let system = std::env::var_os("SystemRoot").ok_or("缺少 Windows 系统目录")?;
        let mut command = Command::new(
            PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        );
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .arg("-InstallDir")
            .arg(&args[0])
            .args(["-IncludeNativeHost", "-RequireCheckpoint"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match child.try_wait().map_err(|error| error.to_string())? {
                Some(status) if status.success() => return Ok(()),
                Some(_) => return Err("旧程序未能安全退出，安装已停止，未移除旧版本".into()),
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("等待旧程序退出超时，安装已停止".into());
                }
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    })();
    let _ = std::fs::remove_file(script);
    result
}
