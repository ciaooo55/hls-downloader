# 原生迁移功能映射

源清单：`config/release/feature-parity.json` 的 33 项。这里单独登记原生迁移情况；旧 Compose 的 verified 不继承给原生候选。实现存在、定向验证、候选验收是三个不同状态。构建脚本默认原生入口，历史公开包仍为 Compose，`release_ready=false`。

| 功能 ID | 原生入口 / 所属模块 | Core 合同 | 原生迁移状态与剩余验收 |
|---|---|---|---|
| architecture.single_core | `native_ui/src/workbench.rs`、`crates/client` | Hello、Capabilities、ListTasks、WaitEventBatch | 已拆分；依赖边界通过，候选组合仍待验收 |
| workbench.geometry | `native_window.rs`、`workbench.slint` | 现有窗口 properties | 工作区夹取、实际 HWND；2026-11-09 窄窗恢复下限与当前 560×360 最小尺寸对齐，避免重开被强制放大。多显示器、混合 DPI、移屏及本次恢复增量未实窗验收 |
| workbench.foundation_component_architecture | `ui/theme.slint`、`workbench_components.slint` 与页面组件 | 无业务状态所有权 | 保留原 SVG/图案，52 个共享图标、14 类窗口均设置应用图标；共享控件尺寸/圆角/间距并固定 Fluent 样式。38 个计划视图已分轮实窗查看，修正图标偏位、字段/按钮拉伸、投屏初始尺寸、系统页裁切与 PiP 标题栏并复验；截图按实际二进制登记。完整 GUI 业务、混合 DPI 与完整键盘/读屏未验收 |
| tasks.selection_keyboard_queue | `task_list.rs`、工作台列表和右键菜单 | TaskAction、ReorderQueue、PlaceQueue | Ctrl/Shift、Ctrl+A/Esc、队列动作已接；2026-11-09 实窗确认更多菜单保留两项选择、复制重读两项、单选清除其他勾选与取消选择同步。全部开始包含隐藏暂停任务且不重写成品，完成任务显式重新下载保留；拖动和全部排序未验收 |
| tasks.named_queue_profiles | `queue_editor.rs`、`queues.slint` | StoreSettings、AssignQueue | 此前真实 UI 新增→保存→关闭→重开通过；最新统一样式候选再次真实 UI 新增/保存、主窗显示并经 Core 重读确认；批量分配、计划执行与限额组合待验收 |
| tasks.details_logs_speed_connections | `DetailsWindow`、`workbench.rs` | GetTaskLog、TaskAction(speed) | 详情、图表、日志、限速已接；图表/连接显示和多窗口压力待验收 |
| tasks.refresh_signed_url | 详情“更新并继续” | RefreshTaskRequest | 已接；2026-11-10 暂停后刷新地址并发布新响应的真实本机 HTTP 回归通过，保持原 5 秒门槛；新 UI 带凭据失败恢复待验收 |
| create.protocols_and_recognition | `TaskEditorWindow`、`resource_tools.rs` | ProbeUrl、CreateTask、HarvestPage、ImportPaths | 新建、批量、cURL、轨道选择已接；全部协议的真实 UI 往返待验收 |
| import.file_picker_and_drop | 文件选择器、winit DroppedFile、共享 Windows 文件交互 | ImportPaths | 2026-11-09 实窗多选复制 2 项经 Windows CF_HDROP 重读确认，Core 后台复制通过；Explorer 三文件粘贴导入两份有效列表，无效列表未新增任务，GUI 明确提示部分成功。OLE 跨窗口拖出与 Esc 取消未验收；旧关机通知已过期 |
| export.normalized_task_list | 工作台导出 | ExportTasks | 2026-11-09 真实另存为导出 5 项，schema、内容与无合成 Cookie 泄漏检查通过；未将此扩大为所有导出格式通过 |
| import.exported_task_json | 工作台导入 | ImportPaths | 两份 Core 格式列表经真实 GUI 粘贴导入成功；真实导出内容已检查，导出文件再次导入的完整往返仍待验收 |
| workbench.automatic_responsive_layout | 工作台、主题设置 | 窗口尺寸、task_sort | 按用户 2026-11-09 要求改为自动布局：删除逐列宽度/显示顺序、工具栏排列及重置入口，窄窗自动收起速度/大小列并压缩工具栏。453–1920 可用宽度范围与伸缩恢复两项回归通过；本轮长队列标题可省略并显示完整 Tooltip，25 项 UI 回归、新 Release 构建与自检通过。上批标题栏 API 证据单独保留。用户禁止桌面操作后未再检查实窗，宽窄布局、外观页与旧布局保存不受影响尚未实窗验收。历史手工布局验收不作为当前设计要求 |
| media.hls_transfer_parity | 新建 HLS、任务动作 | CreateTask、TaskAction、RefreshTaskRequest | Core 保留；原生 UI HLS/DASH/直播、断点及成品组合待验收 |
| media.local_player_controls | `media_controller.rs`、`media.slint` | PlayTask、PlayerControl、PlayerEmbed、PlayerThumbnail | 最新样式候选真实鉴权视频哈希、轨道/暂停/seek/倍速/音量/预览通过；同会话主窗/PiP/全屏往返，主窗最小化后的独立 PiP 与返回通过，PiP 标题栏和应用图标已复验。播放未额外请求源；悬停预览增量待验收。2026-11-09 会话换片、停止及成品切换撤销旧本机挂载，真实 HTTP 206→404 回归通过；本轮播放器为 null 后端，未追加 GUI 验收 |
| media.cast_dlna_chromecast | `cast_tools.rs`、`cast_tools.slint` | DiscoverCastDevices、CastToDevice、ShareMedia、CastControl | 已编译；本机模拟接收端验证了设备选择、来源恢复、代数和停止；DLNA/Chromecast 实机与组播发现按既有排除 |
| media.lan_share | 投屏窗口的局域网发布与会话页 | ShareMedia、CastToDevice、CastControl | 真实 Core + 本机模拟接收端 Range 字节、来源恢复、替换和停止撤销地址通过；独立发布入口 GUI 往返待验收 |
| media.tvbox_push | 右键 TVBox、工具菜单 | DiscoverCastDevices、ShareMedia | 原生接收端选择、实际 POST 与 Range 读取的本机模拟往返通过；真实设备排除 |
| media.cast_and_player_concurrency | 播放器控制与独立投屏会话 | PlayerControl、ControlCast | 播放控制代数已接；原生投屏 HUD/并存模拟接收端验证待完成 |
| torrent.multi_file_selection | `ResourceToolsWindow` 种子模式 | ProbeTorrent、SelectTorrentFiles、SetTaskTorrentFiles | 异步探测结果、文件选择已接；真实种子 UI 往返待验收 |
| settings.current | `SettingsEditor`、`ServiceSettingsWindow` | LoadSettings、StoreSettings、SetDefaultCookie、SetSiteRuleCredential、Subtitle | 通用设置与专用字幕/凭据已接；旧格式、凭据保留与输入拒绝回归通过；最新候选真实 UI 将站点限速保存为 512，Core 重读及原凭据引用保留通过，浅/深色及设置重开实际观察通过；全部字段组合和专用服务 GUI 仍未逐一验收。2026-11-09 修复字幕配置失败回执被误报成功，UI 定向回归已通过 |
| updates.confirmed_download | `SystemWindow` 更新页 | CheckUpdate、DownloadUpdate、InstallUpdate | 发现、确认下载、摘要门槛、确认安装已接；候选安装、占用失败和回滚未验收 |
| browser.request_context_replay | 扩展、轻 Host、Core | OfferResource、AdmitHandoff | 精确请求身份和作用域回归已做；新组合真实 Host/Core 成品交接待完成 |
| browser.takeover_and_recovery | Presenter、`SystemWindow` 浏览器页 | PresentHandoff、AcceptHandoff、RejectHandoff、LoadHandoffs | 原 ID 恢复合同保留；新 UI 和双浏览器断连/重启组合待完成 |
| browser.media_push_device_selection | `cast_tools.rs` 推送裁决 | RequestMediaPush、ResolveMediaPush、LoadHandoffs | 真实 Core 恢复原 ID、GUI 选择并发送、原 ID 持久完成通过；浏览器/Host 断连矩阵待验收，实机排除 |
| browser.hot_confirmation_process | `hot_main.rs`、激活管道 | PresentHandoff | 确认、进度、完成与单实例已迁；同包带凭据交接呈现时延未验收 |
| browser.online_subtitle_translation | 扩展字幕、`ServiceSettingsWindow`、`player_subtitles.rs` | configuration/configure/start/audio/reset/poll/stop | 本地配置与 DPAPI 保存已接；原生主窗/PiP 字幕 HUD、按需工作线程、Core 自有字节有界解码、seek/换片/音轨代数失效已实现，工作区 UI 28 项与真实 FFmpeg/Core 私有挂载字幕回归通过。未实现暂停等待字幕；真实云质量排除 |
| accessibility.automation | Slint accessibility、工作台控件与列表标签 | 无 | 已实际读取 UIA 控件树并完成中文搜索和部分操作；输入法组合输入、完整 Tab/读屏未验收 |
| performance.release_thresholds | Release 构建和性能脚本 | 完整组合 | 2026-11-10 新 Core 的两次 256 MiB 本机下载为 32.58/41.13 MiB/s，哈希正确、发布后额外读取 0，峰值工作集 27.09/27.21 MiB；修正检查点等锁/同步误触发降并发，保留 8 MiB 持久化。60 秒空闲及 18,000 次空任务快照 IPC 通过，p95 0.203 ms、错误 0；四 Host 并发只启动一个 Core，最慢首次回执 968.29 ms。低并发候选的两次性能失败均保留。完整包、千任务及长期运行尚未验收；无同条件 IDM 对照 |
| package.install_upgrade_rollback | 原生 launcher、WiX、Portable、Updater | 既有更新合同 | 已增加原生 EXE/MSI/Portable 构建路径，默认不使用 Compose/JRE；轻量 WiX 结构夹具通过，支持已公开 Compose 7.0.3 自定义目录迁移。真实安装/升级/失败回滚尚未验收，P6/P7 未完成 |
| free.cli | `native_clients` CLI | 共享客户端、原有命令 | 已与 Core 库解耦；2026-11-10 与最新 Core 组合完成单 Core 启动、下载/暂停/重启恢复、队列、导入导出、显式删除、stdin 上下文及脱敏的本机成品验收；未扩大为全部命令与退出码组合通过 |
| free.crawl_sync | `ResourceToolsWindow` 抓站模式 | Crawl 各动作 | 项目创建、列表、暂停、恢复、同步、入队已接；2026-11-10 最新 Core 本机集成验证通过循环/深度边界、预览恢复、跨域请求前拒绝、ETag、失败保留旧文件、队列窗口与定时同步；原生 GUI 往返仍待验收 |
| free.zip_preview | `ResourceToolsWindow` ZIP 模式 | PreviewZip | 2026-11-09 本地路径经原生 UI 输入并读取，正确显示两项归档内容；远程 ZIP 经真实 Core API 读取通过。远程 GUI 和系统选择器选中 ZIP 仍未验收：owned 弹窗自动化输入/缓存异常，未用 Core 结果替代 GUI 通过 |
| http.active_tail_split | 现有实验开关、Core RangeScheduler | 调度内部 | 保持默认关闭；新候选正确性、崩溃恢复及收益比较未完成 |
| http.coordinated_range_stop | `http_engine.rs` 范围 worker、正文循环及 TCP/WinHTTP/curl 等待 | 调度内部，无协议变化 | 2026-11-09 补齐共享停止到传输层的传播，保留暂停原因与首个错误；单流接收原 200 响应时解除范围停止绑定。2026-11-10 最终 HTTP 83 项通过、8 项忽略，覆盖本机 TCP/WinHTTP 响应头、正文、chunked 停滞及原响应复用。curl 新停止分支未运行专用组件回归；DNS/连接阶段仍有底层超时边界。最新成品验证见实施记录 |

每次完成验证应在 `native-migration-progress.md` 登记实际日志、二进制身份和范围，再更新本表。模拟 Core、真实 Core、本地媒体和真实浏览器交接的证据不能互相替代。
