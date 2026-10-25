# 浏览器下载文件名等待修复

## 对照与根因

核对 AB 浏览器集成当前固定提交 `658a0c70188d78033f478ba51c102a273322166f`：[MV3 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)不阻断响应，[共用拦截器](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/DownloadLinkInterceptor.ts)通过下载创建事件取消、移除浏览器项。不能据此认定 AB 的任意 Chromium 下载从不创建浏览器项。[Chrome 官方文档](https://developer.chrome.com/docs/extensions/reference/api/webRequest)也限定普通 MV3 扩展的 blocking 权限。没有检查 IDM 闭源内部，不推断它的内部实现。

本项目 `refreshedDownload()` 无条件等待文件名确定事件；不支持该事件的浏览器也会等待 450ms。接管路径在响应头已经提供附件名时，仍可能进入这段等待。修复只在浏览器实际支持事件时等待；已有附件名直接查询当前下载状态，无名下载仍读取浏览器元数据。未提前取消浏览器源文件，未改变 POST/Blob 导入、排除站点、离线回退和 Cookie 策略，没有新增依赖、权限或服务。

在修改逻辑前扩展既有回归：模拟不支持文件名事件的浏览器，当前元数据已经可读，旧实现仍未返回，1 failed / 16 passed。修复后定向 17 项通过；完整插件类型检查及 400 项测试、Chromium/Firefox 构建和 ZIP、Firefox 扩展校验均通过，lint 为 0 errors / 0 warnings / 0 notices。第一次完整类型检查发现新增测试使用的 mockResolvedValue 不适配重载类型，改为同文件既有的 mockImplementation 写法后通过；失败原文保留。

## 实际浏览器验收

产品只在 Windows Sandbox 中安装和运行。真实 Edge、Firefox 各 18 项通过，使用生产构建插件、真实 Native Host/Core：明确链接、无扩展名附件、媒体/音频/图片/JSON/HTML 附件、未知附件类型、重定向、表单、新标签页、POST/Blob、禁用接管、排除站点及桌面离线。验收核查实际下载创建事件、取消后不产生第二次请求、成品字节，以及本地导入完成后源文件移除；确认决定经 Core IPC，不等同于实际点击原生确认按钮。

创建事件增加记录浏览器初始 filename 与时间戳，保留创建与事后清理的区别。Edge 的若干附件创建事件 filename 确实为空，但本批未做旧/新包同环境延迟对照，不宣称真实网页普遍快了 450ms。Firefox 仍在响应阶段阻断相应附件；Chromium 通用附件仍会创建后清理下载项。短路测试证明移除了不存在事件的等待，不证明全浏览器零下载界面。

Edge/Firefox 的报告后台 bundle 哈希分别与本地最终构建一致，见 results。该轮浏览器验收时 Core/UI/播放器未修改或重建，沿用前批 `e406c0485567f3ee1dc2ee25853176e6cf44aed31bc6b310e87eaafa54396c75` Engine。

## 最终候选包未通过启动门槛

候选 MSI SHA-256 为 `559db7c1389a625163858aa865847775153816d264dae775df2c77ac30fc95d6`。沙盒内包哈希、自选含空格安装路径、五组件哈希及双插件 ZIP 哈希四项通过；正常默认管道首次启动的四个 Host 首响应为 2381.11–2384.74ms，超过既有 1500ms 门槛。启动前默认数据库不存在；阶段诊断显示新任务库初始化约 2247.599ms，主要在 PRAGMA 和事务提交。没有继续后续下载、闲置与正常卸载验收；finally 清理卸载返回 0，旧/新产品登记均已移除。原始报告见 [filename-installer.json](results/filename-installer.json) 和 [filename-upgrade.json](results/filename-upgrade.json)。

因此未执行桌面发布脚本，桌面 MSI 仍为 `3378bebfa5dc7e61b033c7ae8f8e52aa20d42f754244161d9e77d14181fb0a3f`，双插件及源码包也保持原套件。不能把插件单项通过说成新安装包已交付。

## 独立新任务库对照发现另一个根因

固定同一 Engine/Host，以 AppData→QA→QA→AppData 顺序，每轮新任务库、四 Host 并发启动。最大首响应依次为 186.75、168.85、5102.39、161.64ms；四轮均只启动一个 Core。失败轮 Core 在约 99ms 已监听，三个 Host 在约 161–165ms 返回，另一个 Host 等了约 5.1 秒；该轮 SQLite 初始化仅约 11.5ms，不能归因于数据库慢。见 [对照汇总](results/filename-startup-paths.json) 和 [慢样本逐 Host 诊断](results/filename-startup-qa-2.json)。

代码根因位于 `CoreIpcClient::connect_existing()`：管道请求失败后有两个独立的时限判断，后一个判断可以在期限边界直接跳出循环，绕过 `tcp_loopback_enabled()` 检查，进入原本未启用的 TCP 五秒重试。修复为所有管道超时统一携带原错误退出，再检查 TCP 策略；不修改 SQLite 同步策略、不提前报告 IPC 就绪、不放宽门槛。此修复与安装现场新库写入慢属于不同问题。

修复后离线编译库测试、release Engine 和 Native Host 成功；产品仅在同一沙盒运行，既有 IPC 定向回归 16 项通过。新 Engine SHA-256 为 `dd4dd9d2e647974ed0ccfaddd0549106b9696d338ccabcf0e347f7425cde2222`，Native Host 为 `21bde12cce452818ed1749b66bc72bb01e1c057cea53da14cdb1b80cc1040a4e`。没有增加依赖、配置项或线程。

沿用相同夹具和 1500ms 门槛，再做 8 轮独立新任务库、每轮四 Host 并发各发两个 ping，共 32 个 Host、64 个响应帧。8 轮最大首响应分别为 155.48、193.79、165.71、152.58、168.33、176.93、155.44、155.89ms，均通过且只启动一个 Core。见 [逐轮汇总与诊断](results/pipe-boundary-startup.json)、[IPC 回归](results/pipe-boundary-tests.log.txt)和同前缀构建记录。这验证了本轮样本，不能证明历史安装现场的新库初始化慢点或 15 秒超时已经全部解决。未重跑完整 Core、UI、浏览器与最终 MSI 验收，也未将新 Engine/Host 包装交付。

宿主只读检查显示：当前 32/64 位 HKCU/HKLM 产品卸载登记、精确 Native Messaging 产品键及产品进程均为空，见 [检查记录](results/host-readonly-check.json)。它不等同于全盘零残余证明，工作目录的既有构建缓存仍在；没有操作其他项目或用户下载。

## 边界

指定直播 403、原始 HLS 卡死现场、历史首次初始化慢点与超时仍未闭环。本批不是全功能或 IDM/AB 全面对等认证，没有收费云端、实体设备、完整 UI 或长期负载验收。不在宿主安装、不签名、不更新 GitHub Release。历史缓存删除被自动审批拒绝，本批不绕过重试；临时脚本与生成物仍集中在现有 `.tool-cache`，不能声称只剩源码。
