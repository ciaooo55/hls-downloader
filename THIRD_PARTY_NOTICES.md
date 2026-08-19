# Third-Party Notices

HLS Downloader 的源码采用 MIT License；发布包还包含或调用下列第三方组件，其许可证分别适用：

- libmpv / mpv：v6 播放器在同进程加载 `libmpv-2.dll`（若存在）或 spawn `mpv`。libmpv 适用 GPL；分发者必须保留许可证与版权通知，并按 GPL 提供对应源代码。
- FFmpeg / FFprobe：发布构建使用 BtbN 的 Windows GPL 构建，并以独立可执行文件方式调用。FFmpeg 及启用的外部库可能适用 LGPL、GPL 及其他许可证；构建来源与配置见 <https://github.com/BtbN/FFmpeg-Builds> 和 <https://ffmpeg.org/legal.html>。分发者必须核验实际构建许可、保留许可证与版权通知，并按适用许可证提供对应源代码和构建信息。本项目用户协议不限制第三方许可证依法允许的修改、重新链接或逆向工程。
- Python、FastAPI、Uvicorn、httpx、Pydantic、aiosqlite、m3u8、curl-cffi、cryptography、yt-dlp、libtorrent、PyChromecast 及其传递依赖：以各自发行包中的许可证和元数据为准。
- Tauri、Rust crates、React、WXT、hls.js、Lucide 及 npm 传递依赖：以各自仓库或发行包中的许可证为准。
- NSIS：用于生成 Windows 安装器，许可证与附加组件条款见 <https://nsis.sourceforge.io/License>。

`requirements-release.lock`、`frontend/pnpm-lock.yaml`、`extension/pnpm-lock.yaml` 与 `frontend/src-tauri/Cargo.lock` 记录发布时使用的精确依赖版本。发布或再分发前应根据这些锁文件重新生成 SBOM，并保留第三方许可证文件。

本文件不是第三方许可证正文，也不会替代各组件随附的版权与许可证通知。二次开发或再分发者应在每次发布前重新审计依赖、FFmpeg 构建选项、专利风险和源代码提供方式，不能仅复制本文件作为合规结论。
