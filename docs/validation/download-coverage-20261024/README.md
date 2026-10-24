# 下载入口覆盖与第三方对照（2026-10-24）

本轮检查不限于用户给出的 GitHub EXE 地址。目标是普通下载由桌面确认和管理，取消不恢复浏览器副本；不隐藏下载栏，不增加交接网页、系统代理或网络驱动。仅在 Windows Sandbox 中运行产品，宿主未安装。

## 官方实现依据

- AB 官方插件当前 `master` 为 `658a0c70188d78033f478ba51c102a273322166f`（GitHub API 核对）。[Firefox 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest2DownloadLinkInterceptor.ts)返回 `cancel:true`；[Chromium 实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)不阻断响应。
- AB 的[公共拦截器](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/DownloadLinkInterceptor.ts)在 Chromium 下载项创建后取消；自动下载路径仅支持 HTTP GET，并过滤 Blob/data。不能用 AB 的日常观感证明所有协议均没有创建浏览器下载项。
- [IDM 官方故障文档](https://www.internetdownloadmanager.com/register/new_faq/enableBFE.html)明确说明高级集成依赖网络驱动及 Windows Base Filtering Engine。其专有内部实现未公开，本轮没有逆向或声称复刻其驱动。
- [Chrome webRequest 文档](https://developer.chrome.com/docs/extensions/reference/api/webRequest)限制普通 MV3 的 `webRequestBlocking`；但这不意味着 MV3 所有接口都不能提前拦截。[DNR 文档](https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest)支持 Chrome 128+ 按响应头匹配规则。

## 实验与取舍

在隔离的 Edge 156 中实测 DNR 响应头规则：无扩展名 `attachment` 可以重定向到扩展页面，原下载零创建事件、零下载项，见 [API 实验结果](results/dnr-probe.json)。原始查询参数保留。第一次实验的 WebDriver 上下文超时也保留，未算通过。

此方案会改变当前导航；桌面离线时恢复原下载需要再请求一次，无法保留服务器已经发出的一次性响应。用户继续要求成熟软件的正常操作体验，故未将这个实验或额外权限并入产品。没有用接口旧限制作无限制的“不可能”结论，也没有以实验成功冒充全量产品验收。

## 根因与修改

1. 视频和音频虽然被服务器明确标为 `attachment`，仍被媒体预览的点击要求挡住。脚本、地址栏、表单和新标签不一定有可信点击记录。现以附件响应本身作为下载证据，普通媒体预览仍受原有条件约束。旧 Firefox 的真实沙盒报告在 `/media-attachment` 记录到下载创建事件，证明缺陷，不只是静态推测。
2. `downloads.onCreated` 参数是历史快照。读取策略期间小文件可能已完成，再调用 `pause` 会失败；原代码直接返回，遗留浏览器文件而不交接。现在暂停失败后查询实际状态，只有确认完成的文件继续交接，其余失败仍不启动第二份传输。旧 Edge 扩展验收中媒体/音频附件漏接；最终构建对同类入口重新验收。
3. 自动交接按资源 URL 生成固定 `client_request_id`。同一 URL 再下载时，Core 正确执行幂等协议却返回了之前已接受或拒绝的交接，浏览器副本已清理，但没有新的确认窗。重定向回到刚下载的同一 URL 的沙盒场景复现了这一问题。现每次下载分配独立请求，NativeBridge 的通信重试仍复用原负载；删除了已无调用需求的“自动/主动”双轨标识生成逻辑。

生产改动集中于 `fileTakeover.ts` 和 `background.ts`；未增加产品依赖、配置或系统组件。

## 验收范围

[沙盒脚本](../../../scripts/validation/smoke_extension_download_ownership.py)使用真实浏览器、NativeHost 和 Core。原生确认决定通过 Core IPC 发出，不声称完成原生按钮 UI 验收。每次 Edge 执行使用独立配置目录，避免失败执行留下的下载项污染后续报告；失败时保存后台控制台供定位。

每种浏览器的 16 个场景：直接 ZIP 接受、取消、无扩展名 ZIP、用户 GitHub EXE 点击后取消；无点击记录的视频、音频、图片、JSON、HTML、未知类型附件、重定向、GET 表单、新标签；禁用、站点排除、明确离线。ZIP 与未知文件类型实际下载并校验 1 MiB 完整字节，测试文件随后仅在沙盒删除。

中间执行均保留原结论：`firefox-baseline` 复现旧媒体附件的下载创建事件；`edge-candidate` 和隔离配置后的 `edge-diagnosis-02` 复现快速文件漏交接；`edge-diagnosis` 因复用前次失败的浏览器配置而被旧下载项污染，随后修正测试配置隔离；`edge-final` 在快速文件修正后暴露同 URL 重复交接缺陷。它们不计为通过，也不被描述为多轮全量对抗测试。

Firefox 接管场景要求零下载创建事件；Edge 明确链接要求零事件，服务器附件允许创建后取消，但在桌面确认时不得保留浏览器副本。禁用、排除和明确离线要求浏览器只发起一次原下载并得到完整文件。这两个标准不能混用。

POST、Blob/data 仍是浏览器保存后本地导入；本轮没有把它改成重复提交 POST 或丢弃网页内存文件。浏览器“保存网页”、其他扩展生成的文件、受限页面、所有网站和长期运行未全面验证，不能宣称所有下载完全零浏览器参与。Chromium 通用附件的创建后取消仍未满足用户要求的绝对零浏览器反应。

最终结果、构建日志、包身份和交付清单位于本目录。GitHub Release 资产本轮未更新。

实际最终构建：类型检查通过，6 文件 76 项单元回归通过，Chromium/Firefox 构建通过，Firefox lint 为零错误/警告/提示；[Firefox](results/firefox-ownership.json)和 [Edge](results/edge-ownership.json)各 16 项通过，[安装包](results/installer.json)9 项通过。[执行来源索引](results/summary.json)绑定最终浏览器后台脚本和验收脚本。Firefox 接管场景零创建事件；Edge 的通用附件场景共记录 10 个真实创建事件，随后取消清理，不是零创建。

桌面仍为 `HLSDownloader/最新-20261024` 下 9 个文件，见[交付清单](delivery.json)。MSI 为 203,444,224 B，SHA256 为 `7aa26f74fbc1e4db6f0cf1fba1a52eaeb26c78a479e56f31f67f95c56206ec8d`；安装文件共 469,440,889 B、222 个，较前一包增加 64 B。Core/UI/Host/媒体组件未重新编译，插件包与源码包按实际测试字节交付。包内构建记录保留真实基线 `main@42a89cd` 加 [source.patch](source.patch)，不伪写为最终提交的干净构建。

清理未完成：自动审批拒绝删除本轮 `.tool-cache/response-intercept-20261024`，仅返回 `blocked by policy`，整条被拒绝的命令未执行，未改用其他工具绕过。此前被拒绝的两轮缓存、`extension/node_modules`、`.output`、`.wxt` 等也仍保留。已将本轮目录加入[限定手工清理脚本](../browser-takeover-20261024/finish-cleanup.ps1)，并更新现有两个脚本的最终 MSI 哈希保护；未执行该清理脚本，不能称源码目录已无缓存。
