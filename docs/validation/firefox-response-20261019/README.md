# Firefox 在建立下载项之前接管（2026-10-19）

本轮修复 Firefox 普通 HTTP GET 下载先出现浏览器暂停项的问题。Firefox 增加 `webRequestBlocking`，在响应头阶段等待实际桌面 NativeHost 确认接收，再取消原响应；不以下载项暂停、隐藏或随后 erase 作为成功标准。[Mozilla API 依据](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/webRequest/onHeadersReceived)。

策略继续服从开关、站点排除、大小门槛及 Alt 绕过。桌面不可用时保留浏览器正常下载；桌面已经接管后，取消表示取消这次操作，下载失败显示在项目内，不另启浏览器副本。删除旧的 Firefox 请求发送阶段准备分支，复用两端的策略和请求上下文；跟踪缓存写入失败也不会推翻已经收到的桌面接收确认。

实际下载还暴露并修复了附件识别的遗漏：导航附件中的图片、JSON 和无扩展名二进制响应不能套用被动媒体嗅探的排除规则。同源 `download` 属性覆盖 inline 响应的情形也保留了明确点击门禁，避免抢走普通预览。

## 验证

- 插件类型检查、全量 50 文件 / 386 项测试通过；最后分类修补后，相关 4 文件 / 92 项定向复测通过。
- 最终 Chromium/Firefox 构建通过；Firefox `web-ext lint` 的错误、警告、提示均为 0。
- Windows Sandbox，Firefox 158.0，实际交付 Core/NativeHost 和未签名候选插件。最终背景脚本 SHA-256 见 [result.json](result.json)。
- 普通链接、download 属性、重定向、inline 加 download 属性、无扩展名、无 Content-Length、图片附件、JSON 附件和真实 GitHub EXE 均由 Core 完成并核对文件 SHA-256。
- 真实文件为用户提供的 `Codex-Account-Switcher-Portable-0.13.14.exe`，102,535,433 字节，使用 GitHub Release 资产公开 digest 校验；只作为数据下载，没有运行该 EXE。
- 每个接管案例在桌面决定之前和完成之后都核对：下载项列表为空、永久记录的 `downloads.onCreated` 事件为空、Firefox 下载面板 closed、浏览器下载目录为空。测试即使遇到随后 erase 的项也会失败。
- 桌面取消、桌面 HTTP 403 失败后同样没有浏览器下载项。页面自身 fetch 仍取得完整响应且没有桌面接管。
- 关闭接管、排除站点、实际中断本次浏览器 NativeHost 并让注册指向不存在的可执行文件：浏览器按预期完成下载且没有桌面 offer，内容哈希一致；恢复注册后 NativeHost 重连成功。仅修改沙盒临时登记。
- Alt 点击核对的是没有桌面 offer；Firefox 自身的 Alt 点击/另存为行为受偏好影响，本轮没有将它描述为浏览器完整下载验收。

测试通过 Core 协议确认/拒绝下载，未将其冒充桌面按钮 GUI 点击测试。宿主没有安装或启动产品，也没有修改宿主 NativeHost 注册。完整包、插件及审核源码的交付哈希另记于 `desktop-artifacts.json`。

## 尚未达到的完整替代目标

本轮证明了上述 Firefox HTTP GET 下载的提前接管。POST、Blob/Data 网页导出仍保留原浏览器下载后导入路径；Chromium MV3 仍通过 DownloadItem 暂停交接。这些路径没有达到“浏览器完全不创建下载项”的要求，不能宣称所有浏览器、所有下载类型已完全替代。没有为追求这个说法移除数据保全门禁或改动系统浏览器安全机制。

最初的测试夹具曾因指定默认根目录以外的保存位置、测试 Python 缺少证书库配置而中断；已修正夹具并重新执行。扩展测试也实际抓到无扩展名响应仍建立 DownloadItem，修复分类后才得到最终通过记录；没有放宽零下载项标准。

## 最终交付与安装验收

源码实现提交 `bf5927b`。桌面 `HLSDownloader/最新-bf5927b` 的四份成品已更新并重新核对哈希：[交付清单](desktop-artifacts.json)。Firefox ZIP 曾被正在使用的浏览器映射，常规覆盖被 Windows 拒绝，随后以同目录原子替换成功；没有结束宿主 Firefox，没有留下旧包或 partial 文件。已加载的旧扩展不会因此自动热更新，测试本轮修复需要在 Firefox 中换用新包。

安装包内两份插件 ZIP 与外部交付完全一致，审核源码内修改文件与仓库逐字节相同，Firefox ZIP 背景脚本也与通过 38 条沙盒验收的脚本一致。没有签名或发布新的 GitHub Release。

最终 MSI 的自选目录静默安装、已安装插件/Core 哈希、源码溯源、正常卸载及所查程序/NativeHost 登记移除已在全新沙盒通过，安装、卸载退出码均为 0；[结果](package-result.json)及 [MSI 生命周期合同](msi-lifecycle.json)。这是静默安装参数验证，中文路径选择页和一次确认覆盖界面的 GUI 证据沿用上一轮，未在这里重复宣称。

MSI 为 203,436,032 字节，比本轮基线增加 45,056 字节（44 KiB）；沙盒安装文件合计 469,462,423 字节。没有增加播放引擎、浏览器内核或产品运行依赖。本轮未重新测试字幕云端、播放器及其他未修改功能，也没有将旧 Release 的状态算作本轮已更新。

## 清理状态

本轮专用沙盒均已关闭，宿主产品未安装。最终源码仍仅有 main 和一个 worktree。

自动审批拒绝执行四个本轮生成目录的递归删除，返回 `blocked by policy`，没有提供具体原因；因此 `extension/node_modules`、`extension/.output`、`extension/.wxt` 和 `.tool-cache/firefox-response` 仍待清理。没有换工具绕过拒绝。可人工执行 [finish-cleanup.ps1](finish-cleanup.ps1)，脚本先核对四个桌面成品哈希，只处理列出的目录和空缓存父目录，不操作源码、Git、用户下载或其他项目。脚本尚未执行，仅完成 PowerShell 静态语法检查。
