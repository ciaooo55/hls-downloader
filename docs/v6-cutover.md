# v6 切包清单

GitHub Windows Release 的现网安装包是 v6：`scripts/build_v6.ps1` 产出 `HLSDownloader-v*-Windows-x64-{Setup.exe,Portable.zip}`。5.x `backend/`、`frontend/` 仍是行为规格与测试矿，直到仓库删除它们。

## 单一入口

安装版与便携版只启动 `HLSDownloader.exe`（`native_ui` crate）。该进程：

- `CoreServer::open_default` 打开 SQLite（唯一持有者），并监听 `\\.\pipe\HLSDownloader.v6`；Windows 产品默认不绑 `127.0.0.1` TCP。Slint 与 Native Messaging 只走命名管道。
- 预创建确认 / 进度 / 完成 / 设置 / 新建 / 播放器窗口
- 托盘与单实例由同一进程持有

本地验证（需要 MSVC）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\run_v6_gates.ps1
cargo test --manifest-path native_shell/Cargo.toml --lib --no-default-features
cargo test --manifest-path native_ui/Cargo.toml
powershell -ExecutionPolicy Bypass -File scripts\build_v6.ps1 -Version 6.0.0-dev
```

Native Messaging 安装 `HLSDownloaderNativeHost.exe`（同一二进制的副本或硬链接）。文件名含 `NativeHost` 时走 `--native-host` 路径，**不打开 SQLite**，只连 v6 Core。

Setup.exe 安装时执行 `register-native-host.ps1 -Cutover`，把现网 host 名 `com.ciaooo55.hls_downloader` 指到 v6 二进制。便携包可手动：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\register-native-host.ps1 -Cutover
```

`libmpv-2.dll` 若存在于 `HLS_V6_LIBMPV`、仓库根、`native_ui/` 或 exe 旁，会打进安装包；没有 DLL 时播放降级，NSIS 用 `/nonfatal`。`ffmpeg.exe` / `ffprobe.exe` 始终从与 5.x 相同的针定 BtbN 包打入（本地可用 `-UseSystemFfmpeg`）。

## 5.x 数据迁移

首次启动会从 5.x 安装目录 / 便携目录 / 源码树的 `config.json` + `data.db` 导入设置、任务行（含状态与已下字节）、浏览器回放凭证（DPAPI），并把未完成 HTTP 的 `payload.downloading` / Range 检查点拷进 v6 任务目录。失败不删除 5.x 文件。进行中的任务导入为暂停，不会自动开下。可用环境变量覆盖：

- `HLS_V6_MIGRATE_CONFIG` / `HLS_V6_MIGRATE_DB`
- `HLS_V6_MIGRATE_TEMP` 额外的 5.x `.tasks` 根目录
- `HLS_V6_SKIP_MIGRATE` 跳过
- `HLS_V6_MIGRATE_FORCE` 再导一次

## 行为矩阵

切包后才允许从仓库移除 Python / Tauri 发布路径。在此之前 `backend/`、`frontend/` 仍是行为规格与测试矿。Release workflow 在打 v6 包前仍跑 5.x pytest / 前端 / 扩展作为冻结规格。

## 诚实口径

源码与 GitHub Release 现网安装包都是 Core+Slint 的 v6。BT 仍是 `TorrentSession` / `BuiltinTorrentEngine`（swarm 冻结，可换 libtorrent）。播放器把 libmpv `wid` 接到播放器 HWND 客户区子窗；没有 `libmpv-2.dll` 时降级。FFmpeg 随包装入，供 HLS/DASH 合并。
