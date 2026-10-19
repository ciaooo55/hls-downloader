use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn concat_files(inputs: &[PathBuf], output: &Path) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("no segments to concatenate".into());
    }
    let mut dest = fs::File::create(output).map_err(|error| error.to_string())?;
    for input in inputs {
        let mut source = fs::File::open(input).map_err(|error| error.to_string())?;
        std::io::copy(&mut source, &mut dest).map_err(|error| error.to_string())?;
    }
    dest.flush().map_err(|error| error.to_string())
}

pub fn locate_ffmpeg() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("HLS_FFMPEG") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["ffmpeg.exe", "ffmpeg", "bin/ffmpeg.exe", "bin/ffmpeg"] {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    which("ffmpeg")
}

pub fn merge_with_ffmpeg(task_dir: &Path, output: &Path, control: &Path) -> Result<(), String> {
    let playlist = task_dir.join("local.m3u8");
    let ffmpeg = locate_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
    let input = if playlist.exists() {
        playlist
    } else {
        return Err("local playlist missing for ffmpeg mux".into());
    };
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-y",
            "-nostdin",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file,crypto",
            // 清单由本程序生成，仅引用已下载的本地分片；站点后缀可能是 jpeg/bin。
            "-allowed_extensions",
            "ALL",
            "-extension_picky",
            "0",
            "-i",
        ])
        .arg(&input)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(output);
    run_ffmpeg(&mut command, &task_dir.join("ffmpeg-merge.log"), control)
}

pub fn mux_av(
    video: &Path,
    audio: Option<&Path>,
    subtitles: &[PathBuf],
    output: &Path,
    control: &Path,
) -> Result<(), String> {
    if audio.is_none() && subtitles.is_empty() {
        if video != output {
            fs::copy(video, output).map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    let ffmpeg = locate_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-y",
            "-nostdin",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file,crypto",
            "-i",
        ])
        .arg(video);
    if let Some(audio) = audio {
        command.args(["-i"]).arg(audio);
    }
    for subtitle in subtitles {
        command.args(["-i"]).arg(subtitle);
    }
    for arg in mux_map_args(audio.is_some(), subtitles.len()) {
        command.arg(arg);
    }
    command.args(["-c", "copy"]);
    if !subtitles.is_empty() {
        command.args(["-c:s", "mov_text"]);
    }
    command.args(["-movflags", "+faststart"]).arg(output);
    run_ffmpeg(&mut command, &output.with_extension("ffmpeg.log"), control)
}

fn run_ffmpeg(command: &mut Command, log_path: &Path, control: &Path) -> Result<(), String> {
    // 写入文件避免后台 Core 的诊断管道关闭后丢失真正的合流错误。
    let log = fs::File::create(log_path).map_err(|error| error.to_string())?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("ffmpeg 启动失败: {error}"))?;
    let status = loop {
        let action = fs::read_to_string(control).unwrap_or_default();
        if matches!(action.trim(), "pause" | "cancel") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if action.trim() == "pause" {
                "paused"
            } else {
                "canceled"
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("ffmpeg 状态读取失败: {error}"));
            }
        }
    };
    if status.success() {
        Ok(())
    } else {
        let mut log = fs::File::open(log_path).map_err(|error| error.to_string())?;
        let size = log.metadata().map_err(|error| error.to_string())?.len();
        log.seek(SeekFrom::Start(size.saturating_sub(4096)))
            .map_err(|error| error.to_string())?;
        let mut tail = Vec::new();
        log.read_to_end(&mut tail)
            .map_err(|error| error.to_string())?;
        Err(format!(
            "ffmpeg 合流失败 ({status}): {}；完整日志: {}",
            String::from_utf8_lossy(&tail).trim(),
            log_path.display()
        ))
    }
}

