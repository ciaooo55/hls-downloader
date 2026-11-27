//! Boundary regressions for shared client-side request validation.
use hls_client::request_validation::{
    canonical_resource_key, http_fetch_url_allowed, remote_resource_url_allowed,
    reserved_dos_device_name, sanitize_http_method, validate_local_source_path,
};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[test]
fn reserved_device_names_match_case_extensions_and_superscript_ports() {
    for name in [
        "CON",
        "con",
        "Con.txt",
        "PRN.mp4",
        "aux",
        "NUL.tar.gz",
        "CONIN$",
        "conout$.log",
        "CLOCK$",
        "COM0",
        "com1.ts",
        "COM9",
        "LPT1",
        "lpt9.bin",
        "COM¹",
        "LPT²",
        "COM³.mp4",
        " CON .txt",
    ] {
        assert!(reserved_dos_device_name(name), "{name}");
    }
    for name in [
        "",
        ".",
        "CONSOLE",
        "CONFIG.sys",
        "AUXILIARY",
        "NULL",
        "COM",
        "LPT",
        "COM10",
        "LPT0x",
        "COM⁴",
        "PRN_",
        "video-CON.mp4",
        "我的CON.mp4",
        "C0M1",
    ] {
        assert!(!reserved_dos_device_name(name), "{name}");
    }
}

#[test]
fn http_methods_collapse_to_get_post_or_head() {
    for (raw, expected) in [
        ("POST", "POST"),
        (" post ", "POST"),
        ("Post", "POST"),
        ("HEAD", "HEAD"),
        ("\thead\n", "HEAD"),
        ("GET", "GET"),
        ("", "GET"),
        ("PUT", "GET"),
        ("DELETE", "GET"),
        ("CONNECT", "GET"),
        ("POST\r\nX-Injected: 1", "GET"),
        ("POSTS", "GET"),
        ("ＰＯＳＴ", "GET"),
    ] {
        assert_eq!(sanitize_http_method(raw), expected, "{raw:?}");
    }
}

#[test]
fn fetch_urls_only_allow_http_schemes() {
    for url in [
        "http://a.test/",
        "HTTPS://A.TEST/x",
        "\u{feff}https://a.test/",
        "  http://a.test/  ",
    ] {
        assert!(http_fetch_url_allowed(url), "{url:?}");
    }
    for url in [
        "",
        "https",
        "https:/a.test",
        "ftp://a.test/",
        "file:///C:/Windows/win.ini",
        "javascript:alert(1)",
        "data:text/plain,hi",
        "blob:https://a.test/1",
        "//a.test/",
        "https://a.test/\u{0}",
        "https://a.test/\u{7f}",
        "https://a.test/\u{85}",
    ] {
        assert!(!http_fetch_url_allowed(url), "{url:?}");
    }
}

#[test]
fn remote_resources_allow_only_supported_transfer_schemes() {
    for url in [
        "http://a.test/",
        "https://a.test/",
        "FTP://a.test/f",
        "ftps://a.test/f",
        "sftp://user@a.test/f",
        "MAGNET:?xt=urn:btih:abc",
        "\u{feff}magnet:?xt=urn:btih:abc",
    ] {
        assert!(remote_resource_url_allowed(url), "{url:?}");
    }
    for url in [
        "",
        "file:///etc/passwd",
        "smb://host/share",
        "ssh://host/f",
        "tftp://host/f",
        "data:,x",
        "blob:https://a.test/1",
        "C:\\Users\\a.torrent",
        "magnet",
        "sftp://a.test/\r",
    ] {
        assert!(!remote_resource_url_allowed(url), "{url:?}");
    }
}

#[test]
fn canonical_keys_drop_userinfo_and_keep_malformed_authorities_verbatim() {
    assert_eq!(
        canonical_resource_key("https://user:pass@CDN.Test/a.mp4"),
        "https://cdn.test/a.mp4"
    );
    assert_eq!(
        canonical_resource_key("https://cdn.test"),
        "https://cdn.test/"
    );
    assert_eq!(
        canonical_resource_key("https://cdn.test:/a"),
        "https://cdn.test/a"
    );
    assert_eq!(
        canonical_resource_key("https://cdn.test:443?token=1"),
        "https://cdn.test/"
    );
    for raw in [
        "https://cdn.test:99999/a",
        "https://cdn.test:abc/a",
        "https://[::1/a",
        "https://[::1]x/a",
        "https:///a",
        "https://user@/a",
        "https://:8080/a",
    ] {
        assert_eq!(canonical_resource_key(raw), raw, "{raw}");
    }
}

#[test]
fn canonical_keys_keep_valueless_and_empty_query_parts_stably() {
    assert_eq!(
        canonical_resource_key("https://a.test/p?b&a=&&token=1&c=3"),
        "https://a.test/p?a=&b&c=3"
    );
    assert_eq!(
        canonical_resource_key("https://a.test/p?token=1&expires=2"),
        "https://a.test/p"
    );
    assert_eq!(
        canonical_resource_key("https://a.test/p?"),
        "https://a.test/p"
    );
    assert_eq!(
        canonical_resource_key("https://a.test/p?x=1#frag?token=2"),
        "https://a.test/p?x=1"
    );
    assert_eq!(canonical_resource_key(""), "");
    assert_eq!(canonical_resource_key("   "), "");
}

struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str, bytes: &[u8]) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hls-client-validation-{}-{nanos}-{name}",
            std::process::id()
        ));
        // create_new refuses existing paths, including planted symlinks.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| file.write_all(bytes))
            .unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn local_sources_require_blob_or_data_urls_and_an_exact_regular_file() {
    let file = TempFile::new("source.bin", b"12345");
    let path = file.0.to_str().unwrap();

    let canonical = validate_local_source_path("blob:https://a.test/1", path, Some(5)).unwrap();
    assert_eq!(canonical, fs::canonicalize(&file.0).unwrap());
    assert!(validate_local_source_path("DATA:text/plain,x", &format!(" {path} "), Some(5)).is_ok());

    for url in [
        "",
        "https://a.test/1",
        "file:///tmp/x",
        "blob:https://a.test/\n1",
        " blob:https://a.test/1",
    ] {
        assert!(
            validate_local_source_path(url, path, Some(5)).is_err(),
            "{url:?}"
        );
    }

    assert!(validate_local_source_path("blob:x", path, None)
        .unwrap_err()
        .contains("缺少声明的文件大小"));
    for size in [0, 4, 6, u64::MAX] {
        assert!(validate_local_source_path("blob:x", path, Some(size))
            .unwrap_err()
            .contains("大小不匹配"));
    }

    let dir = std::env::temp_dir();
    assert_eq!(
        validate_local_source_path("blob:x", dir.to_str().unwrap(), Some(0)).unwrap_err(),
        "本地源必须是常规文件"
    );
    for bad in [
        "".to_string(),
        "relative/source.bin".to_string(),
        format!("{path}\u{0}"),
        format!("{path}\n"),
    ] {
        assert!(
            validate_local_source_path("blob:x", &bad, Some(5)).is_err(),
            "{bad:?}"
        );
    }
    let missing = dir.join(format!(
        "hls-client-validation-{}-missing",
        std::process::id()
    ));
    assert!(
        validate_local_source_path("blob:x", missing.to_str().unwrap(), Some(0))
            .unwrap_err()
            .contains("无法读取本地源文件")
    );
}
