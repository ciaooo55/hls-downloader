# Third-Party Notices

HLS Downloader 的源码采用 MIT License；发布包还包含或调用下列第三方组件，其许可证分别适用：

- FFmpeg / FFprobe：发布构建使用 BtbN 的 Windows GPL 构建。FFmpeg 及启用的外部库可能适用 LGPL、GPL 及其他许可证；构建来源与配置见 <https://github.com/BtbN/FFmpeg-Builds> 和 <https://ffmpeg.org/legal.html>。
- Python、FastAPI、Uvicorn、httpx、Pydantic、aiosqlite、m3u8、curl-cffi、cryptography、yt-dlp、libtorrent、PyChromecast 及其传递依赖：以各自发行包中的许可证和元数据为准。
- Tauri、Rust crates、React、WXT、hls.js、Lucide 及 npm 传递依赖：以各自仓库或发行包中的许可证为准。
- NSIS：用于生成 Windows 安装器，许可证与附加组件条款见 <https://nsis.sourceforge.io/License>。

`requirements-release.lock`、`frontend/pnpm-lock.yaml`、`extension/pnpm-lock.yaml` 与 `frontend/src-tauri/Cargo.lock` 记录发布时使用的精确依赖版本。发布或再分发前应根据这些锁文件重新生成 SBOM，并保留第三方许可证文件。

本文件不是第三方许可证正文，也不会替代各组件随附的版权与许可证通知。
