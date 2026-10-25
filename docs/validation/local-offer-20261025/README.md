# 本地文件交接优先级修复

## 确认的根因与改动

POST/Blob 下载走浏览器完成保存、Core 复制本地文件的交接路径，发出的 Native Messaging 操作是 `offer_local_file`。旧 `NativeBridge` 用写操作白名单分配优先级，却漏了它：该请求落入最低优先级，与心跳相同。因此等待心跳时不能优先处理；它正在执行时，优先级更高的状态查询又能断开连接、将本地交接重新排队。协议的请求 ID 幂等保护仍在，但不能因此认为反复断线和重新发送没有问题。

现在仅明确的只读操作使用可抢占优先级：`ping`、`wait_handoff`、`handoff_status`、`media_push_status`；其他操作保持交互优先级，不再因白名单漏项而成为可抢断请求。字幕连接的动作仍同优先级，PCM 不抢断 poll。没有添加新队列、端口类型、配置、依赖、重试层或常驻进程。

产品源码提交为 `a28ead3`。生产修改仅在 `extension/lib/nativeBridge.ts`；Core、NativeHost、Presenter、Updater、桌面 JAR、JRE 与媒体组件复用基线 MSI，不使用已撤回的尾段实验 Engine。

## 实际验证

两项新回归先在旧代码执行，均失败；修复后连同既有 Bridge 回归 21 项通过。使用手动回复的事件协调请求顺序，验证本地交接优先于卡住的心跳、忽略旧端口回复，以及交接执行时状态查询等待、不重发交接。没有靠睡眠碰撞并发时序。完整插件类型检查及 **399 项测试**通过，21 项已包含其中，不能相加成 420 项。

Chromium/Firefox 生产构建与 ZIP 打包通过；Firefox `web-ext lint` 零错误、警告、提示。沙盒用实际 Edge 156.0.4314.42、Firefox 158.0.1、NativeHost 和已验证 Core，各通过 **18 个下载所有权场景**，见 [Edge](results/edge.json)、[Firefox](results/firefox.json)。确认决定通过真实 Core IPC 发出，没有把它描述为原生确认按钮或完整 UI 的实际点击验收。

场景覆盖直接 ZIP 接受/取消、无扩展名附件、用户的 GitHub EXE 点击后取消；媒体/音频/图片/JSON/HTML/未知类型附件、重定向、GET 表单及新标签；POST/Blob 导出；禁用、排除、明确离线。POST 原始提交一次、Blob 源 fetch 一次，最终导入文件与 1MiB 源内容逐字节相同，SHA-256 见结果；只有 Core 复制完成后才移除浏览器源文件。禁用、排除与明确离线均保留浏览器原始传输一次。

两个浏览器的端到端回归证明正常实际交接路径成立；“恰好在请求执行中收到状态查询”的时序由前面的可控单元回归覆盖，不能把普通浏览器回归说成真实进程内已经强制触发了同一并发时序。

## 夹具失败保留

首次 Edge 增量验收在 Blob 场景等待确认超时。第二次增加页面错误与下载创建事件采集，查到测试网页内联处理器的 `URL.createObjectURL` 抛出 `TypeError: URL.createObjectURL is not a function`，尚未发起 Blob 下载；不能当作插件漏接的证据。内联处理器改为明确使用 `window.URL.createObjectURL` 后，两种浏览器的完整矩阵通过。两次失败分别保留在 [首次记录](results/fixture-first-failure-edge.json)、[诊断记录](results/fixture-diagnostic-failure-edge.json)，没有抹去或计算为产品全量对抗通过。

首次单元回归启动命令的日志目录误算在 `extension/.tool-cache` 下，重定向失败，测试并未启动；修正为项目既有 QA 目录后才得到前述旧代码失败结果。没有为此创建额外根目录或把未执行记为通过。

## 成品与边界

基线 MSI SHA-256 为 `e6c49715dfd122c1858f8b568c69ec45e4273c9ba6c58ce3f0b4e60f696493cf`。重建插件与旧交付按 ZIP 内文件逐一对照，仅 `background.js` 变化，缩小 68 字节；不能由此宣称 CPU 或内存全面下降。新 MSI 为 203,440,128 字节，SHA-256 `2c87bdc8af83100538350809befae63324540e0b430e23acd8b2ce7408432946`，较基线减少 8,192 字节，不能解释为安装体积显著下降。

首次最终包安装验收在并发启动阶段失败：四个 Host 均收到两个正确响应，且只启动一个 Core，但最慢首响应 **1645.09ms**，超过既有 1500ms 门槛。剖析的 `database_open` 阶段耗时 1510.865ms；目前只能定位到数据库打开及初始化这一段，不能据此认定某条 SQLite 语句、磁盘或防病毒是根因。失败及正常卸载结果保留在 [首次安装失败](results/installer-first-failure.json)，没有放宽门槛、修改 Core 或删除失败记录。

随后预先固定顺序执行旧包→新包各一次完整安装验收，两者均通过，见 [旧包](results/installer-compare-baseline.json)、[新包](results/installer-compare-candidate.json)。旧包安装后启动、常驻接入、正常关闭后重启最慢为 151.21/78.25/248.91ms，新包为 145.90/74.55/143.42ms；每阶段四个 Host、两个 ping，共 24 个帧响应，均保持一个 Core。对照复用了已有沙盒任务库；报告中的 `first-installed-launch` 是本次安装后的首次启动，并非全新账户或新任务库的首次初始化。两次对照不能证明先前 1645.09ms 延迟或历史首次启动超时已解决，也不构成性能提速结论。

新包这次验收还通过含空格的自选安装路径、五个组件及双插件 ZIP 哈希、Windows Known Folder 默认 Downloads、公开 MDN 视频的成品字节与 SHA-256、完成后缓存清理、测试成品删除和 MSI 卸载注销。没有再次启动桌面 UI 或把 IPC 确认描述成原生按钮点击。桌面 `HLSDownloader/最新-20261025` 已更新为上述新包及对应插件、完整插件源码和 Firefox 审核源码；[交付清单](results/desktop-delivery.json)明确 `release_ready=false`，并保留首次启动超标和其他未闭环限制。

Chromium 通用附件仍会先建立 DownloadItem 再取消清理，POST/Blob 仍由浏览器保存后导入；本次没有实现“所有下载都零浏览器参与”。指定直播 403、原始 HLS 卡死现场、历史首次启动超时、实体设备、云端和长期负载边界仍未全部闭环。没有宿主产品安装、收费云端调用、签名或 GitHub Release 资产替换。本轮没有创建分支或 worktree；历史被拒绝的构建缓存清理不换方法重试，不能声称工作目录只剩源码。

收尾时沙盒内测试产品进程为空，已关闭本轮沙盒及其客户端；[状态记录](results/handoff-state.json)列明仍保留的生成目录。宿主[只读检查](results/host-residue.json)未发现产品命名进程及三个常用卸载根下的 HLS Downloader 记录；没有删除用户文件或注册表，也不把这些有限检查描述为全盘零残留。桌面 8 条 SHA256SUMS 全部与当前文件相符。
