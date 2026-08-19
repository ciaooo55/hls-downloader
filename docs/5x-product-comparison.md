# HLS Downloader 5.x product and architecture comparison

这份矩阵用于约束 5.x 升级方向，不把任何竞品的实现或界面直接复制进项目。
产品能力按当前公开资料和本仓库 3.x 功能基线对照；实现状态以代码和测试为准。

## Reference products

| 产品 | 可借鉴能力 | 5.x 不照搬的部分 |
| --- | --- | --- |
| [AB Download Manager](https://github.com/amir1376/ab-download-manager) | 队列、调度、限速、浏览器扩展、跨平台现代 UI；主应用使用 Kotlin Multiplatform/JBR/Compose | JVM 常驻工作集和跨平台抽象不适合 Windows 热路径；当前公开 issue 仍显示手动 `.m3u8` 可能只保存清单，不能替代本项目的媒体引擎 |
| [Internet Download Manager](https://www.internetdownloadmanager.com/support/segmentation.html) | 动态二分最大剩余段、连接复用、连接数/超时调节、频繁保存位置；浏览器接管和队列弹窗 | 闭源 Windows 集成、浏览器注入和专有协议不进入本项目；只借鉴行为契约 |
| [Free Download Manager](https://www.freedownloadmanager.org/features.htm) | 分段并行、断点恢复、BitTorrent、智能目录、计划任务、限速、预览和多语言 | 不把全部功能塞进单一进程；媒体时间轴仍由 HLS/DASH 专用引擎处理 |
| [aria2](https://github.com/aria2/aria2) / [Motrix](https://motrix.app/) | HTTP/HTTPS、FTP/SFTP、BT/Metalink、多源、RPC、磁盘缓存、选择性下载和清晰任务列表 | 不以通用 RPC 引擎替代浏览器媒体归属、Windows 原生确认窗口或现有 Python 媒体能力 |

## 5.x target architecture

```text
HLSNativeShell.exe (Rust/Win32, resident)
  tray + pre-created handoff/progress/complete windows
  native HTTP Range hot path, direct logical-file writes
        │ length-prefixed JSON / authenticated loopback
HLSDownloaderCore.exe (Python 3.12 + FastAPI + SQLite)
  HLS/LL-HLS/DASH/live/BT/FTP/SFTP, scheduler, checksums, FFmpeg, cast
        │ on demand
HLSDownloader.exe (Tauri 2 + React 19 + TypeScript + Vite)
  settings, task workbench, player and complex forms
        ▲
WXT MV3 extension (Chrome/Edge/Firefox)
  click intent, response chain, MSE/Blob ownership, manifest inspection
```

唯一持久化状态属于 Python Core。原生壳、扩展和 Tauri 只消费事件与快照，不能各自维护任务数据库或另一套状态枚举。

## Capability matrix

| 能力 | 3.x 基线 | 5.x 目标 | 当前证据 / 剩余工作 |
| --- | --- | --- | --- |
| 普通 HTTP(S) Range | Python 分段、断点和回退 | Rust 常驻热路径，动态尾段二分，直接按偏移写一个 `payload.downloading`，完成后只做本地发布 | Rust `native_shell/src/http_engine.rs` 已实现动态尾段、资源身份、`If-Range`、Windows WinHTTP `Content-Range` 校验、短响应续传重试和落盘同步；后续补真实公网 Windows 网络基准 |
| 续传一致性 | URL/ETag/Last-Modified 检查点 | Native/Python 共用资源身份、验证器、长度和持久化顺序 | Python 已有 v3 检查点；Native sidecar 使用版本/身份/验证器/长度并原子替换，活动游标按批次持久化；需补跨进程断电恢复 fixture |
| 二次网络下载 | 普通文件不能因并发重新拉全文件 | 所有 Range worker 写最终逻辑布局，完成时不再网络拼接 | Rust/ Python Range tests 覆盖单文件 seek 写入；媒体流仍按时间轴本地 mux，这是容器处理而非二次下载 |
| HLS / LL-HLS | TS、fMP4、直播、AES-128、断点 | 保留专用时间轴，处理 init/discontinuity/key/live window，下载和本地处理分阶段展示 | `backend/app/downloader/hls.py` 与 HLS tests 已覆盖；继续增加真实 master/variant/LL-HLS fixture |
| DASH | representation、init、音视频 mux、live | 保留时间轴和轨道边界，避免把 fMP4 当字节拼接 | `dash_native.py` 与 DASH tests 已覆盖；继续增加多 Period、BaseURL、字幕 fixture |
| 媒体识别 | URL/MIME/页面/播放器多来源 | `kind + evidence + owner + confidence + replay context`，多播放器和 iframe 不串源 | 扩展已有 MSE/Blob/manifest/iframe 关联；owner 现在使用页面内稳定 media-element ID 或请求 ID，replay metadata 限长并去除敏感 key/URL 查询串；5.1 仍需做 precision/recall fixture |
| 浏览器接管 | 点击、DownloadItem、Native Messaging | 先暂停浏览器下载，确认后原子转交；拒绝/超时恢复原下载；普通文件不等待播放器 | 扩展 takeover tests、handoff tracker 和 native host tests 已覆盖；需补 Firefox/Edge 实机矩阵 |
| 常驻占用 | Tauri/WebView 参与热路径 | 空闲由 Rust/Win32 壳托管；WebView 只在设置/播放器/复杂工作台打开 | `native_shell` + packaged smoke 已证明流程；需在 Windows release 包记录 P50/P95 工作集和点击到确认窗延迟 |
| UI/UX | 桌面和扩展各自演进 | 同一语义 token、状态词、焦点/键盘/错误态和弹窗层级 | 桌面/扩展共用 cool-slate/blue 语义 token，桌面弹窗已使用统一 z-index scale；仍需补 Light/Dark 截图与可访问性巡检 |

## Release gates

1. Python：ruff、mypy、全量 pytest。
2. Native shell：`cargo fmt --check`、`cargo check`、`cargo test --all`、Windows release build。
3. Frontend/extension：类型检查、Vitest、Vite/WXT Chrome+Firefox 构建。
4. Behavior matrix：Range/无 Range/中断/验证器变化、HLS TS/fMP4/live/AES、DASH 多轨、BT、FTP/SFTP、浏览器接管和恢复。
5. Package/smoke：隔离端口、认证 API、Native Messaging 多消息复用、核心重启、安装升级、Portable 升级/回滚。
6. Windows performance：空闲 Native Shell/Core/WebView2 工作集、确认窗 P95、普通 HTTP 网络二次下载字节数必须为 0。

## Sources

- AB Download Manager [documentation](https://abdownloadmanager.com/docs) and [source repository](https://github.com/amir1376/ab-download-manager).
- IDM [dynamic segmentation](https://www.internetdownloadmanager.com/support/segmentation.html) and [browser/options integration](https://www.internetdownloadmanager.com/support/options.html).
- FDM [current features](https://www.freedownloadmanager.org/features.htm).
- aria2 [protocol and segmented download features](https://github.com/aria2/aria2).
- Motrix [supported protocols](https://motrix.app/).
