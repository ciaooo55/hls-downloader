use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn concat_files(inputs: &[PathBuf], output: &Path) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("no segments to concatenate".into());
    }
    let mut dest = fs::File::create(output).map_err(|error| error.to_string())?;
    for input in inputs {
        let bytes = fs::read(input).map_err(|error| error.to_string())?;
        dest.write_all(&bytes).map_err(|error| error.to_string())?;
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

pub fn merge_with_ffmpeg(task_dir: &Path, output: &Path) -> Result<(), String> {
    let playlist = task_dir.join("local.m3u8");
    let ffmpeg = locate_ffmpeg().ok_or_else(|| "ffmpeg not found".to_string())?;
    let input = if playlist.exists() {
        playlist
    } else {
        return Err("local playlist missing for ffmpeg mux".into());
    };
    let status = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file,crypto",
            "-i",
        ])
        .arg(&input)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(output)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg exited with {status}"))
    }
}

pub fn mux_av(
    video: &Path,
    audio: Option<&Path>,
    subtitles: &[PathBuf],
    output: &Path,
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
    command.args(["-c", "copy"]);
    if !subtitles.is_empty() {
        command.args(["-c:s", "mov_text"]);
    }
    command.args(["-movflags", "+faststart"]).arg(output);
    let status = command.status().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg mux exited with {status}"))
    }
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
        mux_av(&video, None, &[], &output).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"VIDEO");
        let _ = fs::remove_dir_all(dir);
    }
}
