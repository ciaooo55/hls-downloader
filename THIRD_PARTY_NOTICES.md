# Third-Party Notices

HLS Downloader 的源码采用 MIT License；发布包还包含或调用下列第三方组件，其许可证分别适用：

- libmpv / mpv：v7 的隔离播放器进程加载随包装入的 `libmpv-2.dll`（固定 shinchiro `mpv-dev` x86_64 构建）。libmpv 适用 GPL；分发者必须保留许可证与版权通知，并按 GPL 提供对应源代码。构建时使用固定 `7zr.exe` 从 `.7z` 抽出 DLL，该解压器本身不打进安装包。
- FFmpeg / FFprobe：发布构建使用 BtbN 的 Windows GPL 构建，并以独立可执行文件方式调用。FFmpeg 及启用的外部库可能适用 LGPL、GPL 及其他许可证；构建来源与配置见 <https://github.com/BtbN/FFmpeg-Builds> 和 <https://ffmpeg.org/legal.html>。分发者必须核验实际构建许可、保留许可证与版权通知，并按适用许可证提供对应源代码和构建信息。本项目用户协议不限制第三方许可证依法允许的修改、重新链接或逆向工程。
- Compose Desktop、Skiko、Kotlin 及 Gradle 传递依赖：v7 主工作台使用；以各自仓库或发行包中的许可证为准。
- Slint 及 Rust crates：低延迟 presenter 和 Rust Core 使用；以各自仓库或发行包中的许可证为准。
- WXT 及 npm 传递依赖：Chromium/Firefox 扩展使用；以各自仓库或发行包中的许可证为准。

`native_shell/Cargo.lock`、`presenter_ui/Cargo.lock`、`desktop_ui` Gradle 配置与 `extension/pnpm-lock.yaml` 记录 v7 产品依赖。发布或再分发前应根据实际打进安装包的组件重新生成 SBOM，并保留第三方许可证文件。

本文件不是第三方许可证正文，也不会替代各组件随附的版权与许可证通知。二次开发或再分发者应在每次发布前重新审计依赖、FFmpeg 构建选项、专利风险和源代码提供方式，不能仅复制本文件作为合规结论。
