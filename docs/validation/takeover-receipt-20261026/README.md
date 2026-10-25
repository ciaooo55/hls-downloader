# 浏览器接管回执与副本清理

产品源码提交：`1b5efac80c91a06c23202278f0a8ae5d0b233404`。仅修改插件接管路径，复用上一候选的 Core、Native Host、UI、播放器、Presenter 和 Updater，没有新增生产依赖。

## 根因和修复

桌面返回持久接收回执后，`offer()` 仍等待 `storage.session` 写入跟踪记录；下载创建事件随后又等待 `storage.local` 写入清理队列，才取消浏览器副本。存储响应延迟会阻止清理，即使桌面已经接收。

修复后，状态跟踪异步进行，并在写入前安排状态轮询。两处浏览器接管共用同一收尾逻辑，将清理责任持久化与浏览器副本取消并行；清理失败仍保留后续处理记录。没有把收到回执前的失败当作桌面接管，也没有改变 POST/Blob 本地导入的源文件保护。

普通文件的 `resourcePayload()` 原先也遍历同页面其他媒体请求，为最多 32 个目录上下文收集 Cookie。现在普通文件只携带自己的请求上下文；HLS/DASH 继续保留音轨、分片、密钥等独立请求上下文。未改变所选资源及来源页面的身份处理。未做延迟或 CPU 基准，不据此宣称具体性能提升。

## 验证

在旧清理顺序下新增慢存储回归，实际出现 1 failed / 17 passed：清理队列写入未完成时，浏览器记录没有被移除。并行收尾后定向 18 项通过，完整插件类型检查及 403 项测试通过。Chromium/Firefox 构建、ZIP 和 Firefox 源码包构建通过；Firefox lint 为零错误、零警告、零提示。见 results 中的前后回归及构建记录。

沙盒浏览器和最终安装验收结果保存在 results；确认决定通过真实 Core IPC 提交，不等同于原生按钮点击验收。Firefox 验收保留下载创建事件日志，不能靠事后 erase 冒充未创建下载项；Chromium 验收允许记录创建事件，但要求桌面决定前副本和文件已移除。

最终安装包在 Windows Sandbox 中从上一候选覆盖升级，升级时旧 Core 运行中。旧 MSI 登记移除，新登记正常；安装后的双插件 ZIP、来源记录与候选一致。Firefox 158.0.1、Edge 156.0.4314.42 分别通过 39 个验收检查点（包含同一场景的前后检查，不是 39 种下载）：明确附件、download 属性、重定向、inline 配合 download 属性、无扩展名、未知长度、图片、JSON、XHR 保持页面处理、用户取消、Core 可见失败、公开 GitHub EXE、禁用、排除站点、离线及 Alt 绕过。下载字节和哈希一致，沙盒测试任务被清理，最终卸载登记及 Engine 文件移除。

用户先前提供的 GitHub EXE 实际下载 102535433 字节，SHA-256 为 `3f98533c3ff1c0fcea635358a1c8b79dd5df94668ae03df723230901ec05bbd6`，与 GitHub Release 资产 digest 一致，未执行该 EXE。Firefox 在确认前后没有下载创建事件、下载项或文件，下载面板关闭。Edge 的重定向和无扩展名场景确实记录了创建事件，但确认前副本已取消并移除；这不证明 Chromium 所有附件完全零下载界面。

最终 MSI SHA-256 为 `140b9774ebcb457dd7875b384de055568321a171a493e7cbf2972f50d72fce96`，203448320 字节，与上一 MSI 大小相同。完整插件源码 ZIP 的 123 个受控文件与产品提交 Git blob 逐字节一致。桌面仍沿用 `HLSDownloader/最新-20261025` 目录，更新其中的 20261026 未签名 MSI、双插件、完整源码、Firefox 审核源码和交付记录。本轮宿主未安装产品；只读检查的精确 Native Host 键、产品卸载项及进程均为空，不声称全盘零痕迹。

夹具失败单独保留：第一轮复制后未找到预期安装包路径，改用固定英文文件名；第二轮覆盖升级和卸载成功，但沙盒内 PowerShell Archive 模块加载失败，未开始浏览器验收。解压改用 .NET 自带 ZipFile，未修改产品或系统安全设置。

## 对照及边界

核对 [AB 接管源码](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/DownloadLinkInterceptor.ts)及 [MV3 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)：Firefox 可在响应阶段取消；Chromium 通过下载创建事件取消、移除记录。AB 还明确排除非 HTTP 和已记录的非 GET 请求，不能据此承诺全部下载都支持。[IDM 官方说明](https://www.internetdownloadmanager.com/register/new_faq/enableBFE.html)确认其高级集成还涉及网络驱动，未检查其闭源内部。

本轮没有实现 Chromium 通用附件完全零下载项，也没有取消 POST/Blob 的浏览器中间暂存。指定直播 403、原始 HLS 卡死现场及历史启动慢点未在本轮闭环。没有云端、实体设备、原生 UI 绘制或长期负载验收，不宣称全功能无问题或全面等同 IDM。候选保持未签名、`release_ready=false`，不更新 GitHub Release。
