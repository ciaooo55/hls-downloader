# 成熟下载器逐项对照审计

这份文档记录可复核的代码/行为证据和本项目的整改状态。它不是“功能名称对照表”，每一项都必须能落到实现文件和回归测试。

## 对照基线与边界

- AB Download Manager：`amir1376/ab-download-manager@6adc53c`。
- AB 浏览器集成：`amir1376/ab-download-manager-browser-integration@2adb8d5`。
- Streamlink：`streamlink/streamlink@4aa0943`。
- N_m3u8DL-RE：`nilaoda/N_m3u8DL-RE@e113dee`。
- 本机 IDM 6.42：检查了官方 Chrome 6.42.60、Firefox 6.42.59 插件包和 Native Messaging/网络组件。

IDM 下载核心闭源。本项目只能依据插件、已安装组件、公开文档和可观察行为提炼机制，不能声称看过其核心源码，也不复制其混淆代码。

## 浏览器普通下载接管

### 成熟实现证据

- ABDM 在 `DownloadLinkInterceptor.ts` 用 requestId 保存请求头和重定向链，在 `onHeadersReceived` 根据 frame 类型、GET、响应码、Content-Disposition/文件扩展名和最小大小决定接管；应用未响应时可放行浏览器。Chromium 无法阻断响应时，再由 `downloads.onCreated` 清理已接管项目。
- IDM Chromium 插件在 `downloads.onCreated` 立即暂停项目，把 DownloadItem 与极短时间窗口内的网络请求记录绑定，再交给 Native Host/主程序；Firefox 还使用阻塞式 webRequest。IDM 同时有浏览器捕获 DLL、网络监控 DLL 和 WFP/TDI 组件，扩展不是完整实现。

### 本项目状态

- 请求、重定向、请求体、响应头和页面身份：`extension/lib/requestChain.ts`。
- 用户点击意图与 OAuth 排除：`extension/lib/clickIntent.ts`、`extension/lib/resources.ts`。
- Chromium 响应头提前 offer、DownloadItem 暂停/恢复/清理和 Firefox 阻断路径：`extension/entrypoints/background.ts`。
- Native Messaging 长连接、重连和多浏览器 client 身份：`extension/lib/nativeBridge.ts`、`backend/native_host.py`、`backend/app/browser_handoff.py`。

本轮整改：DownloadItem 现在在读取设置或等待文件名之前立即暂停；已有响应头证据的路径不再额外等待 `onDeterminingFilename`。明确下载控件的意图从 `click` 前移到可信 `pointerdown`，后续 `click` 会去重；键盘激活仍由 `click` 处理。`ClickIntentStore` 把 MV3 session 恢复、并发 hydration 和消费规则独立出来，轮询未命中不再每 50 ms 写一次 `storage.session`。桌面端未呈现或用户拒绝时仍恢复原浏览器下载。

真实 Edge 接管 smoke 还覆盖了浏览器项目在暂停瞬间短暂变为 `interrupted` 的状态；恢复逻辑现在也处理该可恢复状态。popup 的自动接管设置采用本地先写入、带版本号的待同步队列，Native Host 短暂断开时按钮仍立即生效，重连后只同步最新一次选择。popup 在异步初始化完成前禁用设置按钮，避免快速点击丢失；“自动接管”状态显示使用桌面实际返回值，不再把“返回值等于请求值”误当成布尔状态。

右键 MSE/`blob:` 播放器不会再伪造一个普通文件任务，而是打开当前播放器的已关联资源面板；可验证的 HTTP 媒体源仍走快速直发路径。资源 session 写入按页面键串行化，避免多个同时到达的 HLS、音频和 MSE 事件互相覆盖。

仍有平台边界：Chrome MV3 不允许任意响应阻断，因此无法像 Firefox MV2/阻塞接口一样保证浏览器完全不创建 DownloadItem；正确做法是尽早暂停并保留失败放行，而不是先取消再冒险重建带 Cookie/POST 的请求。

## 媒体识别与播放器归属

### 成熟实现证据

