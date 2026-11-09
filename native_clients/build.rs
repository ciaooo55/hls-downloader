fn main() {
    println!("cargo:rerun-if-changed=../assets/app-icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("windows") {
        return;
    }
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon = output.join("hls-downloader.ico");
    std::fs::copy("../assets/app-icon.ico", &icon).expect("stage application icon");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(icon.to_str().expect("build-cache icon path"));
    resource.set("ProductName", "HLS Downloader");
    resource.set("FileDescription", "HLS Downloader Native Client");
    resource.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    resource.set("FileVersion", env!("CARGO_PKG_VERSION"));
    resource
        .compile()
        .expect("embed Windows application identity");
}
