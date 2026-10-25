# 浏览器下载文件名等待修复

## 对照与根因

核对 AB 浏览器集成当前固定提交 `658a0c70188d78033f478ba51c102a273322166f`：[MV3 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)不阻断响应，[共用拦截器](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/DownloadLinkInterceptor.ts)通过下载创建事件取消、移除浏览器项。不能据此认定 AB 的任意 Chromium 下载从不创建浏览器项。[Chrome 官方文档](https://developer.chrome.com/docs/extensions/reference/api/webRequest)也限定普通 MV3 扩展的 blocking 权限。没有检查 IDM 闭源内部，不推断它的内部实现。

本项目 `refreshedDownload()` 无条件等待文件名确定事件；不支持该事件的浏览器也会等待 450ms。接管路径在响应头已经提供附件名时，仍可能进入这段等待。修复只在浏览器实际支持事件时等待；已有附件名直接查询当前下载状态，无名下载仍读取浏览器元数据。未提前取消浏览器源文件，未改变 POST/Blob 导入、排除站点、离线回退和 Cookie 策略，没有新增依赖、权限或服务。

在修改逻辑前扩展既有回归：模拟不支持文件名事件的浏览器，当前元数据已经可读，旧实现仍未返回，1 failed / 16 passed。修复后定向 17 项通过；完整插件类型检查及 400 项测试、Chromium/Firefox 构建和 ZIP、Firefox 扩展校验均通过，lint 为 0 errors / 0 warnings / 0 notices。第一次完整类型检查发现新增测试使用的 mockResolvedValue 不适配重载类型，改为同文件既有的 mockImplementation 写法后通过；失败原文保留。

## 实际浏览器验收

产品只在 Windows Sandbox 中安装和运行。真实 Edge、Firefox 各 18 项通过，使用生产构建插件、真实 Native Host/Core：明确链接、无扩展名附件、媒体/音频/图片/JSON/HTML 附件、未知附件类型、重定向、表单、新标签页、POST/Blob、禁用接管、排除站点及桌面离线。验收核查实际下载创建事件、取消后不产生第二次请求、成品字节，以及本地导入完成后源文件移除；确认决定经 Core IPC，不等同于实际点击原生确认按钮。

创建事件增加记录浏览器初始 filename 与时间戳，保留创建与事后清理的区别。Edge 的若干附件创建事件 filename 确实为空，但本批未做旧/新包同环境延迟对照，不宣称真实网页普遍快了 450ms。Firefox 仍在响应阶段阻断相应附件；Chromium 通用附件仍会创建后清理下载项。短路测试证明移除了不存在事件的等待，不证明全浏览器零下载界面。

Edge/Firefox 的报告后台 bundle 哈希分别与本地最终构建一致，见 results。Core/UI/播放器未修改或重建，沿用前批 `e406c0485567f3ee1dc2ee25853176e6cf44aed31bc6b310e87eaafa54396c75` Engine。最终包验收和桌面交付在完成后追加。

## 边界

指定直播 403、原始 HLS 卡死现场、历史首次初始化慢点与超时仍未闭环。本批不是全功能或 IDM/AB 全面对等认证，没有收费云端、实体设备、完整 UI 或长期负载验收。不在宿主安装、不签名、不更新 GitHub Release。历史缓存删除被自动审批拒绝，本批不绕过重试；临时脚本与生成物仍集中在现有 `.tool-cache`，不能声称只剩源码。
