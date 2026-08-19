# v6 切包清单

5.x 安装器继续按 [releasing.md](releasing.md) 发布。v6 切包前必须通过 [v6-release-gates.md](v6-release-gates.md)。

## 单一入口

安装版与便携版只启动 `HLSDownloader.exe`（`native_ui` crate）。该进程：

- `CoreServer::open_default` 打开 SQLite（唯一持有者），并监听 `\\.\pipe\HLSDownloader.v6`；Windows 产品默认不绑 `127.0.0.1` TCP。Slint 与 Native Messaging 只走命名管道。
- 预创建确认 / 进度 / 完成 / 设置 / 新建 / 播放器窗口
- 托盘与单实例由同一进程持有

本地验证（需要 MSVC）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\run_v6_gates.ps1
cargo test --manifest-path native_shell/Cargo.toml
cargo test --manifest-path native_ui/Cargo.toml
cargo build --manifest-path native_ui/Cargo.toml --release --bin HLSDownloader
Copy-Item native_ui\target\release\HLSDownloader.exe native_ui\target\release\HLSDownloaderNativeHost.exe
```

Native Messaging 安装 `HLSDownloaderNativeHost.exe`（同一二进制的副本或硬链接）。文件名含 `NativeHost` 时走 `--native-host` 路径，**不打开 SQLite**，只连 v6 Core。

打包：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build_v6.ps1 -Version 6.0.0-dev
```

产物在 `release/`：便携 zip；本机有 `makensis` 和 `assets/app-icon.ico` 时再出 Setup.exe。安装包只放这一个主 exe（Native Host 是同文件副本）。5.x 的 `scripts/build_installer.ps1` 仍是现网发布路径。

## Native Messaging

并行开发清单：

- 5.x 继续用 `com.ciaooo55.hls_downloader`
- v6 用 `com.ciaooo55.hls_downloader.v6`（`extension/native-host/v6-chrome.json` / `v6-firefox.json`）
  `scripts/register-native-host.ps1 -V6`
- 切包当天把现网 5.x host 名指到 v6 二进制：`scripts/register-native-host.ps1 -Cutover`（扩展代码不必改 host 名）

## 5.x 数据迁移

首次启动会从 5.x 安装目录 / 便携目录 / 源码树的 `config.json` + `data.db` 导入设置、任务行（含状态与已下字节）、浏览器回放凭证（DPAPI），并把未完成 HTTP 的 `payload.downloading` / Range 检查点拷进 v6 任务目录。失败不删除 5.x 文件。进行中的任务导入为暂停，不会自动开下。可用环境变量覆盖：

- `HLS_V6_MIGRATE_CONFIG` / `HLS_V6_MIGRATE_DB`
- `HLS_V6_MIGRATE_TEMP` 额外的 5.x `.tasks` 根目录
- `HLS_V6_SKIP_MIGRATE` 跳过
- `HLS_V6_MIGRATE_FORCE` 再导一次

## 行为矩阵

切包后才允许从仓库移除 Python / Tauri 发布路径。在此之前 `backend/`、`frontend/` 仍是行为规格与测试矿。

## 诚实口径

v6 安装包脚本与并行 Native Messaging 名已经就绪。现网 5.x 安装器仍按 [releasing.md](releasing.md) 发布，直到有人在 Windows 上跑完行为矩阵并执行 `scripts/register-native-host.ps1 -Cutover`。Release workflow 会额外产出 `release-v6/` 预览包，不会并进 5.x 的「恰好 N 个文件」检查，也不会自动改现网 host 名。
