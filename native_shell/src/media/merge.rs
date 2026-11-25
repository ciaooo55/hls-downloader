use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, RwLock};

pub(crate) fn hls_input_options(command: &mut Command) {
    static SUPPORT: OnceLock<Mutex<std::collections::HashMap<PathBuf, bool>>> = OnceLock::new();
    let mut cache = SUPPORT.get_or_init(Mutex::default).lock().unwrap();
    let supported = *cache
        .entry(PathBuf::from(command.get_program()))
        .or_insert_with(|| {
            let mut probe = Command::new(command.get_program());
            probe.args(["-hide_banner", "-h", "demuxer=hls"]);
            probe.stdin(Stdio::null());
            probe.stdout(Stdio::piped());
            probe.stderr(Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                probe.creation_flags(0x08000000);
            }
            probe
                .spawn()
                .and_then(|child| {
                    crate::job_object::assign_child(&child);
                    child.wait_with_output()
                })
                .is_ok_and(|output| {
                    output.status.success()
                        && (has_extension_picky(&output.stdout)
                            || has_extension_picky(&output.stderr))
                })
        });
    command.args(hls_input_args(supported));
}

fn has_extension_picky(help: &[u8]) -> bool {
    String::from_utf8_lossy(help)
        .lines()
        .any(|line| line.split_whitespace().next() == Some("-extension_picky"))
}

fn hls_input_args(extension_picky: bool) -> &'static [&'static str] {
    if extension_picky {
        &["-allowed_extensions", "ALL", "-extension_picky", "0"]
    } else {
        &["-allowed_extensions", "ALL"]
    }
}

