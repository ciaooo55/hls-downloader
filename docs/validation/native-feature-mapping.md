# 原生迁移功能映射

源清单：`config/release/feature-parity.json` 的 33 项。正式版 [v7.0.4](https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.4) 已发布，受测源码为 `8b6fa0743db2441c798e5d3d47a96137c4347fc7`；源码 CI、候选打包、浏览器和原生完整验收均绑定此提交。正式包复用受测组件字节，正式 EXE 再次通过安装启动检查；13 项发布附件与本地 SHA-256 一致。最新范围与限制见 [当前验证表](current-evidence.json) 的 `formal_release`。

以下逐项表保留迁移过程中的阶段记录；其中“待验收”描述记录当时的状态，当前结论以顶部正式发布记录、发布附件 `FEATURE-PARITY.json` 和 `RELEASE-EVIDENCE.json` 为准。真实投屏/TVBox、付费云字幕质量仍按用户决定排除；Firefox 交付待签名包，软件缩放测试不代表物理多显示器切换验收。

| 功能 ID | 原生入口 / 所属模块 | Core 合同 | 原生迁移状态与剩余验收 |
|---|---|---|---|
| architecture.single_core | `native_ui/src/workbench.rs`、`crates/client` | Hello、Capabilities、ListTasks、WaitEventBatch | 已拆分；依赖边界通过，候选组合仍待验收 |
| workbench.geometry | `native_window.rs`、`workbench.slint` | 现有窗口 properties | 工作区夹取、实际 HWND；2026-11-09 窄窗恢复下限与当前 560×360 最小尺寸对齐，避免重开被强制放大。多显示器、混合 DPI、移屏及本次恢复增量未实窗验收 |
| workbench.foundation_component_architecture | `ui/theme.slint`、`workbench_components.slint` 与页面组件 | 无业务状态所有权 | 保留原 SVG/图案，52 个共享图标、14 类窗口均设置应用图标；共享控件尺寸/圆角/间距并固定 Fluent 样式。38 个计划视图已分轮实窗查看，修正图标偏位、字段/按钮拉伸、投屏初始尺寸、系统页裁切与 PiP 标题栏并复验；截图按实际二进制登记。完整 GUI 业务、混合 DPI 与完整键盘/读屏未验收 |
| tasks.selection_keyboard_queue | `task_list.rs`、工作台列表和右键菜单 | TaskAction、ReorderQueue、PlaceQueue | 2026-11-13 c42636a4 成品以真实键盘完成 Ctrl+A/Esc、Shift 范围多选，并以鼠标拖动改变实际 Core 队列顺序；13 项业务验收通过。最终同源候选仍须复验。 |
| tasks.named_queue_profiles | `queue_editor.rs`、`queues.slint` | StoreSettings、AssignQueue | 此前真实 UI 新增→保存→关闭→重开通过；最新统一样式候选再次真实 UI 新增/保存、主窗显示并经 Core 重读确认；批量分配、计划执行与限额组合待验收 |
| tasks.details_logs_speed_connections | `DetailsWindow`、`workbench.rs` | GetTaskLog、TaskAction(speed) | 详情、图表、日志、限速已接；图表/连接显示和多窗口压力待验收 |
| tasks.refresh_signed_url | 详情“更新并继续” | RefreshTaskRequest | 已接；2026-11-10 暂停后刷新地址并发布新响应的真实本机 HTTP 回归通过，保持原 5 秒门槛；新 UI 带凭据失败恢复待验收 |
| create.protocols_and_recognition | `TaskEditorWindow`、`resource_tools.rs` | ProbeUrl、CreateTask、HarvestPage、ImportPaths | 新建、批量、cURL、轨道选择已接；全部协议的真实 UI 往返待验收 |
| import.file_picker_and_drop | 文件选择器、winit DroppedFile、共享 Windows 文件交互 | ImportPaths | 2026-11-09 实窗多选复制 2 项经 Windows CF_HDROP 重读确认，Core 后台复制通过；Explorer 三文件粘贴导入两份有效列表，无效列表未新增任务，GUI 明确提示部分成功。OLE 跨窗口拖出与 Esc 取消未验收；旧关机通知已过期 |
| export.normalized_task_list | 工作台导出 | ExportTasks | 2026-11-09 真实另存为导出 5 项，schema、内容与无合成 Cookie 泄漏检查通过；未将此扩大为所有导出格式通过 |
| import.exported_task_json | 工作台导入 | ImportPaths | 两份 Core 格式列表经真实 GUI 粘贴导入成功；真实导出内容已检查，导出文件再次导入的完整往返仍待验收 |
| workbench.automatic_responsive_layout | 工作台、主题设置 | 窗口尺寸、task_sort | 自动布局保留；2026-11-13 实际 620×480、960×640、1400×820 HWND 尺寸和四档 Slint 缩放通过，工具栏控件在窗口内。实际 OS DPI 为 96，软件缩放不冒充物理多显示器切换。 |
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
| browser.online_subtitle_translation | 扩展字幕、`ServiceSettingsWindow`、`player_subtitles.rs` | configuration/configure/start/audio/reset/poll/stop | 本地配置、DPAPI 保存、主窗/PiP 字幕 HUD、按需工作线程与 Core 自有字节有界解码已接。2026-11-11 补齐现有预取模式的字幕不足自动暂停、准备完成恢复、用户暂停优先、偏移与速度对应的时间轴、禁用/错误退出及旧会话失效；本轮源码验证见 `native-migration-progress.md`。rc2 包不包含此修复；新包 GUI 和真实云质量未计通过 |
| accessibility.automation | Slint accessibility、工作台控件与列表标签 | 无 | 已实际读取 UIA 控件树并完成中文搜索和部分操作；输入法组合输入、完整 Tab/读屏未验收 |
| performance.release_thresholds | Release 构建和性能脚本 | 完整组合 | c42636a4 成品 Host/IPC/真实传输通过；1000 任务实际重绘 1196 帧、约 18.32 秒、p95 15.88 毫秒。1811.484 秒持续负载完成 157 次下载和一次保留 1000 任务的重启，峰值工作集 43.42 MiB，空闲句柄峰值增长每进程 1。开发诊断不替代最终同源门禁；没有同条件 IDM 性能对照。 |
| package.install_upgrade_rollback | 原生 launcher、WiX、Portable、Updater | 既有更新合同 | rc3 及 c42636a4 成品均通过真实 EXE 安装/快捷方式启动、MSI 升级/故障回滚、Portable 升级/回滚。旧 schema 迁移通过；不支持 schema 的报错/退出/原数据库字节保全通过。最终正式 EXE 仍须独立安装复验。 |
| free.cli | `native_clients` CLI | 共享客户端、原有命令 | c42636a4 成品通过单 Core 启动、下载/暂停/重启恢复、队列、导入导出、显式文件删除、stdin 上下文和默认脱敏；不扩大为任意命令和退出码组合保证。 |
| free.crawl_sync | `ResourceToolsWindow` 抓站模式 | Crawl 各动作 | c42636a4 成品 API 回归通过循环/深度/跨域边界、ETag、失败保留旧文件、队列窗口及定时同步；真实原生 GUI 创建、后台发现、刷新勾选保留及入队下载通过。 |
| free.zip_preview | `ResourceToolsWindow` ZIP 模式 | PreviewZip | c42636a4 成品远程 ZIP 元数据有界读取、资源变化和 2200 项中央目录回归通过；真实原生窗口输入远程地址并列出归档内容通过。 |
| http.active_tail_split | 现有实验开关、Core RangeScheduler | 调度内部 | 已校验的 HTTP Range 资源默认启用动态尾块；三轮真实成品比较的均匀场景约 1.44 秒持平，慢尾场景 raw/WinHTTP 代理约从 5.07 秒降至 2.03/2.04 秒，暂停及强杀恢复通过。比较对象、完整报告和额外服务端写入量分别记录；不是同条件 IDM 比较。 |
| http.coordinated_range_stop | `http_engine.rs` 范围 worker、正文循环及 TCP/WinHTTP/curl 等待 | 调度内部，无协议变化 | 2026-11-09 补齐共享停止到传输层的传播，保留暂停原因与首个错误；单流接收原 200 响应时解除范围停止绑定。2026-11-10 最终 HTTP 83 项通过、8 项忽略，覆盖本机 TCP/WinHTTP 响应头、正文、chunked 停滞及原响应复用。curl 新停止分支未运行专用组件回归；DNS/连接阶段仍有底层超时边界。最新成品验证见实施记录 |
| http.ipv6_download | `http_engine.rs`、`credentials.rs` | 既有 HTTP 下载与媒体请求 | 2026-11-11 支持方括号 IPv6 字面地址，统一 Host、重定向、连接复用与来源判断；Cookie 域匹配支持压缩/完整/IPv4 映射写法并保持跨源隔离。HTTP 86 项、凭据 17 项回归通过，包含本机 IPv6 TCP/WinHTTP 的真实 Range 与重定向。不支持 zone identifier；未将本机 HTTP 测试记为公网 IPv6 HTTPS 验收。rc2 包不包含此修复 |

每次完成验证应在 `native-migration-progress.md` 登记实际日志、二进制身份和范围，再更新本表。模拟 Core、真实 Core、本地媒体和真实浏览器交接的证据不能互相替代。
