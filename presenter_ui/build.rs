fn main() {
    println!("cargo:rerun-if-changed=../assets/app-icon.ico");
    slint_build::compile("ui/hot.slint").expect("compile v7 hot presenter UI");
    embed_windows_version();
}

fn embed_windows_version() {
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut resource = winresource::WindowsResource::new();
    resource.set("ProductName", "HLS Downloader");
    resource.set("FileDescription", "HLS Downloader");
    resource.set("ProductVersion", &version);
    resource.set("FileVersion", &version);
    resource.set("OriginalFilename", "HLSDownloader.exe");
    resource.set("LegalCopyright", "HLS Downloader");
    // Stage the icon under OUT_DIR so rc.exe never receives the non-ASCII repository path.
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon = output.join("hls-downloader.ico");
    std::fs::copy("../assets/app-icon.ico", &icon).expect("stage presenter icon");
    resource.set_icon(icon.to_str().expect("ASCII build-cache icon path"));
    resource.compile().expect("embed Windows version resource");
}
