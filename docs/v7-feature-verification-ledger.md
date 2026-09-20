# v7 功能面逐项台账（2026-09-19，本地 `main` @ `ecb103b`）

本文是**唯一活动版本 7.0.2** 的"项目应有功能"全集台账，来源是 canonical
`artifacts/v7-productization/feature-parity.json`（28 项：27 verified / 1 partial）。
目的是把每一项的**代码实现位置**、**是否依赖本机之外的条件**、**本机当前能否实测**讲清楚，
避免把"清单声明 verified"误读为"这台机器现在实测通过"。

## 判定口径

- **代码实现**：实际读到的源码是否真的实现了该功能（不采信清单声明）。
- **本机可实测**：在当前本机（Windows、仅装 Edge、LAN 上无可用的 DLNA/TVBox 接收端）能否端到端跑通。
- **清单证据**：feature-parity 该项 `verification` 引用的证据来自哪个日期/机器。绝大多数为
  2026-09-13 / 2026-09-15 在**带真实局域网设备的历史环境**采集，属历史证据，本机不可复现。

## 本机现状（实测）

- 工作网段：`192.168.2.0/24`（WLAN `192.168.2.6/24`）。
- `192.168.2.11`（历史 DLNA/Phicomm 证据设备）：ARP `Unreachable`，ping 不通。
- `192.168.2.5`（历史 TVBox 证据设备）：主机可达（MAC `FC-7C-02-49-D6-6D`），但 **TVBox 端口 9978 未开放**（连接被拒），当前**不是**可用接收器。
- SSDP `M-SEARCH` 主动探测 4s：**0 个** DLNA 设备响应。
- `libmpv-2.dll`：仓库源码树内没有；仅存在于打包产物（`artifacts/v7-productization/candidate/**`、当前交付目录 `outputs/local-*/**`）与 Compose 构建缓存中。

结论：本机当前**没有任何可用的投屏/TVBox/DLNA 接收端**，因此所有"需要真实接收设备"的门禁在本机无法闭环。

## 逐项台账

| # | feature id | 代码实现 | 本机可实测 | 清单证据环境 |
| --- | --- | --- | --- | --- |
| 1 | architecture.single_core | ✅ `contract.rs`/`core_runtime.rs` | ✅ 需先编译引擎 | 本地脚本 |
| 2 | workbench.geometry | ✅ `Main.kt` + `WindowGeometryStore.kt` | ✅ Compose test | 本地 |
| 3 | workbench.foundation_component_architecture | ✅ `WorkbenchComponents.kt` | ✅ Compose test | 本地 |
| 4 | tasks.selection_keyboard_queue | ✅ `Main.kt` selection/drag + Rust reorder/place | ✅ Compose test + Rust | 本地 |
| 5 | tasks.named_queue_profiles | ✅ `QueueManagerDialog`/`QueueAssignDialog` + assign_queue | ✅ Compose + Rust | 本地 |
| 6 | tasks.details_logs_speed_connections | ✅ `TaskDetailsDialog`/`get_task_log` | ✅ Compose + Rust | 本地 |
| 7 | tasks.refresh_signed_url | ✅ `download_worker::refresh_task_request` | ⚠️ 单测可，真实续传需网络 | 本地 |
| 8 | create.protocols_and_recognition | ✅ `NewTaskDialog`/`ProbeResultDialog` + Rust recognize | ✅ 可 | 本地 |
| 9 | import.file_picker_and_drop | ✅ `DesktopToolbar` + `import_paths` | ✅ 可 | 本地 |
| 10 | export.normalized_task_list | ✅ `task_export.rs` | ✅ 可 | 本地 |
| 11 | import.exported_task_json | ✅ `task_import.rs` | ✅ 可 | 本地 |
| 12 | workbench.automatic_responsive_layout | ✅ `resolveTaskColumns` | ✅ Compose test | 本地 |
| 13 | media.hls_transfer_parity | ✅ `media/hls.rs` 全链路（AES/BYTERANGE/LIVE） | ✅ 纯 HTTP，本机可跑 | 2026-09-13 单测 |
| 14 | media.local_player_controls | ✅ `player.rs`（`LoadLibrary libmpv-2.dll`） | ⚠️ 需 `libmpv-2.dll`（仅打包产物有） | 2026-09-15（用安装目录 DLL） |
| 15 | media.cast_dlna_chromecast | ✅ `cast.rs` SSDP+mDNS | ❌ 无可达 DLNA 设备 | 2026-09-13（`192.168.2.11`） |
| 16 | media.lan_share | ✅ `playback.rs` + `share_media` | ⚠️ 发布/复制地址可验，电视端播放不可验 | 2026-09-15 |
| 17 | media.tvbox_push | ✅ `cast.rs::push_tvbox` | ❌ 无可用 TVBox（`:9978` 不可达） | 2026-09-15（`192.168.2.5`，仅一次成功） |
| 18 | media.cast_and_player_concurrency | ✅ Compose 叠放 HUD | ⚠️ 仅视觉基线 | 2026-09-15（截图） |
| 19 | torrent.multi_file_selection | ✅ `torrent_engine.rs` + `TorrentSelectionDialog` | ⚠️ 单测可，真机 swarm 需 peer | 本地 + loopback |
| 20 | settings.full_migration | ✅ `SettingsV7.FullSettingsDialog` 十节 | ✅ Compose + Rust | 本地 |
| 21 | updates.confirmed_download | ✅ `updater.rs` + `UpdateDialog` | ❌ 端到端需 MSI + GitHub 网络 | 本地单测 + 实机 |
| 22 | browser.request_context_replay | ✅ extension + Native Host 重放上下文 | ✅ Edge 内可触发 | 扩展 36 文件/208 测试 |
| 23 | browser.takeover_and_recovery | ✅ extension 接管链路 + Compose HandoffDialog | ✅ Edge 内可触发 | 生产 Edge 实机 |
| 24 | browser.media_push_device_selection | ⚠️ **partial**；插件侧 op 齐全，桌面端配 DevicePicker | ❌ 需真实 LAN 设备 | 待实机门禁 |
| 25 | browser.hot_confirmation_process | ✅ presenter 热确认窗口 | ⚠️ 依赖 native/presenter 渲染 | 生产 Edge 实机 |
| 26 | accessibility.automation | ✅ Compose semantics + AccessBridge | ⚠️ 语义测试可，AccessBridge 端到端需起工作台 | 实机 |
| 27 | performance.release_thresholds | ✅ `LazyColumn` 虚拟化 | ⚠️ 模型测试可，release 阈值需真实负载 | 实机 |
| 28 | package.install_upgrade_rollback | ✅ `build-v7.ps1`（jpackage MSI） | ❌ 需真实 MSI 安装 + 管理员 + `E:\h` | 隔离 MSI 生命周期 |

