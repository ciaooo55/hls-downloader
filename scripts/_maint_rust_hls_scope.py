from pathlib import Path

path = Path("native_shell/src/media/hls.rs")
text = path.read_text(encoding="utf-8")

# The general migration script intentionally stays simple; scope its test-only
# synchronization edits to the authenticated live test here so sibling VOD
# tests remain byte-for-byte unchanged apart from rustfmt.
misplaced_import = "        use std::net::TcpListener;\n        use std::sync::mpsc;\n        use std::thread;\n"
plain_import = "        use std::net::TcpListener;\n        use std::thread;\n"
if misplaced_import not in text:
    raise SystemExit("expected temporary mpsc import was not produced")
text = text.replace(misplaced_import, plain_import, 1)

new_join = """        let first_result = done_rx
            .recv_timeout(Duration::from_secs(15))
            .expect("paused live download did not finish");
        assert_eq!(first_result.unwrap_err(), "paused");
        first.join().unwrap();"""
old_join = "        assert_eq!(first.join().unwrap().unwrap_err(), \"paused\");"
if new_join not in text:
    raise SystemExit("expected temporary result-channel join was not produced")
text = text.replace(new_join, old_join, 1)

marker = "    fn authenticated_live_pause_resume_restores_atomic_timeline() {"
start = text.find(marker)
if start < 0:
    raise SystemExit("authenticated live HLS test not found")
end = text.find("\n    #[test]", start + len(marker))
if end < 0:
    end = len(text)
block = text[start:end]

if plain_import not in block:
    raise SystemExit("live HLS test imports changed unexpectedly")
block = block.replace(
    plain_import,
    "        use std::net::TcpListener;\n        use std::sync::mpsc;\n        use std::thread;\n",
    1,
)
if old_join not in block:
    raise SystemExit("live HLS join assertion changed unexpectedly")
block = block.replace(old_join, new_join, 1)

text = text[:start] + block + text[end:]
path.write_text(text, encoding="utf-8", newline="\n")
