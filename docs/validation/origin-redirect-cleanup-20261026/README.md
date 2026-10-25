# 下载来源、HTTP 跳转和播放器清理

产品源码提交：`1dea557e54b1eb0684065a53368b74c761a1d42d`。本轮只修复三项静态审计发现，未增加生产依赖。

## 修复的行为

1. 浏览器下载先绑定 DownloadItem 的标签页/框架身份；缺失身份时，只有来源唯一的请求链或页面证据才能匹配。同 URL 分属不同标签页或框架时停止猜测。点击记录也检查来源歧义，避免请求链被拒绝后又从点击队列借用另一页身份。保留新标签页下载的实际请求来源，并将已知媒体框架传入查询。
2. 下载完成时，在播放器锁内切换成品、保存成品路径及完成状态，随后释放播放器和缩略图保护，再清理分片。此后新播放只能挂载成品；保存状态失败则保留缓存。清理入口的测试屏障阻住完成线程，同时要求另一线程能进入播放器，验证锁已释放。
3. raw HTTP、WinHTTP 和 curl 使用一致的跳转方法规则：POST 经 301/302/303 转 GET 并丢弃正文；307/308 保留方法与正文。后续跳转重新应用请求上下文时，仍移除已丢弃正文的相关头，避免 POST Content-Type 回到 GET。

## 已执行验证

Windows Sandbox 中，跳转回归遍历三种真实传输与五种状态码；每种组合还跟随一次 308，检查第二、第三跳的方法、正文、正文头和最终文件字节。跨域 Cookie 隔离、播放器锁交接、成品记录及缓存保护回归也通过。命令筛选存在重叠，不能把重复执行的测试累计成独立测试数。见 [沙盒汇总](results/summary.json)及对应 stdout/stderr。

插件在本机和 Windows Sandbox 均完成类型检查及 402 项测试；回归覆盖同 URL 的不同标签页、不同框架、来源缺失、身份明确及歧义时不消耗点击记录。Chromium/Firefox 构建和 ZIP 打包成功，Firefox 扩展校验为零错误、零警告、零提示。Core/Native Host release 构建成功。见 [插件测试](results/extension-tests.stdout.txt)、[扩展校验](results/firefox-lint.json)及构建日志。

## 交付与范围

未签名候选包复用上一已验收 MSI 中的 Desktop UI、JRE、播放器、Presenter 和 Updater，仅替换 Engine、Native Host、双插件及来源记录。安装包大小为 203448320 字节，较上一包增加 4096 字节；这不构成内存、CPU 或响应时间的性能结论。最终 MSI SHA-256 为 `2522a1ae46c203d8c581f5dad985ad7806b480e09e2a521473889c577cf8a574`。

完整插件源码 ZIP 的 123 个文件集合和每个文件字节均与产品提交的 Git blob 一致。桌面沿用 `C:/Users/lee/Desktop/HLSDownloader/最新-20261025` 目录，MSI 文件名更新为 `HLSDownloader-7.0.3-20261026-未签名.msi`，同时更新双插件、两份插件源码包和交付记录；旧 MSI 在核对新包后移除。见 [桌面交付清单](results/desktop-delivery.json)。

最终 MSI 在新沙盒中从上一包覆盖升级，升级时旧 Core 正在运行。旧产品登记移除，新登记正常；安装后的 Engine、Host、Presenter、Updater、桌面 JAR、来源记录和双插件 ZIP 均逐项核对 SHA-256。升级前保存并读回限速，完成字节已知的任务；升级后任务、文件全部字节和限速保留。使用安装后的 Core 发起 POST 导出，服务器实际收到 POST、随后 GET，最终成品字节一致；测试任务/文件删除后卸载，产品登记及 Engine 移除。见 [最终安装验收](results/installer-qa.json)。该安装后用例的 POST 正文为空；非空正文及正文头清除由前述三传输回归验证，没有混称为同一场景。

夹具失败没有计为通过：前两次 MSI 路径使用正斜杠，Installer 日志将 `C:/QAWork/baseline.msi` 解析成 `/QAWork/baseline.msi`，返回 1619；改为本地文件及 Windows 反斜杠路径后安装成功。随后一次夹具指定下载子目录却未先配置下载根，正确触发目录边界拒绝；修正为先设置下载根后完成正式验收。失败报告保存在 results 中，未为夹具放宽产品保护。

本轮产品只在沙盒运行；没有在宿主安装或写产品注册表。关闭沙盒前核对 PID 与创建时间，见 [沙盒关闭](results/sandbox-closed.json)。本轮构建生成物的最终清理结果见 [清理核对](results/cleanup.json)。

本轮没有做新的真实浏览器多标签页交接、原生 UI 绘制、云端、实体设备或长期负载验收。Chromium 通用附件零下载项、POST/Blob 无浏览器暂存、指定直播 403、原始 HLS 卡死现场及历史启动慢点仍未闭环。候选保持 `release_ready=false`，不更新 GitHub Release。
