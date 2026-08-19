//! Shared HLS / DASH / live HTTP fixtures for the phase-3 media gate.
//!
//! Playlist parsing stays in the engine modules. This harness is the one
//! loopback origin those engines must download through.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use super::{download_dash, download_hls, download_hls_with, HlsDownloadOptions};

#[derive(Clone)]
pub struct FixtureOrigin {
    pub base: String,
    #[allow(dead_code)]
    pub requests: Arc<Mutex<Vec<String>>>,
    #[allow(dead_code)]
    pub body_bytes: Arc<AtomicU64>,
}

pub fn serve_files(files: HashMap<String, Vec<u8>>) -> FixtureOrigin {
    serve_dynamic(Arc::new(move |path: &str, _hit: usize| files.get(path).cloned()))
}

pub fn serve_dynamic(
    resolve: Arc<dyn Fn(&str, usize) -> Option<Vec<u8>> + Send + Sync>,
) -> FixtureOrigin {
    let listener = TcpListener::bind("127.0.0.1:0").expect("media fixture bind");
    let addr = listener.local_addr().expect("media fixture addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let body_bytes = Arc::new(AtomicU64::new(0));
    let seen = Arc::clone(&requests);
    let bytes = Arc::clone(&body_bytes);
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let count = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..count]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .split('?')
                .next()
                .unwrap_or("/")
                .to_string();
            let hit = {
                let mut list = seen.lock().unwrap_or_else(|err| err.into_inner());
                list.push(path.clone());
                list.iter().filter(|item| item.as_str() == path).count()
            };
            let body = resolve(&path, hit).unwrap_or_else(|| b"missing".to_vec());
            bytes.fetch_add(body.len() as u64, Ordering::SeqCst);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    FixtureOrigin {
        base: format!("http://127.0.0.1:{}", addr.port()),
        requests,
        body_bytes,
    }
}

pub fn run_hls_vod_fixture() -> Result<Vec<u8>, String> {
    let mut files = HashMap::new();
    files.insert(
        "/index.m3u8".into(),
        b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\na.ts\n#EXTINF:1,\nb.ts\n#EXT-X-ENDLIST\n"
            .to_vec(),
    );
    files.insert("/a.ts".into(), b"AAA".to_vec());
    files.insert("/b.ts".into(), b"BBB".to_vec());
    let origin = serve_files(files);
    let dir = std::env::temp_dir().join(format!("hls-harness-vod-{}", std::process::id()));
    let control = dir.join("control");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    std::fs::write(&control, "run").map_err(|error| error.to_string())?;
    let merged = download_hls(
        &format!("{}/index.m3u8", origin.base),
        &HashMap::new(),
        "",
        &dir,
        &control,
        false,
    )?;
    let bytes = std::fs::read(&merged).map_err(|error| error.to_string())?;
    let playlist = std::fs::read_to_string(dir.join("local.m3u8")).map_err(|error| error.to_string())?;
    if !playlist.contains("#EXT-X-ENDLIST") || !playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD") {
        return Err("HLS VOD local playlist missing VOD markers".into());
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(bytes)
}

pub fn run_hls_live_fixture() -> Result<(Vec<u8>, String), String> {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let origin = serve_dynamic(Arc::new(move |path: &str, _hit: usize| {
        if path.ends_with(".m3u8") {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Some(b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nlive0.ts\n".to_vec())
            } else {
                Some(
                    b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nlive0.ts\n#EXTINF:1,\nlive1.ts\n#EXT-X-ENDLIST\n"
                        .to_vec(),
                )
            }
        } else if path.ends_with("live0.ts") {
            Some(b"L0".to_vec())
        } else {
            Some(b"L1".to_vec())
        }
    }));
    let dir = std::env::temp_dir().join(format!("hls-harness-live-{}", std::process::id()));
    let control = dir.join("control");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    std::fs::write(&control, "run").map_err(|error| error.to_string())?;
    let merged = download_hls_with(
        &format!("{}/live.m3u8", origin.base),
        &HashMap::new(),
        "",
        &dir,
        &control,
        HlsDownloadOptions {
            live: true,
            live_max_minutes: 1,
            ..HlsDownloadOptions::default()
        },
    )?;
    let bytes = std::fs::read(&merged).map_err(|error| error.to_string())?;
    let playlist = std::fs::read_to_string(dir.join("local.m3u8")).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_dir_all(dir);
    Ok((bytes, playlist))
}

pub fn run_dash_static_fixture() -> Result<Vec<u8>, String> {
    let mut files = HashMap::new();
    files.insert(
        "/manifest.mpd".into(),
        br#"<MPD type="static"><Period><AdaptationSet><Representation id="v" bandwidth="1000" mimeType="video/mp4"><BaseURL>/</BaseURL><SegmentList><SegmentURL media="init.mp4"/><SegmentURL media="1.m4s"/></SegmentList></Representation></AdaptationSet></Period></MPD>"#.to_vec(),
    );
    files.insert("/init.mp4".into(), b"INIT".to_vec());
    files.insert("/1.m4s".into(), b"DASH".to_vec());
    let origin = serve_files(files);
    let dir = std::env::temp_dir().join(format!("dash-harness-{}", std::process::id()));
    let control = dir.join("control");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    std::fs::write(&control, "run").map_err(|error| error.to_string())?;
    let merged = download_dash(
        &format!("{}/manifest.mpd", origin.base),
        &HashMap::new(),
        "",
        &dir,
        &control,
    )?;
    let bytes = std::fs::read(&merged).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_dir_all(dir);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_vod_dash_and_live_share_one_origin_harness() {
        assert_eq!(run_hls_vod_fixture().unwrap(), b"AAABBB");
        assert_eq!(run_dash_static_fixture().unwrap(), b"INITDASH");
        let (live, playlist) = run_hls_live_fixture().unwrap();
        assert!(live == b"L0L1" || live == b"L0", "live bytes {live:?}");
        assert!(
            playlist.contains("#EXT-X-PLAYLIST-TYPE:EVENT")
                || playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD")
        );
        if live == b"L0L1" {
            assert!(playlist.contains("#EXT-X-ENDLIST"));
        }
    }
}
