# 下载交接与取消修复（2026-10-27）

产品源码：`da06ce46e7f306feaba0d53383f832f3a0179c3c`。本轮只修改插件的下载交接，复用源码未变的 Core、Native Host、桌面 UI、播放器和升级助手，没有新增生产依赖。

## 确认的根因与修复

1. 浏览器文件在处理下载事件期间完成，旧逻辑仍发送 URL 让主程序重下，并在收到交接回执后删除原文件。一次性链接可能已经失效，唯一成功的文件也会被删除。现在暂停操作结束后重新读取下载状态；已完成的普通文件和直连媒体复用本地导入，复制完成前保留浏览器源文件。复制失败或用户拒绝时保留原文件。HLS/DASH 仍下载完整媒体，不拿单个清单替代视频。
2. 暂停期间用户取消或移除了下载，旧收尾逻辑仍可能创建桌面任务或恢复浏览器传输。现在停止交接，并且不恢复不可续传的 `USER_CANCELED` 项；主程序接收失败时，可续传的临时中断仍会恢复浏览器下载。

Blob、POST 与已完成 GET 共用既有的本地导入队列，持久请求 ID 和最终复制状态继续负责恢复与源文件保护。没有新建下载引擎或并行接管架构。

## 实际验证

- 在旧源码上运行新增事件监听器回归，实际出现 3 失败、4 通过，确认完成文件被发送成 URL。修复后完整插件类型检查和 415 项测试通过；新增用例还覆盖暂停完成竞态、取消、可续传回退、POST 和禁用/排除策略。见 [旧逻辑结果](results/before.txt)、[最终结果](results/extension-tests.txt)。
- Chromium/Firefox ZIP 与 Firefox 审核源码构建通过，Firefox lint 为零错误、零警告、零提示。完整插件源码 ZIP 的 124 个文件集合及字节与产品 Git 树一致。见 [源码检查](results/extension-source.json)。
- Firefox 158.0.1、Edge 156.0.4314.42 在 Windows Sandbox 中，通过真实浏览器下载 API、Native Host 和 Core 验证一次性 GET 的接受、拒绝、源文件不可用。设置读取由可控门禁保持，确认真实 DownloadItem 完成后才放行；每个链接仅请求一次。接受后成品字节一致并清理源文件；拒绝时保留源；源不可用时在创建任务前明确报错。源文件故障用重命名保留了测试原件，没有丢弃唯一副本。见 [Firefox](results/firefox-completed.json)、[Edge](results/edge-completed.json)。
- Firefox 普通响应接管的 39 个检查点通过，涵盖附件、重定向、无扩展名、未知长度、inline 加 download 属性、图片/JSON、XHR 不接管、取消、Core 失败、禁用、排除、Alt、离线及公开 GitHub EXE。指定 EXE 实际下载 102,535,433 字节，SHA256 为 `3f98533c3ff1c0fcea635358a1c8b79dd5df94668ae03df723230901ec05bbd6`，与 GitHub 发布资产 digest 一致，没有执行 EXE。见 [响应接管](results/firefox-response.json)。
- 最终 MSI 在同一沙盒从 r15 覆盖升级，升级时旧 Core 正在运行；旧 MSI 登记移除，222 个安装文件哈希匹配。随后使用安装包内实际解出的双插件，重复上述一次性 GET 验收，并重复 Firefox 39 个检查点。见 [升级](results/installer.json)、[安装后 Firefox](results/installed-firefox-completed.json)、[安装后 Edge](results/installed-edge-completed.json)、[安装后响应接管](results/installed-firefox-response.json)。升级报告的文件数误记为逐项 Count 数组，实际哈希检查逐文件完成；已修正后续 runner 的计数表达式，原报告未改写。
- 最终 MSI 又通过全新含空格路径安装、全部安装哈希、Windows Downloads Known Folder、公开 MDN 文件字节、Native Host 冷启动/并发/常驻/重启、UI 显示并连接和卸载。见 [全新安装](results/fresh-installed.json)。UI 实屏非空，paint 截图为空；这不是原生按钮和视觉细节的完整验收。确认决定通过 Core IPC 提交，没有当作原生确认按钮点击结果。