- ABDM 只在 tab 级收集 `media` 响应和 URL 匹配 `*.m3u8` 的 XHR，再解析 HLS 信息显示全局媒体列表；它没有通用的“每个播放器旁精确归属”能力。
- IDM 内容脚本结合 video/audio/object/embed DOM、当前鼠标元素、窗口/全屏坐标、媒体 URL/内部标识、MutationObserver，以及主程序下发的站点选择器/规则。它的准确率来自通用证据、站点适配和原生网络层共同工作，不是单一正则。

### 本项目状态

- MAIN world 的 fetch/XHR/MSE 观察：`extension/entrypoints/hooks.content.ts`。
- 每个 video 的播放会话、悬浮窗和资源选择：`extension/entrypoints/content.ts`。
- 广告/字幕/缩略图过滤、短签名去重、播放证据打分：`extension/lib/resources.ts`。
- HLS/DASH 浏览器侧清单检查：`extension/lib/hlsInspection.ts`、`extension/lib/dashInspection.ts`。

本轮整改：

1. 扫描并监听开放 Shadow DOM 中的视频；用 `composedPath()` 取得真实 video，避免事件被重定向到 Shadow host 后漏识别。
2. MSE 数据来源覆盖 `Response.arrayBuffer/blob/bytes/clone`、`Blob.arrayBuffer` 和 `ReadableStream` reader，不再只支持最简单的 `fetch().arrayBuffer()`。
3. HLS 检查保留最近的 EXTINF、MAP、PART、PRELOAD-HINT URL。播放器实际 append 的分片可与具体清单精确绑定，即使多个清单位于同一 CDN 目录。
4. DASH 新增 MPD 类型、时长、最高分辨率、码率、估算大小和 SegmentTemplate/BaseURL 证据提取；动态 MPD 不伪造总大小。
5. response Blob 作为 video `blob:` 地址播放时，可回溯到原始直链媒体。
6. 动态 `attachShadow()` 会由 MAIN world 发出只携带 DOM 路径的通知，开放 ShadowRoot 在创建后也能立刻进入监听，不依赖低频全页轮询。
7. 已实际播放的 `video.currentSrc` 当场登记为播放器级证据；Firefox iframe 不再依赖不稳定的 PerformanceObserver/webRequest 回传。
8. MSE 所有权会穿过 `ArrayBuffer.slice()`、`Blob.slice()` 和 TypedArray `slice()`，避免播放器整理字节后退化为页面级猜测。
9. 显式广告路径和查询参数会在资源候选阶段排除；广告切主片后按钮 ID 必须切换到主片资源。

本轮继续整改：

- `RequestChainStore` 现在限制请求头数量、单字段长度和总大小，并清理 CR/LF；POST 重放体在复制前检查 `byteLength`，表单字段也有数量/大小上限，避免异常页面把扩展后台变成无界内存缓存。
- HLS 解析记录 `EXT-X-CUE-OUT/CUE-IN`、SCTE-35/DATERANGE 和明确广告路径；桌面端按设置默认只跳过这些“有证据”的广告分片，普通文件名中包含 `ad` 不会被误删，断点处自动写入 discontinuity。
- 播放器存在多个未检查自适应清单时，悬浮窗的保守等待从 1.5 秒降为 650 毫秒；已经与 `currentSrc` 或 SourceBuffer 证据精确匹配的资源仍然立即显示。
- 浏览器侧 HLS/DASH 清单检查使用 `boundedResponse.ts` 流式读取并限制 2 MiB；没有 `Content-Length` 的响应也会在超限的第一个 chunk 立即取消，避免异常站点拖垮 MV3 后台。
- 右键“批量发送选中的链接”现在会切换到显式选择模式并打开资源面板；此前只写入资源缓存但没有活动播放器时，面板仍为空，实际功能不可见。
- 清单探测现在沿用“实际页面来源 + 精确 CDN Cookie + 可重放应用头”三类证据；若扩展环境拒绝上下文头，会自动用无上下文的安全头重试。此前探测主动丢弃 Referer/Origin，登录态或防盗链清单会被误判为不可解析。
- 浏览器侧 HLS master 解析保留 CODECS，并在存在视频变体时排除高码率 audio-only 变体，避免“最高码率”把音频播放列表当成当前视频。
- iframe 资源列表按 `sender.frameId` 过滤后再压缩；写入共享页缓存时同一 URL 的不同 frame 也保持独立，popup 仍可聚合显示，但各 frame 的悬浮窗不会把兄弟播放器的 MSE 清单当成自己的候选（`extension/lib/resources.ts`、`extension/entrypoints/background.ts`）。
- MAIN-world fetch/XHR 对带 manifest URL 提示或 manifest MIME 的响应做 128 KiB 前缀嗅探；即使 CDN 使用无扩展 URL + `application/octet-stream`，也能在不消费真实播放响应的情况下补报 HLS/DASH 类型（`extension/lib/manifestSniff.ts`、`extension/entrypoints/hooks.content.ts`）。
- HTTP 元数据探测对“响应头已有可信大小/媒体 MIME，但首个 body chunk 被边缘节点延迟”的情况增加 3 秒有界等待并安全放行，避免长时间停留在“读取文件信息”；HTML/JSON/文本错误仍必须读取前缀并拒绝（`backend/app/downloader/http_file.py`）。
- 直播在尚未写入任何分片时若首批媒体请求全部失败，现在保留最后一个 worker 的错误（例如签名 403）并报告“首批分片下载失败”，不再伪装成“清单长时间没有新分片”；已有录制内容仍按停播收尾（`backend/app/downloader/hls.py`）。