图例：✅ 本机可实测通过 / ⚠️ 部分可测或需额外条件 / ❌ 本机不可实测（依赖本机之外条件）。

## 本机本次实际执行的自动化结果（`main` @ `ecb103b`）

| 命令 | 结果 |
| --- | --- |
| `cargo +1.98.1 test --manifest-path native_shell/Cargo.toml --lib` | `464 passed; 0 failed; 1 ignored`，exit 0 |
| `cargo +1.98.1 test --manifest-path presenter_ui/Cargo.toml` | `9 passed; 0 failed`，exit 0 |
| `desktop_ui\gradlew.bat test --no-daemon --rerun-tasks`（JDK21） | `BUILD SUCCESSFUL`，8 tasks executed（强制真实执行） |
| 扩展 `wxt prepare` → `tsc --noEmit` → `vitest run` | 全部 exit 0；`vitest` 43 文件 / 301 用例通过 |

> 这些是**回归测试**（证明既有实现对既有契约仍然成立），不等于正式发布门禁的实机证据。

## 结论

1. **28 项功能在代码层面全部真实实现**（含唯一 partial 项，其插件侧调用链完整、无缺失 op），
   未发现"清单声称已实现但代码缺失"或"按钮在但后端 op 缺失"的断链。
2. **本机可实测的**：Rust/presenter/Compose/扩展的全部回归测试（见上表）均通过；
   插件在本机 Edge 内的识别 / 下载 / 复制链接 / UI 已端到端验证通过。
3. **本机不可实测、必须依赖本机之外条件的**：
   - 需要真实局域网接收端的：`media.cast_dlna_chromecast`、`media.tvbox_push`、
     `browser.media_push_device_selection`（本机 LAN 现无可达接收端）。
   - 需要真实安装/MSI/管理员/`E:\h` 的：`package.install_upgrade_rollback`。
   - 需要正式签名证书与 `hls-release` 自托管运行器的：正式打包/签名/发布。
   - 需要真实负载长时间运行的：`performance.release_thresholds` 的 release 阈值脚本。
4. 因此：**"所有功能都没问题"这句不能无条件成立**。能成立的说法是——
   "全部功能在代码层面已实现且通过本机可执行的回归测试；其中投屏/TVBox/正式安装/正式发布
   这几项依赖本机没有的硬件与授权环境，属未实测，`browser.media_push_device_selection` 保持 partial，
   `release_ready=false` 不变。"

## 真实浏览器端到端验证（2026-09-20，本机 Edge 153 + 未打包扩展 + 已注册原生主机）

**方法**：用独立 Edge profile（`--user-data-dir` 指向仓库内 `.tool-cache\build-cache\edge-inject*`，
不触碰用户自己的 Edge 数据）以 `--load-extension` 加载 `extension\.output\chrome-mv3`，
通过 CDP 导航与求值；浏览器进程设置隔离的 `HLS_V7_DATA_DIR` / `HLS_V7_DOWNLOAD_DIR`，
避免污染真实数据目录。

| 验证项 | 实测结果 |
| --- | --- |
| 内容脚本真实注入 | `document.documentElement[data-hls-downloader-extension]="1"`（加载完成态），shadow root 已挂载 |
| 播放 overlay 激活 | 合成 `play` 事件后 `.video-buttons` 图层 `display:block` / `position:fixed`，1 个操作组；按钮 aria-label 为 `下载当前视频`、`更多操作：投屏或推送当前媒体链接`，hover 面板含 `下载` / `投屏` / `TVBox` |
| overlay 截图 | 1240x845，1192 种颜色，非白像素 9.1%，含主题色 `#2563EB`（3720 px） |
| popup 真实扩展上下文 | `chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/popup.html` 正常渲染：`HLS Downloader / 连接中… / 打开 / 自动接管 / 本站 Cookie / 本站提示 / 已识别资源 0 / 重新识别 / 浏览器插件 版本 7.0.2`；9 个按钮、资源列表存在、**无"未启用/无法连接"错误**（原生消息通道可用） |
| popup 截图 | 420x640，952 种颜色，非白像素 86.0%，含主题色 `#2563EB` |

**仍未覆盖**：本机无 Firefox、无任何 WebDriver，故 Firefox 与 Brave / Vivaldi / Opera / Chromium
未实测；投屏 / TVBox 的真实推送仍需真实局域网接收端。