首次沙盒夹具没有稳定触发完成竞态；第二轮错误地要求源不可用时创建失败任务，实际 Core 已在创建前返回明确错误；第三轮固定延迟仍未让 Edge 完成。均保留在 first/second/third-attempt，最终改为确认浏览器完成后释放门禁，没有把这些夹具失败计为产品通过。打包时首次输入文件名不符合脚本的 `Firefox.zip`/`Chromium.zip` 合同，纠正夹具命名后重打包，记录也保留。

## 成品与 GitHub

最终未签名 MSI：203,448,320 字节，与上一包相同；SHA256：`71dc67c1ba08e466f91f750a325f66519edd531b0e2314a53efa2a63ed828564`。没有把上一轮性能退化试验装入产品，也没有据这次功能修复宣称性能提升。

桌面成品位于 `HLSDownloader/最新-20261025`，复用原目录：新的 20261027 安装包、Chromium/Firefox 插件、插件完整源码、Firefox 审核源码、来源记录、说明和校验和均已更新并核对。旧 20261026 MSI 的删除被自动审批拒绝，所以桌面目前仍多这一份旧包。

[v7.0.3-r16](https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.3-r16) 已公开为未签名预发布，9 个资产的服务端大小与 SHA256 全部核对。移除 r15 发布并保留标签后，GitHub 公开发布仍为 5 个，v7 只有 r16；v6.0.1 仍为历史稳定 Latest。见 [资产核对](results/github-assets.json)、[发布列表](results/releases.json)。

## 仍未完成的范围

Chromium 全部场景零 DownloadItem 仍有缺口；已完成 GET、POST/Blob 仍需保护浏览器暂存文件。Firefox 的响应阻断不等于覆盖所有浏览器入口。没有宣称“所有下载完全等同 IDM”。[Chrome 官方 API](https://developer.chrome.com/docs/extensions/reference/api/webRequest)说明普通 MV3 扩展不能使用阻塞 webRequest；此前核对的 [AB MV3 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)也不在响应阶段阻断，不能把取消后 erase 当作未创建过下载项。

指定直播站点 403、原始 HLS 卡死现场、偶发播放器状态超时和历史启动慢点未在本轮闭环。本轮没有重新构建未变的 Core/UI，也没有云端、实体设备、全部原生按钮、所有浏览器入口或长期负载验收。候选保持 `release_ready=false`。

## 清理现状

沙盒已关闭。本机没有安装或运行产品；只读核对的 Chrome/Edge/Firefox 精确 Native Host 子键、产品卸载登记及产品进程均为空，见 [本机状态](results/host-state.json)。不将这些定向检查说成全盘零痕迹。

删除六个空目录、旧桌面 MSI、以及本轮四个生成目录的动作分别被自动审批拒绝，仅返回 `blocked by policy`，没有具体原因；没有更换执行方式重试删除。上轮 2.62 GiB 缓存也继续保留。本轮生成目录的逻辑文件大小约 0.85 GiB，不等于实际磁盘分配量；私有备份和用户下载保留。当前不能说工作目录只剩源码。

[收尾脚本](finish-cleanup.ps1)默认只读，已实际执行通过；只有用户加 `-Apply` 才清理清单中的生成目录、空目录、上一轮缓存及已核对哈希的旧桌面 MSI。它先核对目录库存、Git 受控文件、目录链接和桌面成品哈希；pnpm 指回源码的目录链接会先被非递归删除链接本身。源码、Git、最新成品、私有备份和用户下载不在删除范围。上一轮脚本只更新了新成品的来源守卫，没有更改清理目标或代理执行删除。

```powershell
& 'A:\Ubuntu\测试\hls-downloader\docs\validation\takeover-20261027\finish-cleanup.ps1' -Apply
```

清理库存见 [本轮目录](results/cleanup-inventory.json)、[其他残留](results/cleanup-status.json)、[默认只读检查](results/cleanup-preview.txt)。本轮没有创建分支或 worktree。
