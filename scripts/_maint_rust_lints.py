from pathlib import Path

ROOT = Path("native_shell/src")

CHANGES = {
    "torrent_engine.rs": [
        ("0x4172_7101_980u64", "0x0417_2710_1980_u64"),
        ("for chunk in raw.chunks_exact(20) {", "for chunk in raw.as_chunks::<20>().0 {"),
        ("fetch_magnet_metadata(&magnet, headers, proxy, enable_dht).or_else(|_| Ok(magnet))", "fetch_magnet_metadata(&magnet, headers, proxy, enable_dht).or(Ok(magnet))"),
        ("raw.chunks_exact(26)\n", "raw.as_chunks::<26>().0.iter()\n"),
        ("raw.chunks_exact(6)\n", "raw.as_chunks::<6>().0.iter()\n"),
    ],
    "cast.rs": [
        ("proto_varint(((field as u64) << 3) | 0)", "proto_varint((field as u64) << 3)"),
    ],
    "checksum.rs": [
        ("rest.replace(':', \"\").replace(' ', \"\")", "rest.replace([':', ' '], \"\")"),
        ("chunk.chunks_exact(4).enumerate()", "chunk.as_chunks::<4>().0.iter().enumerate()"),
        ("u32::from_le_bytes(part.try_into().unwrap())", "u32::from_le_bytes(*part)"),
    ],
    "core_ipc.rs": [
        ("let first = match self.serve_once_inner(Some(&ready)) {\n            Ok(stream) => stream,\n            Err(error) => return Err(error),\n        };", "let first = self.serve_once_inner(Some(&ready))?;"),
        ("let mut attrs = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {", "let attrs = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {"),
        ("                &mut attrs,", "                &attrs,"),
    ],
    "credentials.rs": [
        ("return protect_windows(value.as_bytes());", "protect_windows(value.as_bytes())"),
        ("return unprotect_windows(&decode_hex(&value[PREFIX.len()..])?);", "unprotect_windows(&decode_hex(&value[PREFIX.len()..])?)"),
        ("value.len() % 2 != 0", "!value.len().is_multiple_of(2)"),
        ("value.as_bytes().chunks_exact(2)", "value.as_bytes().as_chunks::<2>().0"),
        ("headers.get(\"Cookie\").is_none()", "!headers.contains_key(\"Cookie\")"),
        ("headers.get(\"Authorization\").is_none()", "!headers.contains_key(\"Authorization\")"),
        ("headers.get(\"Referer\").is_none()", "!headers.contains_key(\"Referer\")"),
        ("headers.get(\"Origin\").is_none()", "!headers.contains_key(\"Origin\")"),
    ],
    "crypto_lite.rs": [
        ("data.len() % 16 != 0", "!data.len().is_multiple_of(16)"),
        ("data.chunks_exact(16)", "data.as_chunks::<16>().0"),
        ("padded.chunks_exact(64)", "padded.as_chunks::<64>().0"),
        ("chunk.chunks_exact(4).enumerate()", "chunk.as_chunks::<4>().0.iter().enumerate()"),
        ("u32::from_be_bytes(part.try_into().unwrap())", "u32::from_be_bytes(*part)"),
    ],
    "download_worker.rs": [
        ("if owner == claimant && presentation == \"presenting\" {\n                        \"presented\"\n                    } else if presenter_id.is_empty() {\n                        \"presented\"", "if (owner == claimant && presentation == \"presenting\") || presenter_id.is_empty() {\n                        \"presented\""),
        ("((max_bytes + 2) / 3) * 4 + 4", "max_bytes.div_ceil(3) * 4 + 4"),
        ("crate::write_clipboard_files(&[published.clone()])?", "crate::write_clipboard_files(std::slice::from_ref(&published))?"),
        ("let mut info = format!(\n            \"d5:filesld6:lengthi4e4:pathl7:one.bineed6:lengthi4e4:pathl7:two.bineee4:name4:demo12:piece lengthi4e6:pieces40:\"\n        )", "let mut info = \"d5:filesld6:lengthi4e4:pathl7:one.bineed6:lengthi4e4:pathl7:two.bineee4:name4:demo12:piece lengthi4e6:pieces40:\".to_string()"),
        ("other.headers.get(\"Cookie\").is_none()", "!other.headers.contains_key(\"Cookie\")"),
        ("other.headers.get(\"Authorization\").is_none()", "!other.headers.contains_key(\"Authorization\")"),
        ("spec.headers.get(\"Cookie\").is_none()", "!spec.headers.contains_key(\"Cookie\")"),
    ],
    "drop_target.rs": [
        ("return install(title, tx, Box::new(wake));", "install(title, tx, Box::new(wake))"),
    ],
    "http_engine.rs": [
        ("let Some(items) = value.get(\"ranges\").and_then(|item| item.as_array()) else {\n        return None;\n    };", "let items = value.get(\"ranges\").and_then(|item| item.as_array())?;"),
        ("_file.as_raw_handle() as *mut core::ffi::c_void", "_file.as_raw_handle()"),
        ("pending.iter().copied().collect()", "pending.to_vec()"),
        (".iter()\n            .any(|start| *start == 3)", ".contains(&3)"),
    ],
    "instance.rs": [
        ("file.as_raw_handle() as *mut core::ffi::c_void", "file.as_raw_handle()"),
    ],
    "link_file.rs": [
        ("if cfg!(windows) {\n        PathBuf::from(decoded)\n    } else if decoded.starts_with('/') {\n        PathBuf::from(decoded)", "if cfg!(windows) || decoded.starts_with('/') {\n        PathBuf::from(decoded)"),
        (".chunks_exact(2)", ".as_chunks::<2>().0.iter()"),
        ("trim_end_matches(|ch| matches!(ch, '.' | ',' | ')' | ';' | ']'))", "trim_end_matches(['.', ',', ')', ';', ']'])"),
    ],
    "media/dash.rs": [
        ("fn select_video<'a>(\n    representations: &'a [Representation],\n    preferred_bandwidth: u64,\n) -> Option<&'a Representation> {", "fn select_video(\n    representations: &[Representation],\n    preferred_bandwidth: u64,\n) -> Option<&Representation> {"),
        (".find(|ch: char| ch == ' ' || ch == '/' || ch == '>')", ".find([' ', '/', '>'])"),
    ],
    "media/hls.rs": [
        ("hex.len() % 2 != 0", "!hex.len().is_multiple_of(2)"),
        ("&[file.clone()]", "std::slice::from_ref(&file)"),
    ],
    "media/subtitles.rs": [
        (".split(|ch: char| ch == '.' || ch == ' ' || ch == '\\t')", ".split(['.', ' ', '\\t'])"),
    ],
    "net_policy.rs": [
        ("seconds.max(1).min(60)", "seconds.clamp(1, 60)"),
    ],
    "playback.rs": [
        ("json_escape(&token)", "json_escape(token)"),
        (".replace('\\r', \"\")\n        .replace('\\n', \"\")", ".replace(['\\r', '\\n'], \"\")"),
    ],
    "player.rs": [
        ("enum Backend {\n    Idle,", "#[derive(Default)]\nenum Backend {\n    #[default]\n    Idle,"),
        ("impl Default for Backend {\n    fn default() -> Self {\n        Self::Idle\n    }\n}\n\n", ""),
        ("speed.max(0.25).min(4.0)", "speed.clamp(0.25, 4.0)"),
        ("if let Some(payload) = payload {\n            if let serde_json::Value::Object(fields) = payload {\n                request.extend(fields);\n            }\n        }", "if let Some(serde_json::Value::Object(fields)) = payload {\n            request.extend(fields);\n        }"),
        ("fn load_libmpv_session(wid: Option<i64>) -> Result<MpvSession, String> {", "#[allow(clippy::manual_c_str_literals)]\nfn load_libmpv_session(wid: Option<i64>) -> Result<MpvSession, String> {"),
    ],
    "store.rs": [
        ("limit.max(1).min(500)", "limit.clamp(1, 500)"),
        ("settings.map_or(true, BTreeMap::is_empty)", "settings.is_none_or(BTreeMap::is_empty)"),
    ],
    "task_export.rs": [
        ("task.resource_kind.clone()", "task.resource_kind"),
        ("serde_json::to_value(&task.resource_kind)", "serde_json::to_value(task.resource_kind)"),
    ],
    "ole_drag.rs": [
        ("&[path.clone()]", "std::slice::from_ref(&path)"),
        (".chunks_exact(2)", ".as_chunks::<2>().0.iter()"),
    ],
    "curl_import.rs": [
        ("parsed.headers.get(\"x-a\").is_none()", "!parsed.headers.contains_key(\"x-a\")"),
    ],
}