已知边界：closed Shadow DOM 无法被扩展读取；多个播放器共用完全相同的 MSE 管线且页面不暴露任何可区分证据时，通用扩展不能保证一一归属。此时应显示选择而不是把别的视频放到当前播放器旁。DRM/EME、SAMPLE-AES 不绕过。

## HTTP 多连接、恢复与“最后一点”

### 成熟实现证据

- ABDM 的 `HttpDownloadJob.kt`/`PartSplitSupport.kt` 会在空闲连接出现时动态拆分剩余 part，并校验长度、ETag 和服务器续传能力变化。
- IDM 公布的“动态文件分段”行为同样会重用连接并拆分慢尾，但具体算法闭源。

### 本项目状态

- `backend/app/downloader/http_file.py` 已实现 Range 探测、Content-Range 严格校验、ETag/Last-Modified/If-Range、断点 checkpoint、动态 end-game 拆分、无 Range/未知长度回退、POST 单流和最终文件原子发布。
- 本地真实 smoke 已验证 24 个不同 Range worker 请求、同时连接、暂停/恢复、无 Range 回退、签名 URL 从 403 刷新后恢复以及最终 SHA-256，不以 UI 进度代替磁盘长度校验。
- `backend/app/network_proxy.py` 提供进程级连接预算、每 Host 上限和 429/503 共享退避，避免“单任务并发 × 多任务”失控。

## HLS、LL-HLS、DASH 与直播收尾

- Streamlink 的直播循环按 target duration/末段时长重载，以 media sequence 判重，清单未变化时缩短重载间隔，并把 ENDLIST 与滑动窗口分开处理。
- N_m3u8DL-RE 对 init、加密、discontinuity、轨道与 mux 有独立模型，不把 fMP4 字节简单拼接成普通 MP4。
- 本项目在 `backend/app/downloader/hls.py` 保存直播 journal/checkpoint、媒体序列和 discontinuity，支持 LL-HLS PART、独立音轨、短签名刷新、停播收尾与恢复。
- LL-HLS 轮询使用 `EXT-X-PART-INF:PART-TARGET`，不再套普通 HLS 的一秒/目标分片下限；下载和状态写入耗时会从下一次 reload 间隔扣除。DASH 动态 MPD 同样按 `minimumUpdatePeriod` 的严格时间线轮询，避免每轮处理后再额外等待而逐步落后直播窗口。
- 对声明 `EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES` 的 LL-HLS，`hls.py::_blocking_reload_url()` 会用最后的 media-sequence/PART 游标发送 `_HLS_msn`/`_HLS_part` 阻塞式 reload；遇到不支持游标的边缘节点会自动回退到无游标的 no-cache 请求，且保留签名参数。回归覆盖游标替换、签名保留和 400 回退。
- HLS 解析遇到正在重写窗口的 `EXTINF` 空 URI 时只跳过该条撕裂记录，不再因单个“缺少 URI”直接终止独立音轨/直播；只有整份清单没有任何可用分片才报告解析失败。
- `backend/app/downloader/merge.py` 用本地 ENDLIST HLS 清单保留 EXT-X-MAP/EXTINF/discontinuity，再让 FFmpeg 生成时间戳；无损输出时长异常会自动重编码并再次用 ffprobe 验证，避免 fMP4 `tfdt` 导致超长视频。
- `backend/app/downloader/dash_native.py` 与 `mpd.py` 负责 DASH 轨道和动态清单；收尾时以本地 HLS 时间轴重建每条 DASH 轨道，再交给 FFmpeg mux，避免把带绝对 `tfdt` 的 fMP4 片段直接字节拼接成超长文件。多 Period 等原生未覆盖情形仍可受控回退 yt-dlp，而不是假装原生支持。
- HLS 点播/直播 WebVTT、AES-128 字幕与 DASH WebVTT/TTML sidecar 已有独立下载、合并和测试；DRM 字幕仍遵守同一不绕过边界。

