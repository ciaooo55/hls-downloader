# 当前源码的真实浏览器交接与离线修复

基线 `fa233537f11c5255297850c7dc5248f06cb1552a`，包含尚未提交的前轮修改。本轮仅在 Windows Sandbox 运行产品，通过 CDP 调用真实 Edge、实际插件、Native Host 和 Core；没有使用桌面鼠标键盘。Core/Host 为当前源码离线编译的 dev 二进制，只用于正确性验证，不能据此比较发布包性能。确认窗从既有 r22 MSI 提取，源码本轮未改；组件哈希见 `offline-fix/bootstrap.json`。

## 确认并修复的根因

`offline-observed/edge.json` 记录：Native Host 在首次交接前不可用，浏览器项仍被暂停；40 秒后为 `in_progress/paused=true/bytesReceived=0`。这不是界面隐藏问题。

原响应接管分支先暂停浏览器，再发送 `offer`。Host 不可用时，无法区分“根本没发出交接”和“Core 已提交但回执丢失”，于是进入未知所有权恢复并持续保留暂停。Firefox 的响应分支还会取消原响应；两浏览器的新增回归修前均失败，见 `offline-regression-red.json`。

修复将连接检查放在新交接的发送与暂停边界之前。直接链接的新请求也走同一检查，删除原来单独的重复前置判断；已有交接查询/原 ID 恢复不凭离线探测回退。探测为只读，按交互优先级越过后台长轮询，普通心跳仍为最低优先级。POST/Blob 继续使用原浏览器字节导入，不重发源请求。

## 实际执行结果

| 范围 | 本轮证据 | 结论 |
|---|---|---|
| 相关所有权及 NativeBridge 回归 | `offline-regression-green.json` | 63 项通过，含已有未知交接不回退及交互探测抢占后台读取；不与旧测试次数相加 |
| 未发送时不暂停原响应 | `offline-no-pause-regression.json` | Chromium/Firefox 两项通过；是前述回归的子集 |
| TypeScript；Chrome/Firefox 构建 | `validation.json` | 实际通过；未生成发布安装包 |
| 当前插件、真实 Host/Core 接收 | `offline-fix/edge.json` | 一交接、一任务、成品 SHA256 正确 |
| 持久接收后丢失回执 | 同上及 `offline-fix/proxy/offers.jsonl` | 注入一次错误，同请求 ID/同负载重试，两次回执指向同一持久交接 |
| 用户拒绝 | `offline-fix/edge.json` | Core 不新增任务，源请求不重发，浏览器不继续下载副本；由 Core 协议触发拒绝，未验证物理按钮 |
| 一次性 POST | 同上 | 源请求一次，导入实际浏览器文件，成品哈希正确 |
| Blob | 同上 | 无新增网络请求，导入字节及成品哈希正确 |
| 首次 Native Host 不可用 | `offline-fix/initial-offline/edge.json` | 全新浏览器配置；原响应请求一次、浏览器完成 1 MiB 文件、哈希正确，Core 没有新增交接或任务 |

Edge 实际版本 `156.0.4314.50`。所有 canary 成品 SHA256 为 `fbbab289f7f94b25736c58be46a994c441fd02552cc6022352e3d86d2fab7c83`。当前源码、构建和运行组件关联见 `validation.json`，原始结果及失败记录由 `MANIFEST.json` 校验。

## 未通过与证据边界

**第 2 阶段仍未通过，不打包或发布。** 当前 Edge 的无扩展名附件仍先创建 DownloadItem，再移交并清理；原始创建事件已保留，不能记为严格零项。POST/Blob 也使用浏览器下载结果。Firefox 的本轮离线保护有回归和构建证据，尚未执行当前修复后的真实 Firefox 联调。完整入口矩阵、完整浏览器重启、Firefox 一次性响应在未知交接下的保全以及真实网站仍未完成。

本轮重新读取 [AB 固定源码的 MV3 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)和[公共接管链路](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/DownloadLinkInterceptor.ts)：其 MV3 不阻断响应，公共链路另在下载创建后取消；不据此假定 AB 满足所有入口严格零项。本轮未实测 AB/IDM。[Chrome 官方限制](https://developer.chrome.com/docs/extensions/reference/api/webRequest)及[响应头规则的重定向语义](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest#once_a_response_is_received)也不能证明一次性请求可无损交还，不添加企业策略、驱动或代理。

另观察到已完成任务快照中仍有 `connection_hint=排队`、连接范围 `done=0/state=queued`，原始快照在上述 Edge 报告中；属于后续状态阶段待定位事项，本轮未宣称已修复或已通过 UI 验收。

## 夹具失败与收尾

- `bootstrap-fixture-failure.json`：Windows PowerShell 5.1 对无 BOM 脚本中的中文 MSI 文件名解码错误。改为按精确日期查找唯一 MSI，并核对固定哈希。
- `matrix-with-offline-timeout.json`：在线链路通过，扩展重载后的自动化调用超时；保留为未验收，不将其当成离线通过。
- `offline/`：补充观测后发现该次尚未出现产品 service worker，Host 也没有启动，属于启动前置条件不足；随后显式核对 worker 注册。
- `initial-offline/`、`offline-observed/`：补齐实际 DownloadItem 状态，最终确认上述暂停缺陷；未通过记录保留。
- `offline-fix/`：修复后的在线矩阵和全新配置离线场景均通过；不再重跑已通过且未受影响的检查。

所有本轮沙盒已关闭，见 `sandbox-state.json`。宿主没有安装或运行产品，没有写产品注册表、签名、收费云端、更新桌面包或发布 GitHub。现有一个 `main`、一个 worktree；前轮与本轮修改保留，未擅自提交。旧缓存的删除曾被自动审批拒绝，本轮没有重试或更换删除方式；复用及新增生成物的精确库存见 `cleanup.json`，因此工作区尚未只剩源码。
