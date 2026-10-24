# 浏览器下载归属修复（2026-10-24）

本轮针对用户报告的 Firefox 暂停记录、取消后浏览器继续下载，以及 Chromium 的重复下载行为。Core、UI、Host 和媒体库沿用已验收安装包，修改集中在扩展；未在宿主安装产品。

## 参考和根因

核对 AB 官方插件源码 `658a0c70188d78033f478ba51c102a273322166f`：

- [Firefox 响应阻断](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest2DownloadLinkInterceptor.ts)返回 `cancel:true`。
- [Chromium MV3](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)不能阻断原响应，由下载项事件取消接管。不能据此声称所有 Chromium 下载都没有创建事件。

原实现把桌面任务进度当成浏览器副本的清理条件：桌面已经持久接收请求，浏览器仍可能保持暂停项；桌面取消或任务失败还会恢复浏览器下载。隐藏下载栏也不能改变实际归属。

## 修复

- Firefox 在已确定为 HTTP(S) GET 下载的响应阶段等待桌面接收回执，再阻断浏览器原响应；用户取消表示取消本次下载。
- Chromium 对明确压缩包、安装包和同源 `download` 链接，在普通可信左键点击时阻止默认导航并交给桌面。保留修饰键、普通 PDF/媒体预览、源码查看及大小过滤行为。
- 其他 Chromium 附件仍从 `downloads.onCreated` 接管，桌面持久接收后立即取消并清理浏览器副本，不再等待用户确认、首字节或完成。持久化清理责任，恢复后继续清理。
- 明确未交接的离线、禁用和排除站点保留原浏览器功能；提前点击路径在 offer 发出后失联不会盲目导航造成第二份下载。
- 移除隐藏下载栏的代码和 `downloads.ui`、`downloads.shelf` 权限。未增加产品依赖。
- POST、Blob/data 仍保留原有本地导入，不通过重复 POST 或重新请求网页内存 URL 冒充提前接管。

## 验证方法

[浏览器验收脚本](../../../scripts/validation/smoke_extension_download_ownership.py)仅在 Windows Sandbox 运行，使用真实浏览器点击、NativeHost 与 Core；Firefox 用 WebDriver 元素点击，Edge 用 CDP 从当前 DOM 定位后发送鼠标输入。确认决定经 Core IPC 发出，不声称完成原生确认按钮的 UI 点击验收。分别记录零下载创建事件与创建后取消，不混用标准。

首轮 Edge 在加载测试页前报 `tab crashed`；浏览器配置当时位于沙盒共享目录。改为沙盒本地配置后，EdgeDriver 元素查找仍无法找到截图和 DOM 中实际存在的链接，后续使用 CDP DOM 与鼠标输入。Firefox 首次脚本读取清单使用系统默认 GBK 编码，改为 UTF-8 后继续。失败记录保留，未计作产品验收通过。

## 实际结果与交付身份

- 插件类型检查通过；相关单元回归 6 文件、66 项通过；Chromium/Firefox 构建通过；Firefox lint 为 0 错误、0 警告、0 提示。
- [Firefox 158.0.1](results/firefox-ownership.json)与 [Edge 156.0.4314.42](results/edge-ownership.json)各 7 项通过。直接 ZIP 接受、取消、无扩展名附件、用户原 GitHub EXE 地址点击后取消、禁用、排除站点、明确离线回退。
- 接受场景实际下载 1 MiB 文件并校验字节。Firefox 接管场景零下载创建事件；Edge 明确链接与 GitHub EXE 零事件，`opaque` 附件记录了 1 次创建事件并在桌面确认前清除。未下载完整 GitHub EXE、未执行该文件。
- Edge 第 7 次执行的离线回退未通过。当时夹具仅删除 HKCU 默认值、没有断连断言，不能排除回落到其他 Host 注册；第 8 次改为不可执行的测试 Host 清单、等待旧 Host 退出，并确认 ping 返回 `ok:false` 后重跑通过。未将第 7 次改写为通过。
- [9 项安装验收](results/installer.json)通过：包含空格的自选路径、包内组件与插件哈希、Windows Downloads、公开 MDN 文件真实下载、UI 可见连接、占用测量和卸载产品登记。安装文件共 469,440,825 B、222 个。
- [结果索引](results/summary.json)明确列出独立执行来源：Firefox 和安装为 attempt-05，Edge 为 attempt-08。相同扩展背景脚本哈希与 MSI 组件哈希连接这些报告，没有把不同执行伪装为一次全过。

MSI 为 203,444,224 B，SHA256：`227cabe3c177ff64136c5a1ba10c99b5161eae9a82d8e1194f82d0db4e2aa937`。Core SHA256 沿用 `db1c84d5d96133405351196bd0a96360446880a42125ffb04cbd9697ce8a51f4`，其他桌面组件未改。包内 [构建快照](BUILD-PROVENANCE.json)保留构建时真实状态，源码基线为 `main@37d0550` 加 [source.patch](source.patch)；[候选身份](candidate-manifest.json)和[组件清单](expected-components.json)固定验收字节。

桌面同一交付目录 `HLSDownloader/最新-20261024` 已更新 MSI、Chromium 插件、Firefox 未签名插件、Firefox 审核源码及插件完整源码，仍为 9 个文件，见[交付清单](delivery.json)。审核源码和完整源码中的生产文件哈希均与构建快照一致。GitHub Release 旧资产没有替换。

现有 `smoke_extension_takeover.py`、`smoke_extension_public_takeover.py` 的断言已按新的接管归属更新，但本轮未执行这两个完整脚本，不计入通过项。原生确认按钮操作、所有网站和下载入口也未全面重测。

本轮不代表全部网站、POST/Blob、所有浏览器下载入口及长期性能已覆盖；未调用收费云端或测试实体投屏设备。Firefox ZIP 未签名，签名由用户处理。

自动审批拒绝了前轮构建目录，以及本轮 `.tool-cache/browser-takeover-20261024`、`extension/node_modules`、`extension/.output`、`extension/.wxt` 和误建的 `extension/.tool-cache/browser-takeover-20261024` 的删除，仅返回 `blocked by policy`。这些目录仍保留，不能称工作目录已只剩源码。代理未通过改用其他工具或删除父目录绕过。

保留[限定范围的手工清理脚本](finish-cleanup.ps1)，绑定本轮最终 MSI 哈希；仅列明本轮与前轮缓存、插件生成目录，保护源码、Git、桌面成品、用户下载及备份。此脚本未由代理执行。