## 安装、升级、数据库与日志

- 安装/升级用 NSIS 和 Tauri 单实例关闭协议；覆盖安装 smoke 同时验证配置、数据库和下载文件保留。
- SQLite 在 `backend/app/database.py` 使用生命周期连接、WAL 和启动迁移，不在每次进度保存时重新建表。
- TaskManager 对任务 URL、来源页、Cookie、请求头、请求上下文、POST Body 以及 HLS/DASH 选择的变体地址统一使用 DPAPI；旧版明文选择字段可兼容读取，下一次保存会迁移为加密值。
- `TaskManager` 使用有界异步日志队列和轮换；SSE 队列也有容量上限。
- 插件商店版由浏览器更新；开发者模式解压插件受浏览器安全模型限制，不能伪装成在线静默更新。

## 当前验证证据

- Python：当前全量测试 533 项通过；本轮解析器、DASH 时间轴、任务迁移针对性回归通过。
- 扩展：当前 TypeScript/Vitest 141 项通过；Edge 与 Firefox 各 12 个真实浏览器场景（上次可用驱动验证）包括 direct、启动/动态 Shadow DOM、同源/跨源 iframe、广告切主片、流式/切片后 MSE、双 MSE、HLS、纯 PART LL-HLS 和 DASH，浏览器错误均为 0。
- 源码真实下载：24 MiB Range 并发及暂停恢复、6 MiB 无 Range、签名 403 刷新、HLS 增量播放、0.333 秒 LL-HLS PART、DASH、ffprobe 时长验证与任务/文件清理全部通过。
- 真实 Edge 普通下载接管 smoke：接受前暂停在 0 B、接受后清除浏览器副本；拒绝和 Native Host 连续断开后均恢复并完成 8 MiB；真实 popup 的排除本站与自动接管开关通过。
- CI 和发布工作流现已调用真实 Firefox 媒体归属 smoke；发布工作流还执行便携真实下载、便携覆盖升级和安装版覆盖升级。

## 仍需保留的边界与后续验证

1. 真实 Chrome/Edge/Firefox 同时运行、不同插件版本并存时的 Native client 隔离已有代码与单元覆盖；当前发布烟测已加入真实 Edge 接管的暂停、接受、拒绝和断线恢复，但仍应在发布候选产物上做一次长时间人工连接观察。
2. closed Shadow DOM、DRM/EME，以及多个播放器共用完全相同且页面不暴露任何区分证据的 MSE 管线，通用扩展无法可靠一一归属。此时保持“选择资源/不显示”，不能误绑。
3. DASH 静态多 Period 在各 Period 使用相同初始化段和编码时已由 `mpd.py` 展平到原生分片引擎；未声明 `Period@duration` 时会从 SegmentTimeline 推导时间轴偏移，字幕按语言/标签跨 Period 合并。动态多 Period、初始化段/编码变化仍受控回退 yt-dlp，因此这些边界不能声称原生分片进度/暂停语义完全一致。