fn mux_map_args(has_external_audio: bool, subtitle_count: usize) -> Vec<String> {
    let mut args = vec!["-map".into(), "0:v:0?".into(), "-map".into()];
    if has_external_audio {
        args.push("1:a:0".into());
    } else {
        args.push("0:a:0?".into());
    }
    let first_subtitle_input = if has_external_audio { 2 } else { 1 };
    for index in 0..subtitle_count {
        args.push("-map".into());
        args.push(format!("{}:s:0", first_subtitle_input + index));
    }
    args
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        let exe = dir.join(format!("{name}.exe"));
        if exe.exists() {
            return Some(exe);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the packaged FFmpeg; run explicitly in Windows Sandbox"]
    fn real_ffmpeg_pause_and_cancel_reap_the_child() {
        let ffmpeg = locate_ffmpeg().expect("configure HLS_FFMPEG to the packaged binary");
        for (action, expected) in [("pause", "paused"), ("cancel", "canceled")] {
            let root = std::env::temp_dir().join(format!(
                "hls-real-merge-{}-{}-{action}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            let control = root.join("control");
            let log = root.join("merge.log");
            fs::write(&control, "run").unwrap();
            let worker_control = control.clone();
            let worker_log = log.clone();
            let ffmpeg = ffmpeg.clone();
            let worker = std::thread::spawn(move || {
                use std::os::windows::process::CommandExt;
                let mut command = Command::new(ffmpeg);
                command.creation_flags(0x08000000).args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-re",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=8000",
                    "-t",
                    "30",
                    "-progress",
                    "pipe:2",
                    "-stats_period",
                    "0.1",
                    "-f",
                    "null",
                    "NUL",
                ]);
                run_ffmpeg(&mut command, &worker_log, &worker_control)
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let ready = loop {
                if fs::read_to_string(&log)
                    .unwrap_or_default()
                    .contains("out_time_us=")
                {
                    break true;
                }
                if worker.is_finished() || std::time::Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let started = std::time::Instant::now();
            fs::write(&control, action).unwrap();
            let result = worker.join().unwrap();
            fs::remove_dir_all(root).unwrap();
            assert!(ready, "FFmpeg did not begin processing");
            assert_eq!(result.unwrap_err(), expected);
            assert!(started.elapsed() < std::time::Duration::from_secs(3));
        }
    }

    #[cfg(windows)]
    #[test]
    fn merging_pause_and_cancel_stop_the_running_child() {
        for (action, expected) in [("pause", "paused"), ("cancel", "canceled")] {
            let root = std::env::temp_dir().join(format!(
                "hls-merge-control-{}-{}-{action}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            let control = root.join("control");
            let log = root.join("merge.log");
            fs::write(&control, "run").unwrap();
            let worker_control = control.clone();
            let worker_log = log.clone();
            let worker = std::thread::spawn(move || {
                use std::os::windows::process::CommandExt;
                let mut command = Command::new("powershell.exe");
                command.creation_flags(0x08000000).args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[Console]::Error.WriteLine('fixture-ready'); [Threading.Thread]::Sleep(30000)",
                ]);
                run_ffmpeg(&mut command, &worker_log, &worker_control)
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let ready = loop {
                if fs::read_to_string(&log)
                    .unwrap_or_default()
                    .contains("fixture-ready")
                {
                    break true;
                }
                if worker.is_finished() || std::time::Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            fs::write(&control, action).unwrap();
            let result = worker.join().unwrap();
            fs::remove_dir_all(root).unwrap();
            assert!(ready, "merge fixture did not start");
            assert_eq!(result.unwrap_err(), expected);
        }
    }

    #[test]
    fn concat_preserves_bytes() {
        let dir = std::env::temp_dir().join(format!("hls-concat-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.ts");
        let b = dir.join("b.ts");
        let out = dir.join("out.ts");
        fs::write(&a, b"AAA").unwrap();
        fs::write(&b, b"BBB").unwrap();
        concat_files(&[a, b], &out).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"AAABBB");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mux_without_extra_tracks_copies_video() {
        let dir = std::env::temp_dir().join(format!("hls-mux-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let video = dir.join("video.bin");
        let output = dir.join("out.bin");
        fs::write(&video, b"VIDEO").unwrap();
        mux_av(&video, None, &[], &output, &dir.join("control")).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"VIDEO");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mux_maps_every_selected_track() {
        assert_eq!(
            mux_map_args(false, 2),
            vec!["-map", "0:v:0?", "-map", "0:a:0?", "-map", "1:s:0", "-map", "2:s:0"]
        );
        assert_eq!(
            mux_map_args(true, 2),
            vec!["-map", "0:v:0?", "-map", "1:a:0", "-map", "2:s:0", "-map", "3:s:0"]
        );
    }
}
