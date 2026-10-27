# 点击前接管与 GET/POST 隔离（2026-10-27）

产品源码提交：`d27804fb3d5ab17b710455fdbc684b31ed42fd80`；验收脚本修订：`aa87b63`。只修改插件后台，复用上一包未变的 Core、Native Host、桌面 UI、播放器和安装助手；没有新增生产依赖。

## 已复现的根因

1. Chromium 点击前接管直接调用 `offer`，绕过工具栏和媒体下载使用的待确认请求去重。连续点击同一链接会创建多个确认。现在复用 `requestDownload`，同一待确认下载共享一份交接；明确接收失败时保留浏览器回退，同时避免读取失败回执中不存在的 `handoff.id`。
2. 点击前尚未发出 GET，同地址的历史 POST 仍可能被请求链查询选中，连旧请求体一起交给主程序。现在点击前接管明确声明 GET，查询链时匹配请求方法；不把旧 POST 当成本次 GET。响应型下载仍保留其真实方法。

旧生产代码运行新增回归实际为 3 失败、12 通过；修复后类型检查与 418 项插件测试通过。Chromium、Firefox 和 Firefox 审核源码构建成功，Firefox lint 零错误、零警告、零提示。完整插件源码 ZIP 的 124 个文件集合及字节独立匹配 Git tree。

## 实际浏览器验收

Edge 156.0.4314.42 在 Windows Sandbox 中使用真实鼠标输入、Native Host 和 Core 验证普通直链、取消、重复点击、旧 POST 干扰以及禁用/排除/离线回退。直链相关场景均未创建 DownloadItem；重复点击只有一份持久确认；旧 POST 没有被重放。回退场景各只创建一个浏览器下载项、发出一次请求，成品字节匹配。

Firefox 158.0.1 的响应接管回归通过，包含附件、重定向、无扩展名、预览边界、取消、禁用、排除、Alt、离线及用户指定的公开 GitHub EXE。没有运行下载的 EXE。

确认决定通过 Core IPC 提交；这不是原生确认按钮的鼠标验收。首次新夹具在网页上下文调用了扩展 API，实际失败记录保留在 `results/first-attempt`；修正为扩展上下文后通过。打包首次输入缺少脚本要求的 `Firefox.zip`/`Chromium.zip` 名称，纠正命名后成功。

## 最终安装包、交付与发布

最终 MSI 覆盖正在运行的 r16，旧 MSI 登记移除；220 个安装文件哈希全部匹配。安装包内解出的实际插件重新通过双浏览器一次性链接接受/拒绝/源不可用、Firefox 响应接管与公开 GitHub EXE、Edge 直链/重复点击/旧 POST/离线回退。升级包随后卸载成功。

第一次全新安装虽然已验证安装路径、文件哈希、Known Folder 和真实下载，但沙盒连接丢失，界面 screen/paint 均为空，验收实际失败；原始 `results/fresh-installed.json`、`results/installer.json` 和 `results/summary.json` 保留失败。随后只重测该边界：新沙盒恢复显示后，全新含空格路径安装、全部文件哈希、Native Host 冷启动/并发/常驻/重启、默认 Windows Downloads、公开 MDN 文件字节、主界面非空绘制与卸载通过。见 `results/fresh-retry`；paint 仍为空，screen 非空。这不是完整原生按钮视觉验收。独立结果汇总在 `results/acceptance.json`，没有改写首次失败。

桌面复用 `C:\Users\lee\Desktop\HLSDownloader\最新-20261025`：安装包、Chromium/Firefox 插件、插件完整源码、Firefox 审核源码、来源记录、说明及校验和已更新并逐文件匹配。MSI 203,448,320 字节，与上一包相同；SHA256 为 `f4eb8d1a051f1d2f7e0b60e90030248efd4035f8856875a37e45f0cdb591e98a`。

[v7.0.3-r17](https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.3-r17) 已公开为未签名预发布。9 个资产的服务端大小和 SHA256 均匹配；r16 发布已移除，源码标签保留。公开发布共 5 个，v7 只有 r17；历史稳定 Latest 仍为 v6.0.1。草稿尚无正式标签时，按 tag 查询 API 返回 404；通过 releases 列表定位草稿并完成核对后才发布。见 `results/github-assets.json`、`results/releases.json`。

## 清理与本机状态

本轮没有宿主安装或运行产品，没有修改宿主注册表。只读核对的产品卸载登记、Native Host 精确子键和产品进程均为空，见 `results/host-state.json`。不能据此声称整台电脑零痕迹。任务创建的沙盒已关闭，没有新分支或 worktree。

**工作目录仍有生成缓存，桌面仍有旧 20261026 MSI。** 本轮 662,940,400 字节缓存的删除被自动审批拒绝，只返回 `blocked by policy`，没有具体原因；命令未执行，没有改用其他方式重试。此前被拒绝的 performance/takeover 缓存、node_modules、构建输出和空目录也保留。本轮目录库存见 `results/cleanup-inventory.json`；不能声称已只剩源码。

安全收尾脚本默认只读，已实际执行通过；代理没有执行 `-Apply`。它校验精确目录库存、受控源码、目录链接和最新桌面成品哈希。前两轮脚本仅更新新成品来源守卫和实际变化的生成目录库存，旧删除目标不变。保留源码、Git、最新桌面成品、私有备份及用户下载。用户可执行：

```powershell
& 'A:\Ubuntu\测试\hls-downloader\docs\validation\takeover-followup-20261027\finish-cleanup.ps1' -Apply
```

## 明确限制

Chromium 普通 MV3 插件仍不能在全部下载入口保证零 DownloadItem。未知附件、POST、Blob 与浏览器已完成的文件不能靠删除记录伪装成从未创建；本轮没有解决这个通用限制。Firefox 响应阻断也不等于所有浏览器入口全覆盖。

对照的 [AB Chromium 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)明确不阻断响应；[Chrome 官方文档](https://developer.chrome.com/docs/extensions/reference/api/webRequest)说明普通 MV3 扩展不能使用阻塞 webRequest。没有采用企业策略强装或系统安全设置变更。

此前直播站点 403、原始 HLS 卡死现场、偶发播放器状态超时、历史启动慢点等没有在本轮闭环。没有收费云端、实体设备、全部原生按钮或长期负载验收；`release_ready=false`，不得称作全功能完成或性能提升。
