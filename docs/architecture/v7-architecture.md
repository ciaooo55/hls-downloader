# v7 运行架构

## 唯一状态所有者

Rust Core 是下载、调度、任务恢复和 SQLite 的唯一所有者。Compose 工作台、Presenter 和 Native Host 通过协议使用同一个 Core，不直接读写数据库。

```text
WXT 扩展 → Native Messaging Host → Rust Core + SQLite
                                     ↕ 本地 IPC
                             Compose / Presenter
                                     ↓
                            隔离 libmpv 播放进程
```

| 模块 | 职责 |
| --- | --- |
| `native_shell/` | 下载协议、任务与设置、媒体服务、Core/Host/播放器进程入口 |
| `desktop_ui/` | 主工作台、任务与队列、设置、设备选择、主界面播放器浮层 |
| `presenter_ui/` | 预热的浏览器确认、下载进度和完成临时窗口，无 SQLite 访问 |
| `extension/` | 网页资源识别、下载接管、字幕与媒体推送，经 Host 提交给 Core |

关闭主工作台不终止活动下载。浏览器 offer 优先交给已驻留 Presenter，避免为确认下载等待 JVM 冷启动；Compose 的后备确认路径不能与 Presenter 重复接单。

## 进程与请求边界

Windows 正常 IPC 使用带 owner/SYSTEM DACL 的 `\\.\pipe\HLSDownloader.v7` 和长度前缀 JSON。可选测试 TCP transport 只接受 loopback。Core 创建 pipe 后才报告就绪，权限和创建失败必须返回错误。

浏览器请求保留顶层页面及资源身份。Cookie、Authorization 和自定义 replay header 按 origin 隔离；跨域请求不继承上一 origin 的凭据。UI 不持有浏览器凭据。

默认播放在主界面浮层内，播放控制也在该浮层。Core 启动 `--player-process` 子进程加载 libmpv，并以有界 JSON 通道控制；播放器崩溃不能终止下载。画中画使用独立宿主并复用会话，不另起一套下载或解码依赖。投屏/TVBox 控制与本地播放入口分开。

## 下载与字幕

完成和删除任务的缓存规则、HLS/DASH/HTTP 的边播边下条件见 [缓存与播放](../guides/download-cache-and-playback.md)。播放复用任务缓存，完成后切换成品并确认卸载旧输入后才清理。

字幕预取以有限音频窗口工作，只有云端对应窗口完成才计入就绪范围；不足时暂停补齐。服务、凭据及请求头边界见 [字幕配置](../guides/qwencloud-subtitles.md)。

## 媒体推送

浏览器 `media_push` 是持久化 Core handoff。Compose 打开与任务播放共用的来源明确的设备选择器，区分 DLNA/Chromecast 与 TVBox；Core 保存 done/failed/canceled，浏览器读取同一结果。设备发现由 Core 按模式执行，不让两类扫描互相覆盖。

直接 Core 调用成功不能证明真实浏览器、安装注册、选择器及接收端拉取组成的整条链路成功。真实设备证据要求见 [媒体推送门禁](browser-media-push-release-gate.md)。

## 构建和证据

运行包包含裁剪 JRE、Core、Host、Presenter、FFmpeg 和 libmpv；ffprobe 和完整扩展图标库不进入生产包。依赖按固定配置重建，源码中不保留依赖缓存或安装镜像。

候选包与正式包的门禁不同。canonical 功能合同、具体包 provenance 和某次实测报告各自描述不同范围；不能以一个布尔值或旧测试数量代替当前包的证据。见 [验证状态](../validation/v7-verification.md)与 [发布 runner](../releases/v7-release-runner.md)。