def replace_required(path: Path, old: str, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    if old not in text:
        raise SystemExit(f"expected pattern missing in {path}: {old!r}")
    path.write_text(text.replace(old, new), encoding="utf-8", newline="\n")


for relative, replacements in CHANGES.items():
    path = ROOT / relative
    for old, new in replacements:
        replace_required(path, old, new)

hls_path = ROOT / "media/hls.rs"
hls_text = hls_path.read_text(encoding="utf-8")
old_use = """        use std::net::TcpListener;\n        use std::thread;\n"""
new_use = """        use std::net::TcpListener;\n        use std::sync::mpsc;\n        use std::thread;\n"""
if old_use not in hls_text:
    raise SystemExit("authenticated live HLS test imports changed unexpectedly")
hls_text = hls_text.replace(old_use, new_use, 1)
old_spawn = """        let first = thread::spawn(move || {\n            download_hls_with(\n                &first_url,\n                &first_headers,\n                \"\",\n                &first_dir,\n                &first_control,\n                HlsDownloadOptions {\n                    live: true,\n                    concurrency: 1,\n                    download_subtitles: false,\n                    ..HlsDownloadOptions::default()\n                },\n            )\n        });\n        let deadline = Instant::now() + Duration::from_secs(15);\n        while !first_segment_seen.load(Ordering::SeqCst) {\n            assert!(\n                Instant::now() < deadline,\n                \"server did not receive the first live segment\"\n            );\n            thread::sleep(Duration::from_millis(5));\n        }\n"""
new_spawn = """        let (started_tx, started_rx) = mpsc::sync_channel(1);\n        let (done_tx, done_rx) = mpsc::sync_channel(1);\n        let first = thread::spawn(move || {\n            let _ = started_tx.send(());\n            let result = download_hls_with(\n                &first_url,\n                &first_headers,\n                \"\",\n                &first_dir,\n                &first_control,\n                HlsDownloadOptions {\n                    live: true,\n                    concurrency: 1,\n                    download_subtitles: false,\n                    ..HlsDownloadOptions::default()\n                },\n            );\n            let _ = done_tx.send(result);\n        });\n        started_rx\n            .recv_timeout(Duration::from_secs(15))\n            .expect(\"live download worker did not start\");\n        let deadline = Instant::now() + Duration::from_secs(15);\n        while !first_segment_seen.load(Ordering::SeqCst) {\n            match done_rx.try_recv() {\n                Ok(result) => panic!(\"live download exited before the first segment: {result:?}\"),\n                Err(mpsc::TryRecvError::Disconnected) => {\n                    panic!(\"live download result channel disconnected before the first segment\")\n                }\n                Err(mpsc::TryRecvError::Empty) => {}\n            }\n            assert!(\n                Instant::now() < deadline,\n                \"server did not receive the first live segment after worker startup\"\n            );\n            thread::sleep(Duration::from_millis(5));\n        }\n"""
if old_spawn not in hls_text:
    raise SystemExit("authenticated live HLS worker block changed unexpectedly")
hls_text = hls_text.replace(old_spawn, new_spawn, 1)
old_join = "        assert_eq!(first.join().unwrap().unwrap_err(), \"paused\");"
new_join = """        let first_result = done_rx\n            .recv_timeout(Duration::from_secs(15))\n            .expect(\"paused live download did not finish\");\n        assert_eq!(first_result.unwrap_err(), \"paused\");\n        first.join().unwrap();"""
if old_join not in hls_text:
    raise SystemExit("authenticated live HLS join assertion changed unexpectedly")
hls_path.write_text(hls_text.replace(old_join, new_join, 1), encoding="utf-8", newline="\n")
