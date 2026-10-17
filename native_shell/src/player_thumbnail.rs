//! On-demand thumbnails use the existing FFmpeg binary; nothing is written to disk.
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use base64::Engine;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static DECODER: Mutex<()> = Mutex::new(());

pub(crate) fn cancel() { GENERATION.fetch_add(1, Ordering::SeqCst); }

// 删除分片前等待解码进程退出，并禁止新解码在清理中途打开旧文件。
pub(crate) fn cancel_and_wait() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    cancel();
    DECODER.lock().map_err(|_| "缩略图解码锁已失效".into())
}

pub(crate) fn decode(source: &str, seconds: f64) -> Result<String, String> {
    if !seconds.is_finite() || seconds < 0.0 { return Err("预览时间无效".into()); }
    let epoch = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let lock_deadline = Instant::now() + Duration::from_millis(500);
    let _decoder = loop {
        if GENERATION.load(Ordering::SeqCst) != epoch { return Err("预览已取消".into()); }
        if let Ok(guard) = DECODER.try_lock() { break guard; }
        if Instant::now() >= lock_deadline { return Err("上一张预览正在结束".into()); }
        std::thread::sleep(Duration::from_millis(25));
    };
    let ffmpeg = crate::media::locate_ffmpeg().ok_or("找不到 FFmpeg")?;
    let mut command = Command::new(ffmpeg);
    command.args(["-nostdin", "-loglevel", "error", "-threads", "1", "-protocol_whitelist", "file,http,tcp,crypto"]);
    if source.starts_with("http://") {
        // 悬停只读已下载内容，不抢占主播放器的分片或 Range 下载优先级。
        command.args(["-headers", "X-HLS-Thumbnail: 1\r\n"]);
    }
    if source.ends_with("/preview.m3u8") {
        command.args(["-allowed_extensions", "ALL", "-extension_picky", "0"]);
    }
    command.arg("-ss")
        .arg(format!("{seconds:.3}"))
        .arg("-i").arg(source)
        .args(["-an", "-sn", "-dn", "-frames:v", "1", "-vf", "scale=184:104:force_original_aspect_ratio=decrease", "-c:v", "mjpeg", "-threads", "1", "-f", "image2pipe", "pipe:1"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().ok_or("缩略图管道不可用")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(128 * 1024 + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if GENERATION.load(Ordering::SeqCst) != epoch { break Err("预览已取消".to_string()); }
        if Instant::now() >= deadline { break Err("预览暂未就绪".to_string()); }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(_)) => break Err("当前位置尚无可解码画面".into()),
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => break Err(error.to_string()),
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    let bytes = reader.join().map_err(|_| "缩略图读取失败")?.map_err(|error| error.to_string())?;
    result?;
    if bytes.len() > 128 * 1024 || !bytes.starts_with(&[0xff, 0xd8]) || !bytes.ends_with(&[0xff, 0xd9]) {
        return Err("当前位置没有有效缩略图".into());
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}
