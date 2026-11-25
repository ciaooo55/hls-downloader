# 更新日志 (Changelog)

本文件记录 **HLS Downloader** 各版本面向用户与开发者的主要变更。

日志格式参考 [Keep a Changelog 1.1.0](https://keepachangelog.com/zh-CN/1.1.0/)，各版本的完整说明与升级注意事项请参阅 `docs/releases/` 目录下的对应发布文档。

---

## [Unreleased] - 10.1.0 Candidate

> **状态说明**：`10.1.0` 当前处于候选迭代与发布前验证阶段（**尚未发布正式安装包**）；最新已发布正式稳定版为 [`10.0.0`](#1000---2026-11-21)。完整草案见 [docs/releases/v10.1.0.md](docs/releases/v10.1.0.md)。

### 新增 (Added)
- **右侧非模态任务检视抽屉**：支持通过工具栏或 `Alt+I` 展开/收起右侧内联任务检视抽屉，实时查看传输速度曲线、`SegmentMapBar` 分片落盘状态图、连接汇总与诊断信息。
- **双轨音视频并发下载与无损封装**：新增 `audio_url` 双轨并发传输与 FFmpeg 自动无损 `.mp4` 混流管线（原生支持 Bilibili `/ogv/player/playview` 及分离 `.m4s` 音视频合流），默认开启点播 HLS `.ts -> .mp4` 无损重封装并支持 FLAC-in-MP4 封装。
- **浏览器 HTTP GET 直连多线程接管加速**：扩展将普通 HTTP/HTTPS `GET` 下载、HLS（`.m3u8`）、DASH（`.mpd`）及双轨流通过 Native Messaging 直接移交桌面 Rust Core 执行多连接分段加速；一次性 `POST` 表单与 `Blob` / `data:` 负载保留浏览器响应接收后再安全导入。
- **扩展快捷授权、DRM 识别与常驻快捷栏**：新增一键 `[授权本站]` 站点 Cookie 授权横幅、常驻快捷设置栏、网页视频悬停一键画质胶囊（精确透传 `preferred_variant_id`）、搜索与分类筛选面板，以及 Widevine / PlayReady / SAMPLE-AES 受保护流识别提示与 YouTube MSE 分片噪声过滤。
- **过期签名 URL 刷新续传与归一化去重**：支持保留已落盘分片与字节检查点刷新过期签名地址无缝续传，提供浏览器辅助刷新入口（`ArmTaskRefresh`）与客户端/服务端统一的预签名 URL 规范化去重识别（`canonical_resource_key`）。
- **原生 FTP/FTPS 与 SFTP 代理隧道**：控制连接、被动数据连接及 `russh` SFTP 会话全面支持 HTTP CONNECT（Basic 鉴权）、SOCKS5/5h、SOCKS4/4a 与 Windows WinINet 按 URL 系统代理路由。
- **工作台高频效率交互**：新增全局快捷键（`Ctrl+F` 搜索、`Ctrl+,` 设置、`Alt+I` 检视抽屉、`Space` 暂停/恢复）、通配符批量 URL 生成、工具栏一键限速与主题切换胶囊、现代 COM 目录选择器、可拖拽交互任务列表滚动条，以及首次关闭至托盘时的系统原生气泡提示。

### 变更 (Changed)
- **旗舰级视觉体系与原生字体排版**：全面升级 **Fluent Graphite + Luminous Cobalt** 深浅双主题与双模式进度 HUD（详情视图 / 紧凑迷你条）；采用系统原生多字重 `Microsoft YaHei UI` 渲染（Regular + Bold 清晰度提升，安装包与运行体积缩减约 16.4 MB），统一二级弹窗系统原生标题栏；设置窗口（`SettingsWindow`）采用带未保存变更提示的统一事务化保存，日常限速与主题切换由主工作台工具栏胶囊一键直达。
- **Core 原生 `WinHTTP` 优先传输与静默快下闭环**：分片拉取默认优先采用原生 `WinHTTP` 引擎并在命中 WAF 拦截（如 403）时自动回退 `curl-impersonate`；Core 端直接支持静默快速下载自动接收，并在后台交接事件到达时按需冷启动唤醒 Presenter。
- **零轮询异步 IPC 与低功耗空闲态**：以 Win32 Overlapped Named Pipe I/O 取代轮询等待，消除空闲周期写盘开销，并持久化托盘主动退出标记以防浏览器后台探测意外拉起引擎。

### 修复 (Fixed)
- 消除程序启动阶段的幽灵弹窗竞态（startup ghost-popup race），并为 `CompleteWindow` 完成提示窗增加 10 秒鼠标悬停感知自动关闭（悬停时暂停倒计时）。
- 对齐 DASH 点播批量检查点刷盘策略（32 分片 / 64 MiB / 1 秒），修复 `WafCurlCache` 403 回退恢复与 `Alt` 点击绕过、页面链接批量提取等多项浏览器接管边缘场景。
- 收紧浏览器辅助刷新链接（`ArmTaskRefresh`）双入口匹配校验（杜绝等待期内误覆盖无关标签页下载）并补齐 `admit_handoff` 路径 `audio_url` 协议与白名单校验。
- 修复双轨 HTTP 下载单轨报错时误清双轨临时文件与断点状态的问题（仅在用户真实取消时清理，并在重试/续传时直接复用已下完轨道）。
- 修复 HLS `select_audio_track` 忽略无独立 `URI` 的 `DEFAULT=YES` 内嵌原声 rendition 导致误混流外部评论音轨的问题；保留 `.kmz`、`.3mf` 等合法非危险 ZIP 容器后缀，并统一设置窗口事务化保存状态追踪以确保队列繁忙或校验报错后可正常点击「保存设置」重试。

### 安全 (Security)
- **Win32 Job Object 与无窗口子进程隔离**：通过 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 与 `CREATE_NO_WINDOW` 统一托管 FFmpeg、`curl-impersonate` 及外部 SFTP/SSH 辅助子进程生命周期，杜绝主引擎退出残留孤儿进程或闪现控制台黑窗。
- **代理 Fail-Closed 与凭据保险库加固**：代理握手或鉴权失败严格拒绝直连回退；代理凭据统一纳入 Windows DPAPI 加密存储与日志脱敏，并强化自动更新单源签名校验与 SSRF / 重定向边界。
- **全量生产代码 `unsafe` 与 `.unwrap()` 审计加固**：启用 `#![cfg_attr(not(test), warn(clippy::undocumented_unsafe_blocks))]` 为生产 `unsafe` 块与 `unsafe impl` 补齐 `// SAFETY:` 不变量说明，并清理生产代码路径中的未受保护 `.unwrap()`。

---

## [10.0.0] - 2026-11-21

> 完整发布说明见 [docs/releases/v10.0.0.md](docs/releases/v10.0.0.md)。

### 新增 (Added)
- **主题跟随系统与双色切换**：支持“跟随系统 / System”“浅色 / Light”“深色 / Dark”三种模式，实时响应 Windows 应用主题变更并兼容历史 `dark_mode` 配置（#120）。
- **速度曲线数值统计**：任务详情速度图新增当前速度、峰值速度与可用样本算术平均速度统计（#120）。
- **完成提示音即时试听**：在完成提示音开关旁新增“试听提示音 / Preview sound”按钮，直接复用真实完成提示音播放路径（#120）。

### 变更 (Changed)
- **确立纯原生 Rust Core + Slint 产品线**：保持 `%LOCALAPPDATA%\HLS Downloader\v7` 数据目录、SQLite schema 7、IPC 管道、Native Messaging Host 与 MSI `UpgradeCode` 升级身份不变，支持从 `7.x` 原地覆盖升级至 `10.0.0`。

### 移除 (Removed)
- 移除历史 Kotlin Compose 桌面工作台及仅为其服务的 Gradle / JDK 构建、CI 与验证链路（#119）。

---

## [7.0.10] - 2026-11-20

> 完整发布说明见 [docs/releases/v7.0.10.md](docs/releases/v7.0.10.md)。

### 新增 (Added)
- **磁力链接 `xs` 元数据入口支持**：支持仅包含 `xs` 元数据地址的磁力链接，并在提供 info hash 时先校验元数据身份再应用文件布局。
- **多实例安装器安全停机协调**：安装器支持核验并等待同一安装目录下多个 Core（含私有管道实例）保存下载检查点后安全停止，不影响其他安装目录的运行进程。

### 变更 (Changed)
- **BitTorrent 多文件目录结构原子交付**：普通新建入口的多文件种子默认按原目录结构全选下载，全部文件写入同步后整体发布，进入交付阶段后锁定文件选择并校验所选数据完整性；同名目录支持改名或 `skip` 保留策略。
- **托盘安全退出保护**：托盘退出前先停止活动传输并落盘断点；若保存失败则保留 Core 与托盘并提示稍后重试。

### 修复 (Fixed)
- 修复 EXE 安装器自定义安装目录（含中文路径）未正确生效的问题。
- 修复媒体代理重复 socket 句柄问题，并兼容不支持 `extension_picky` 参数的 FFmpeg 构建版本。

---

## [7.0.9] - 2026-11-19

> 完整发布说明见 [docs/releases/v7.0.9.md](docs/releases/v7.0.9.md)。

### 新增 (Added)
- **浏览器扩展启动桌面端状态反馈**：从扩展拉起主程序时实时展示启动状态，清晰区分 Native Host 未注册、Core 连接失败与工作台启动失败，并提供重试入口。
- **DASH 直播跨 Period 连续录制**：在轨道编码兼容时支持 DASH 直播跨 Period 持续录制。

### 修复 (Fixed)
- 优化小尺寸下载确认窗的操作可达性与长地址/长路径展示，并在保存前拦截无效文件名、不可用目录与重名冲突以保护已有文件。

---

## [7.0.8] - 2026-11-18

> 完整发布说明见 [docs/releases/v7.0.8.md](docs/releases/v7.0.8.md)。

### 修复 (Fixed)
- 修复 DASH 清单仅声明音轨类型时音频轨道选择失败的问题。
- 优化分段点播多 Period 轨道衔接：后续 Period 提供多种编码时优先选择可与上一段无损拼接的兼容轨道。

---

## [7.0.7] - 2026-11-17

> 完整发布说明见 [docs/releases/v7.0.7.md](docs/releases/v7.0.7.md)。

### 新增 (Added)
- **原生界面主题整合**：统一原生工作台、设置、详情、任务工具、轻量弹窗与浏览器扩展弹窗/网页媒体浮层的石墨与钴蓝视觉样式。
- **FTP/FTPS 多连接与主动模式回退**：支持 FTP/FTPS 多连接并发传输、主动模式回退与暂停恢复资源身份校验。
- **DASH 多形态清单与多 Period 点播**：支持 `SegmentList` 字节范围、`SegmentBase`/`SIDX`、整文件 `SegmentBase` 及兼容的点播多 Period 时间线边界保留。
- **HLS 点播批量检查点与 MP4 封装**：支持分片批量同步落盘后记录检查点，并提供 TS 无损封装为 MP4（保留多音轨，封装失败时自动保全原始文件）。

### 变更 (Changed)
- **浏览器交接凭据与一次性响应保全**：完整保留签名 URL、已授权 Cookie、`Authorization` 与精确 `POST` 正文，明确区分宿主未安装与浏览器禁止访问状态，并保持持久接收回执语义。
- **内存进度镜像与后台周期落盘**：运行中任务采用内存控制与合并进度上报，周期性 SQLite 写入交由后台线程异步处理，状态机关键转换保持同步持久化。

### 修复 (Fixed)
- 修复异常退出后因周期写库滞后导致进度归零的问题，支持按可靠 HTTP 范围检查点恢复已下载字节并重新验证远端资源身份。
- 修复 HTTP / `curl` 有界流式缓冲、正文完整性校验、并发槽位限制及进度冲刷序号阻断后续更新的问题。

[Unreleased]: https://github.com/ciaooo55/hls-downloader/compare/v10.0.0...HEAD
[10.0.0]: https://github.com/ciaooo55/hls-downloader/releases/tag/v10.0.0
[7.0.10]: https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.10
[7.0.9]: https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.9
[7.0.8]: https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.8
[7.0.7]: https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.7