pub fn concat_files(inputs: &[PathBuf], output: &Path) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("no segments to concatenate".into());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staged = output.with_file_name(format!(".hls-concat-{}-{stamp}.tmp", std::process::id()));
    let mut dest = fs::File::create_new(&staged).map_err(|error| error.to_string())?;
    let result = (|| {
        for input in inputs {
            let mut source = fs::File::open(input).map_err(|error| error.to_string())?;
            std::io::copy(&mut source, &mut dest).map_err(|error| error.to_string())?;
        }
        dest.sync_all().map_err(|error| error.to_string())?;
        drop(dest);
        // Keep the old output and any aliased input intact until every copy succeeds.
        crate::atomic_replace::replace_file(&staged, output).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

fn configured_ffmpeg() -> &'static RwLock<Option<PathBuf>> {
    static CONFIGURED: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
    CONFIGURED.get_or_init(|| RwLock::new(None))
}

pub fn set_configured_ffmpeg(path: Option<PathBuf>) {
    *configured_ffmpeg()
        .write()
        .unwrap_or_else(|error| error.into_inner()) = path;
}

pub fn locate_ffmpeg() -> Option<PathBuf> {
    if let Some(path) = configured_ffmpeg()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .filter(|path| path.is_file())
    {
        return Some(path.clone());
    }
    if let Ok(path) = std::env::var("HLS_FFMPEG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["ffmpeg.exe", "ffmpeg", "bin/ffmpeg.exe", "bin/ffmpeg"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    which("ffmpeg")
}

#[cfg(test)]
pub fn merge_with_ffmpeg(task_dir: &Path, output: &Path, control: &Path) -> Result<(), String> {
    merge_with_ffmpeg_opts(task_dir, output, control, true)
}

pub(crate) fn merge_with_ffmpeg_opts(
    task_dir: &Path,
    output: &Path,
    control: &Path,
    faststart: bool,
) -> Result<(), String> {
    let playlist = task_dir.join("local.m3u8");
    let ffmpeg = locate_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
    let input = if playlist.exists() {
        playlist
    } else {
        return Err("local playlist missing for ffmpeg mux".into());
    };
    let mut command = Command::new(ffmpeg);
    command.args([
        "-y",
        "-nostdin",
        "-loglevel",
        "error",
        "-protocol_whitelist",
        "file,crypto",
    ]);
    // 清单由本程序生成，仅引用已下载的本地分片；站点后缀可能是 jpeg/bin。
    hls_input_options(&mut command);
    command.arg("-i").arg(&input).args(["-c", "copy"]);
    if faststart {
        command.args(["-movflags", "+faststart"]);
    }
    run_ffmpeg_to_output(
        &mut command,
        output,
        &task_dir.join("ffmpeg-merge.log"),
        control,
    )
}

/// Rewraps a concatenated MPEG-TS stream as MP4 without re-encoding (stream
/// copy; data and bitmap-subtitle streams, which MP4 cannot carry, are dropped).
#[cfg(test)]
pub fn remux_ts_to_mp4(input: &Path, output: &Path, control: &Path) -> Result<(), String> {
    remux_ts_to_mp4_opts(input, output, control, true)
}

pub(crate) fn remux_ts_to_mp4_opts(
    input: &Path,
    output: &Path,
    control: &Path,
    faststart: bool,
) -> Result<(), String> {
    let ffmpeg = locate_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
    let mut command = Command::new(ffmpeg);
    command
        .args(["-y", "-nostdin", "-loglevel", "error", "-f", "mpegts", "-i"])
        .arg(input)
        .args(["-map", "0:v?", "-map", "0:a?", "-dn", "-sn", "-c", "copy"]);
    if faststart {
        command.args(["-movflags", "+faststart"]);
    }
    run_ffmpeg_to_output(
        &mut command,
        output,
        &output.with_extension("remux.log"),
        control,
    )
}

pub fn mux_av(
    video: &Path,
    audio: Option<&Path>,
    subtitles: &[PathBuf],
    output: &Path,
    control: &Path,
) -> Result<(), String> {
    mux_av_with_offset(video, audio, subtitles, output, control, 0.0)
}

pub(crate) fn mux_av_with_offset(
    video: &Path,
    audio: Option<&Path>,
    subtitles: &[PathBuf],
    output: &Path,
    control: &Path,
    audio_offset: f64,
) -> Result<(), String> {
    if !audio_offset.is_finite() {
        return Err("invalid audio/video presentation offset".into());
    }
    if audio.is_none() && subtitles.is_empty() {
        if video != output {
            concat_files(&[video.to_path_buf()], output)?;
        }
        return Ok(());
    }
    let ffmpeg = locate_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
    let mut command = Command::new(ffmpeg);
    command.args(["-y", "-nostdin", "-loglevel", "error"]);
    mux_input_args(&mut command, video, audio, subtitles, audio_offset);
    for arg in mux_map_args(audio.is_some(), subtitles.len()) {
        command.arg(arg);
    }
    command.args(["-c", "copy"]);
    if !subtitles.is_empty() {
        command.args(["-c:s", "mov_text"]);
    }
    command.args(["-movflags", "+faststart"]);
    run_ffmpeg_to_output(
        &mut command,
        output,
        &output.with_extension("ffmpeg.log"),
        control,
    )
}

fn mux_input_args(
    command: &mut Command,
    video: &Path,
    audio: Option<&Path>,
    subtitles: &[PathBuf],
    audio_offset: f64,
) {
    if audio.is_some() && audio_offset < 0.0 {
        command.args(["-itsoffset", &format!("{:.9}", -audio_offset)]);
    }
    command
        .args(["-protocol_whitelist", "file,crypto", "-i"])
        .arg(video);
    if let Some(audio) = audio {
        if audio_offset > 0.0 {
            command.args(["-itsoffset", &format!("{audio_offset:.9}")]);
        }
        command
            .args(["-protocol_whitelist", "file,crypto", "-i"])
            .arg(audio);
    }
    for subtitle in subtitles {
        command
            .args(["-protocol_whitelist", "file,crypto", "-i"])
            .arg(subtitle);
    }
}

pub(crate) fn run_ffmpeg_to_output(
    command: &mut Command,
    output: &Path,
    log_path: &Path,
    control: &Path,
) -> Result<(), String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut name = std::ffi::OsString::from(format!(".hls-mux-{}-{stamp}", std::process::id()));
    // FFmpeg 根据后缀选择封装格式，暂存文件必须保留原输出后缀。
    if let Some(extension) = output.extension() {
        name.push(".");
        name.push(extension);
    }
    let staged = output.with_file_name(name);
    drop(fs::File::create_new(&staged).map_err(|error| error.to_string())?);
    let result = (|| {
        run_ffmpeg(command.arg(&staged), log_path, control)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&staged)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        let action = fs::read_to_string(control).unwrap_or_default();
        match action.trim() {
            "pause" => return Err("paused".into()),
            "cancel" => return Err("canceled".into()),
            _ => {}
        }
        // 子进程已退出且暂存文件已关闭，成功前不覆盖旧成品或源轨道。
        crate::atomic_replace::replace_file(&staged, output).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

fn run_ffmpeg(command: &mut Command, log_path: &Path, control: &Path) -> Result<(), String> {
    // 写入文件避免后台 Core 的诊断管道关闭后丢失真正的合流错误。
    let log = fs::File::create(log_path).map_err(|error| error.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("ffmpeg 启动失败: {error}"))?;
    crate::job_object::assign_child(&child);
    let stderr = child.stderr.take().ok_or("ffmpeg stderr 不可用")?;
    let logging = match std::thread::Builder::new()
        .name("ffmpeg-log".into())
        .spawn(move || capture_ffmpeg_log(stderr, log))
    {
        Ok(worker) => worker,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("ffmpeg 日志读取启动失败: {error}"));
        }
    };
    let result = loop {
        let action = fs::read_to_string(control).unwrap_or_default();
        if matches!(action.trim(), "pause" | "cancel") {
            let _ = child.kill();
            let _ = child.wait();
            break Err(if action.trim() == "pause" {
                "paused"
            } else {
                "canceled"
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("ffmpeg 状态读取失败: {error}"));
            }
        }
    };
    let log_result = logging
        .join()
        .map_err(|_| "ffmpeg 日志读取线程失败".to_string())
        .and_then(|result| result.map_err(|error| error.to_string()));
    let status = result?;
    if let Err(error) = log_result {
        crate::record_runtime_log("merge_diagnostic_failed", serde_json::json!({}));
        let _ = writeln!(std::io::stderr(), "ffmpeg 日志保存失败: {error}");
    }
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

fn capture_ffmpeg_log(mut source: impl Read, mut file: fs::File) -> std::io::Result<()> {
    const LIMIT: usize = 1024 * 1024;
    const TAIL: usize = 4096;
    let mut buffer = [0; 8192];
    let mut written = 0;
    let mut tail = Vec::new();
    let mut write_error = None;
    let mut truncated = false;
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        tail.extend_from_slice(&buffer[..count]);
        if tail.len() > TAIL {
            tail.drain(..tail.len() - TAIL);
        }
        let keep = count.min(LIMIT - written);
        if keep < count {
            truncated = true;
        }
        if keep > 0 && write_error.is_none() {
            if let Err(error) = file.write_all(&buffer[..keep]) {
                write_error = Some(error);
            }
        }
        written += keep;
        // 即使日志盘写失败也持续读 stderr，不能让 FFmpeg 卡在满管道上。
    }
    if let Some(error) = write_error {
        return Err(error);
    }
    if truncated {
        let marker = b"\n[log truncated; final stderr follows]\n";
        file.seek(SeekFrom::Start((LIMIT - tail.len() - marker.len()) as u64))?;
        file.write_all(marker)?;
        file.write_all(&tail)?;
    }
    file.flush()
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
        let exe = dir.join(format!("{name}.exe"));
        if exe.is_file() {
            return Some(exe);
        }
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_ffmpeg_can_be_set_and_cleared_without_changing_environment() {
        let original_env = std::env::var_os("HLS_FFMPEG");
        let original = configured_ffmpeg().read().unwrap().clone();
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/audit-tests/ffmpeg-configured");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("ffmpeg.exe");
        std::fs::write(&path, b"configured helper fixture").unwrap();
        set_configured_ffmpeg(Some(path.clone()));
        assert_eq!(locate_ffmpeg(), Some(path.clone()));
        set_configured_ffmpeg(None);
        assert!(configured_ffmpeg().read().unwrap().is_none());
        assert_ne!(locate_ffmpeg(), Some(path));
        assert_eq!(std::env::var_os("HLS_FFMPEG"), original_env);
        set_configured_ffmpeg(original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mux_offsets_delay_only_the_later_input() {
        for (offset, expected) in [
            (
                0.0,
                vec!["-i", "video.mp4", "-i", "audio.m4a", "-i", "sub.srt"],
            ),
            (
                2.0,
                vec![
                    "-i",
                    "video.mp4",
                    "-itsoffset",
                    "2.000000000",
                    "-i",
                    "audio.m4a",
                    "-i",
                    "sub.srt",
                ],
            ),
            (
                -1.5,
                vec![
                    "-itsoffset",
                    "1.500000000",
                    "-i",
                    "video.mp4",
                    "-i",
                    "audio.m4a",
                    "-i",
                    "sub.srt",
                ],
            ),
        ] {
            let mut command = Command::new("ffmpeg");
            mux_input_args(
                &mut command,
                Path::new("video.mp4"),
                Some(Path::new("audio.m4a")),
                &[PathBuf::from("sub.srt")],
                offset,
            );
            let expected: Vec<_> = expected
                .into_iter()
                .flat_map(|arg| {
                    if arg == "-i" {
                        vec!["-protocol_whitelist", "file,crypto", arg]
                    } else {
                        vec![arg]
                    }
                })
                .collect();
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        }
        for offset in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                mux_av_with_offset(
                    Path::new("video"),
                    None,
                    &[],
                    Path::new("out"),
                    Path::new("control"),
                    offset
                )
                .unwrap_err(),
                "invalid audio/video presentation offset"
            );
        }
    }

    #[test]
    fn mux_offset_preserves_relative_stream_start_times_when_ffmpeg_is_available() {
        let Some(ffmpeg) = locate_ffmpeg() else {
            eprintln!("skipped: no FFmpeg on PATH/HLS_FFMPEG");
            return;
        };
        let root = std::env::temp_dir().join(format!("hls-mux-offset-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let video = root.join("video.mp4");
        let audio = root.join("audio.m4a");
        for (source, codec, path) in [
            ("color=size=64x48:rate=10:duration=1", "mpeg4", &video),
            ("sine=frequency=440:duration=1", "aac", &audio),
        ] {
            let generated = Command::new(&ffmpeg)
                .args([
                    "-y", "-nostdin", "-v", "error", "-f", "lavfi", "-i", source, "-c", codec,
                ])
                .arg(path)
                .output()
                .unwrap();
            assert!(
                generated.status.success(),
                "{}",
                String::from_utf8_lossy(&generated.stderr)
            );
        }
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        for offset in [-0.5, 0.0, 0.5] {
            let output = root.join(format!("mux-{offset}.mp4"));
            mux_av_with_offset(&video, Some(&audio), &[], &output, &control, offset).unwrap();
            let mut starts = Vec::new();
            for stream in ["0:v:0", "0:a:0"] {
                let decoded = Command::new(&ffmpeg)
                    .args(["-nostdin", "-v", "error", "-copyts", "-i"])
                    .arg(&output)
                    .args(["-map", stream, "-c", "copy", "-f", "framecrc", "-"])
                    .output()
                    .unwrap();
                assert!(
                    decoded.status.success(),
                    "{}",
                    String::from_utf8_lossy(&decoded.stderr)
                );
                let text = String::from_utf8(decoded.stdout).unwrap();
                let timebase = text
                    .lines()
                    .find_map(|line| line.strip_prefix("#tb 0: "))
                    .unwrap();
                let (num, den) = timebase.split_once('/').unwrap();
                let packet = text
                    .lines()
                    .find(|line| !line.starts_with('#') && !line.is_empty())
                    .unwrap();
                let pts = packet
                    .split(',')
                    .nth(2)
                    .unwrap()
                    .trim()
                    .parse::<f64>()
                    .unwrap();
                starts.push(pts * num.parse::<f64>().unwrap() / den.parse::<f64>().unwrap());
            }
            assert!(
                (starts[1] - starts[0] - offset).abs() < 0.05,
                "{offset}: {starts:?}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hls_options_match_demuxer_support() {
        assert_eq!(hls_input_args(false), ["-allowed_extensions", "ALL"]);
        assert_eq!(
            hls_input_args(true),
            ["-allowed_extensions", "ALL", "-extension_picky", "0"]
        );
    }

    #[test]
    fn detects_only_advertised_extension_picky_option() {
        assert!(has_extension_picky(
            b"hls demuxer AVOptions:\n  -extension_picky <boolean> .D......... (default true)\r\n"
        ));
        for help in [
            &b""[..],
            &b"  -allowed_extensions <string>"[..],
            &b"Unrecognized option 'extension_picky'"[..],
            &b"  -extension_picky_other <boolean>"[..],
        ] {
            assert!(!has_extension_picky(help));
        }
    }

    #[test]
    fn missing_ffmpeg_omits_unsupported_option() {
        let mut command = Command::new("/nonexistent/hls-test-ffmpeg");
        hls_input_options(&mut command);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            hls_input_args(false)
        );
    }

    #[test]
    fn hls_merge_works_with_available_ffmpeg() {
        let Some(ffmpeg) = locate_ffmpeg() else {
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "hls-option-compat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let generated = Command::new(ffmpeg)
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:a",
                "aac",
                "-f",
                "hls",
            ])
            .arg(root.join("local.m3u8"))
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let output = root.join("merged.mp4");
        merge_with_ffmpeg(&root, &output, &root.join("control")).unwrap();
        let bytes = fs::read(output).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ts_remux_produces_mp4_with_the_same_streams_when_ffmpeg_is_available() {
        let Some(ffmpeg) = locate_ffmpeg() else {
            eprintln!("skipped: no FFmpeg on PATH/HLS_FFMPEG");
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "hls-ts-remux-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("merged.ts");
        let generated = Command::new(&ffmpeg)
            .args([
                "-y",
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x48:rate=10:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:duration=1",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-c:v",
                "mpeg2video",
                "-c:a",
                "mp2",
                "-f",
                "mpegts",
            ])
            .arg(&input)
            .status()
            .unwrap();
        if !generated.success() {
            eprintln!("skipped: FFmpeg cannot synthesize the fixture");
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        let output = root.join("merged.mp4");
        remux_ts_to_mp4(&input, &output, &control).unwrap();
        let bytes = fs::read(&output).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "remuxed output is an MP4 container");
        // Decode all streams: default FFmpeg stream selection would lose the
        // second audio track despite producing a valid MP4.
        // MP4 identifies MPEG audio generically and FFmpeg chooses its float
        // MP3 decoder, while TS chooses the integer MP2 decoder. Use the same
        // decoder for this MP2 fixture so rounding is not mistaken for data loss.
        let checksums = |path: &Path| {
            let result = Command::new(&ffmpeg)
                .args(["-v", "error", "-nostdin", "-c:a", "mp2", "-i"])
                .arg(path)
                .args(["-map", "0:v", "-map", "0:a", "-f", "streamhash", "-"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap()
        };
        let expected = checksums(&input);
        assert_eq!(expected.lines().count(), 3);
        assert_eq!(checksums(&output), expected);
        assert!(root.join("merged.remux.log").is_file());
        // No staging file is left next to the output.
        let leftovers = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".hls-mux-"))
            .count();
        assert_eq!(leftovers, 0);
        // A non-TS input fails without touching an existing output.
        fs::write(&input, b"not a transport stream").unwrap();
        assert!(remux_ts_to_mp4(&input, &output, &control).is_err());
        assert_eq!(fs::read(&output).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

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
            let output = root.join("已有成品.mp4");
            fs::write(&control, "run").unwrap();
            fs::write(&output, b"previous completed media").unwrap();
            let worker_control = control.clone();
            let worker_log = log.clone();
            let worker_output = output.clone();
            let ffmpeg = ffmpeg.clone();
            let worker = std::thread::spawn(move || {
                use std::os::windows::process::CommandExt;
                let mut command = Command::new(ffmpeg);
                command.creation_flags(0x08000000).args([
                    "-y",
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
                    "-c:a",
                    "aac",
                ]);
                run_ffmpeg_to_output(&mut command, &worker_output, &worker_log, &worker_control)
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
            let elapsed = started.elapsed();
            let saved_output = fs::read(&output).unwrap();
            let files = fs::read_dir(&root).unwrap().count();
            fs::remove_dir_all(root).unwrap();
            assert!(ready, "FFmpeg did not begin processing");
            assert_eq!(result.unwrap_err(), expected);
            assert!(elapsed < std::time::Duration::from_secs(3));
            assert_eq!(saved_output, b"previous completed media");
            assert_eq!(files, 3, "staged output must be removed");
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the packaged FFmpeg; configure HLS_FFMPEG and run explicitly"]
    fn real_ffmpeg_replaces_output_only_after_success_and_supports_in_place_mux() {
        use std::os::windows::process::CommandExt;

        let ffmpeg = locate_ffmpeg().expect("configure HLS_FFMPEG to the packaged binary");
        let root = std::env::temp_dir().join(format!(
            "hls-real-publish-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("已有成品.mp4");
        let log = output.with_extension("ffmpeg.log");
        let control = root.join("control");
        fs::write(&control, "run").unwrap();
        for fail in [true, false] {
            fs::write(&output, b"previous completed media").unwrap();
            let mut command = Command::new(&ffmpeg);
            command.creation_flags(0x08000000).args([
                "-y",
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=8000",
                "-t",
                "0.1",
            ]);
            if fail {
                command.args(["-c:a", "pcm_s16le", "-f", "webm"]);
            } else {
                command.args(["-c:a", "aac"]);
            }
            let result = run_ffmpeg_to_output(&mut command, &output, &log, &control);
            if fail {
                assert!(result.unwrap_err().contains("ffmpeg 合流失败"));
                assert_eq!(fs::read(&output).unwrap(), b"previous completed media");
                assert!(!fs::read(&log).unwrap().is_empty());
            } else {
                result.unwrap();
                // 两个输入与输出是同一路径，旧文件必须一直可读到 FFmpeg 退出。
                mux_av(&output, Some(&output), &[], &output, &control).unwrap();
                let decoded = Command::new(&ffmpeg)
                    .creation_flags(0x08000000)
                    .args(["-nostdin", "-v", "error", "-i"])
                    .arg(&output)
                    .args(["-f", "null", "-"])
                    .output()
                    .unwrap();
                assert!(
                    decoded.status.success(),
                    "{}",
                    String::from_utf8_lossy(&decoded.stderr)
                );
            }
            assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        }
        fs::remove_dir_all(root).unwrap();
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
    fn concat_reads_the_previous_output_without_growing_its_own_input() {
        let root = std::env::temp_dir().join(format!("hls-concat-alias-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("input.ts");
        let output = root.join("output.ts");
        fs::write(&input, b"input").unwrap();
        fs::write(&output, b"old-output").unwrap();
        let result = concat_files(&[input.clone(), output.clone()], &output);
        let saved_input = fs::read(&input).unwrap();
        let combined = fs::read(&output).unwrap();
        let files = fs::read_dir(&root).unwrap().count();
        fs::remove_dir_all(root).unwrap();
        result.unwrap();
        assert_eq!(combined, b"inputold-output");
        assert_eq!(saved_input, b"input");
        assert_eq!(files, 2);
    }

    #[test]
    fn concat_failure_preserves_existing_output_and_inputs() {
        let root = std::env::temp_dir().join(format!("hls-concat-failure-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("input.ts");
        let output = root.join("output.ts");
        fs::write(&input, b"input").unwrap();
        fs::write(&output, b"old-output").unwrap();
        let failure = concat_files(&[input.clone(), root.join("missing.ts")], &output);
        let saved_output = fs::read(&output).unwrap();
        let saved_input = fs::read(&input).unwrap();
        let files = fs::read_dir(&root).unwrap().count();
        fs::remove_dir_all(root).unwrap();
        assert!(failure.is_err());
        assert_eq!(saved_output, b"old-output");
        assert_eq!(saved_input, b"input");
        assert_eq!(files, 2);
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
    fn verbose_merge_log_is_bounded_and_retains_the_final_error() {
        let path = std::env::temp_dir().join(format!("hls-merge-log-{}.txt", std::process::id()));
        let mut source = vec![b'x'; 2 * 1024 * 1024];
        source.extend_from_slice(b"final decoder failure\n");
        capture_ffmpeg_log(
            std::io::Cursor::new(source),
            fs::File::create(&path).unwrap(),
        )
        .unwrap();
        let log = fs::read(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(log.len(), 1024 * 1024);
        assert!(log.ends_with(b"final decoder failure\n"));
        assert!(String::from_utf8(log).unwrap().contains("log truncated"));
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
    fn mux_without_extra_tracks_preserves_an_aliased_input() {
        let root = std::env::temp_dir().join(format!("hls-mux-alias-{}", std::process::id()));
        fs::create_dir_all(root.join("tracks")).unwrap();
        let video = root.join("video.bin");
        let output = root.join("tracks").join("..").join("video.bin");
        fs::write(&video, b"VIDEO").unwrap();
        mux_av(&video, None, &[], &output, &root.join("control")).unwrap();
        let saved = fs::read(&video).unwrap();
        let files = fs::read_dir(&root).unwrap().count();
        fs::remove_dir_all(root).unwrap();
        assert_eq!(saved, b"VIDEO");
        assert_eq!(files, 2);
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
