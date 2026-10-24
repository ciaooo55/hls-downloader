HLS Downloader 7.0.3-r13 是未签名预发布交付，产品版本仍为 7.0.3。包含此前的 HTTP 续传完整性修复、本轮安全自动硬件解码，以及收紧后的 Chromium 下载接管验收。

- 修复截短缓存被误当完整、缺少可靠资源标识时丢弃有效断点、失效断点无法安全回退单流下载的问题。前一轮五轮故障注入及源码证据见仓库验收记录。
- 播放器使用 `hwdec=auto-safe`；沙盒确认实际使用 D3D11VA，不支持时回退。没有新增播放引擎或依赖。
- 本轮沙盒实际通过：GPU/无 GPU 播放和缓存清理各 27 项，播放器回归 3 项、HTTP 引擎回归 59 项、HTTP 边界 13 项；Edge 接管 36 项、7-Zip 官网下载 6 项；新 MSI 安装/默认 Downloads/公开下载/卸载 9 项。
- 接管覆盖接受、取消、失败、通信中断恢复和多种下载形式；成功接管后匹配浏览器下载项为空，成品 SHA 与源数据一致。浏览器自动化以 headless Edge 运行，操作实际桌面确认窗口；不称为全浏览器人工验收。
- MSI 194.04 MiB，安装目录447.76 MiB；UI、Host、Updater、媒体库和插件复用此前已验收组件，新 MSI 逐个核对哈希。本轮只在 Windows Sandbox 安装运行。

**已知限制：**禁用 GPU 时软件渲染仍高 CPU，未解决；不能据短视频测试保证其他硬件性能。Chromium MV3 会先创建下载项再暂停交接，不能承诺浏览器下载 UI 完全无反应。Firefox 本轮未重跑；收费字幕、实体投屏/TVBox 与所有网站不在本轮范围。没有与 IDM/AB 做同机基准。

安装包、Firefox/Chromium 插件均未签名；Firefox 正式签名由用户处理。随附插件完整源码及 Firefox 审核源码。源码、验证与二进制身份见 [本轮记录](https://github.com/ciaooo55/hls-downloader/tree/v7.0.3-r13/docs/validation/hardware-takeover-20261024)。完整项目源码可使用此标签的 GitHub 自动源码包。

包内 provenance 保留打包时的候选状态，`release_ready=false`；外部交付清单说明后续实际验证，不改写已验收 MSI。手动安装更新，自动更新签名校验保留。
