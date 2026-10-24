# 分离音轨提前播放验收（2026-10-24）

源码基线：`main@7dd99d25928b553f6f8d9701f237d2eff63f9f3d`。生产改动由 [source.patch](source.patch) 和 [构建快照](BUILD-PROVENANCE.json) 中三个源文件的 SHA256 固定。快照保留打包时的真实工作区状态，不回写为“已验收”。

## 根因和修改

HLS 和 DASH 点播此前先下载并合并完整独立音轨，再下载视频。首播等待随整条音轨长度增长，而不是只等待可播放前缀。

- 音视频分片按累计播放时间交错领取，共用原下载器。HLS 仍使用原任务并发上限，DASH 仍沿用逐片下载，没有增加线程池、依赖或新的配置项。
- 提前生成两轨完整的本地预览时间轴；实际文件仍受就绪标记保护。两轨首个连续分片完整落盘后才开放预览，后续分片可等待及请求下载优先级，不发送半个分片。
- 合并移到两轨下载结束后；两轨保留各自的身份及大小检查，HLS 暂停时保存两轨已落盘分片的检查点。直播路径不采用点播调度。
- 删除 HLS 原来为串行音轨进度而设的递归下载和进度偏移，进度直接统计同一队列里的两轨。

首轮 DASH 新回归失败，发现 Representation 级 SegmentList 未读 Initialization/sourceURL 和 duration/timescale，并把显式 SegmentURL.media 当模板展开。已修复该输入范围；不代表覆盖所有 DASH 继承、Timeline 或字节范围组合。[首轮失败](results/first-attempt/dash-tests.stdout.log)保留。

## 实际执行

离线编译 Core 测试程序及 release Engine，产品和测试均在 Windows Sandbox 运行，未在宿主安装。UI、Host、Updater、媒体库和插件沿用 r14 相同字节，未重新构建插件。

- [HLS 定向回归](results/hls-tests.stdout.log)：26 通过、0 失败，包含提前预览、暂停后两轨首片不重复请求。
- [DASH 定向回归](results/dash-tests.stdout.log)：17 通过、0 失败，包含显式 SegmentList 与提前预览。
- [真实媒体流程](results/download-playback-cleanup.json)：29 项通过。TS、fMP4、AES HLS、外挂音轨 HLS、DASH、HTTP 六种输入，覆盖暂停续传、实际解码、完整时长、跳至未下载的 80% 位置与回跳、缩略图、原窗口/位置/倍速切换成品、成品完整解码、完成及删除清缓存、保留临时文件设置、保护无关文件。
- 外挂 HLS 与 DASH 的服务端扣住后续音频分片，确认尾片不存在时已开放预览。样本是合成短媒体，不代表实际长片和所有站点已覆盖。
- 安装验收第一次因测试 Python 的 ZIP 模块路径无法读取配套证书策略文件中断，[失败记录](results/installer-environment-failure/installer.json)保留；将原 SSL 模块与原证书策略文件放到普通目录后重跑，未关闭 TLS 校验。

安装验收 9 项通过，见 [installer.json](results/installer.json)，与 [候选身份](candidate-manifest.json) 及 [组件哈希](expected-components.json) 绑定。安装目录 469,445,527 B、222 文件；含空格的自选路径、组件一致性、Windows Downloads、MDN 公开文件字节与缓存清理、UI 可见连接、卸载产品登记均已核对。MSI SHA256：`9b8f1afb154c6a72d9f1cdff2dc4f6739ed911bed34153ff12aacb19e0300203`；203,444,972 B，比旧包小 23,828 B。Engine SHA256：`db1c84d5d96133405351196bd0a96360446880a42125ffb04cbd9697ce8a51f4`。

## 验收状态与接管边界

发布来源生成器新增构建快照说明；后续验收仍由安装包哈希绑定的外部报告记录，未修改旧发布文件的字节或历史结果。

Chromium 当前是普通 MV3 扩展，通用接管发生在下载项创建后；该问题本轮未解决。Chrome 官方说明普通 MV3 扩展不能使用 `webRequestBlocking`，仅企业策略安装例外：[官方 API](https://developer.chrome.com/docs/extensions/reference/api/webRequest)。Firefox POST/Blob 导出也不能通过盲目重发请求解决，避免重复业务操作或丢失网页生成的数据。本轮没有改浏览器策略、隐藏下载记录来宣称提前阻断，亦未扩大浏览器权限。

桌面交付目录为 `HLSDownloader/最新-20261024`，以同名未签名 MSI 的上述哈希区分。四份插件及源码 ZIP 未变。GitHub r14 旧资产未替换；`release_ready:false` 表示正式发布门禁，不能因定向验收通过而改写。

未重测收费云端字幕、实体投屏/TVBox、全站兼容与长期性能，不能承诺所有功能无问题或性能等同 IDM。

自动审批拒绝删除本轮构建缓存，仅返回 `blocked by policy`，未给具体原因；代理未改用其他工具或删除父目录绕过。缓存仍在 `.tool-cache/interleaved-playback-20261024`。桌面交付完成、沙盒关闭后，可手工运行 [限定路径清理脚本](finish-cleanup.ps1)。用户备份和历史其他残留不在该脚本范围。
