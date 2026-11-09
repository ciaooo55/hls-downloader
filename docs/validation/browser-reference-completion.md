# 浏览器插件参考与功能补齐（2026-11-04）

本次基于工作区 `A:\Ubuntu\测试\hls-downloader` 的 `main`，HEAD 为 `412f1f984b645375f7a8e0acb8585113e2a6cc9e`。开始时已有 15 个跟踪文件改动及 3 个未跟踪的站点清单文件；这些内容保留在当前工作树，本说明不将其全部归为本次新增实现。

参考输入为 `A:\Ubuntu\逆向\新建文件夹\浏览器插件破开` 中的 IDM 扩展可读源码及机制说明。参考扩展为 Firefox MV2 6.43.1；本次检查源码并隔离执行了实际裁决函数，未运行 IDM 桌面宿主或原生浮窗。

后续按用户授权扩展到整个参考目录，已静态追踪原始主程序、消息宿主及 `IDMNetMon.dll`，核对 COM 成功返回如何形成插件裁决 3，以及主程序的异步/同步派发分支。证据、样本哈希和未解决范围见 [IDM 原生接管链核查](idm-native-handoff.md)；仍未运行 IDM 本体。

用户明确的方向是借鉴接管、识别、下载发起和请求关联机制，保留 Cookie、Authorization、Referer、Origin、User-Agent、代理及各 CDN 请求上下文，同时保留项目已有的下载管理、字幕、播放器和投屏等更多功能。低占用、速度和稳定性是改进目标；没有基准实测就不宣称速度提升或低于 IDM 的资源占用。

## 参考机制与当前实现

| 机制 | 项目实现与本次处理 |
| --- | --- |
| 先交接、后取消；未接收时保留浏览器下载 | IDM 样本发送后等待宿主裁决，值为 3 才取消。项目以可呈现的持久接收回执转移责任；普通文件保全原响应，丢回执通过原请求 ID 查询，已接收后的用户取消结束本次下载 |
| 请求身份、响应正文和播放器关联 | 当前请求链、MAIN-world fetch/XHR、MSE 归属和内容脚本已有实现；新增跨页面响应与缓存重放隔离 |
| 可见区域与悬浮入口 | 复用裁剪和 Shadow DOM/slot 可见性观察，按绘制顺序选择鼠标下的播放器；Firefox 本地真实媒体场景通过，Edge 本次运行因标签页崩溃未完成 |
| 宿主执行媒体下载 | 保留现有 Native Messaging 与 Core IPC；不引入 IDM 专用 MSG# 协议 |
| 桌面媒体识别 | 修复 `probe_command` 忽略 TaskSpec；复用任务默认值、代理选择、凭据还原和请求上下文再调用识别入口 |
| 精确请求身份 | 对已有 origin/目录上下文增加有界的精确 URL 身份；同目录音视频/密钥、同路径不同查询参数不再互相覆盖 |
| 扩展与 Core 的 DASH 能力一致 | 补齐 Core 的逐层 BaseURL、XML 实体解码和 SegmentList 继承，避免面板识别正常而实际请求错误地址 |

## 本次新增修复

- `extension/entrypoints/hooks.content.ts`：记录 fetch/XHR 的来源页面；响应返回、克隆清单读完和清单解析完成后均保留原页面身份；Response.clone/text 沿用该身份。只重放当前页面的资源和 MSE 事件。
- `extension/entrypoints/content.ts`：初始化缓存和正常事件处理都核对来源页面；SPA 切换清除旧 MSE 证据。URL 片段变化不清除同页资源。
- `native_shell/src/download_worker.rs`：链接识别使用实际 TaskSpec 的请求头、代理和还原后的浏览器请求上下文。
- `extension/lib/requestChain.ts`、`extension/entrypoints/background.ts`、`native_shell/src/credentials.rs`：传递与选择精确 URL 的请求身份（保留签名查询参数、忽略片段），再按目录和 origin 选默认上下文。候选收集上限保持为 32，所选资源的身份另行保留；Cookie 按 domain/path/secure/有效期匹配，跨域时清除上一请求的鉴权头。
- `native_shell/src/download_worker.rs`：凭据引用不存在、解密或上下文解析失败时明确报错；合并新敏感头时保留已有 Cookie、请求体和媒体请求上下文。
- `native_shell/src/media/dash.rs`：按 MPD、Period、AdaptationSet、Representation 解析直接子节点 BaseURL，解码签名 URL 的 XML 实体，并继承分片列表。同类型 SegmentTemplate 逐属性继承，时间线取最近一份；父级不同寻址类型不覆盖子级模板。
- 对应回归位于 `extension/lib/siteManifestHooks.test.ts`、`extension/lib/requestChain.test.ts`、`extension/lib/downloadOwnership.test.ts`、`native_shell/src/download_worker/tests.rs`、`native_shell/src/credentials.rs` 和 `native_shell/src/media/dash.rs`。

## 后续对齐与交接裁决核查

- 请求正文关联：fetch/XHR 附带可见的响应身份字段，后台以完整 URL（保留查询参数）和响应字段选择请求链；多候选鉴权不一致时清除旧身份并拒绝猜测。下载、投屏和 TVBox 三个扩展入口使用同一资源身份选择；这不代表原生直链投屏已支持凭据代理。
- 二进制清单：对网页已经读取的 ArrayBuffer、Blob、bytes 和二进制 XHR 进行有界的 HLS/DASH 检查，不改变网页返回值，不为识别额外消费原响应。
- 播放器匹配：精确播放 URL 的优先级高于 MSE 通配路径；HLS master 可关联其视频 variant，音轨和字幕不充当视频源。鼠标命中结合绘制顺序、透明控制层和 Shadow DOM；遮挡的下层播放器不再抢占悬浮入口，滚动和尺寸变化统一在动画帧内更新。
- Firefox 响应阶段、下载项阶段和手动交接共用持久交接日志；浏览器请求 ID 区分同资源的不同下载。写日志失败不发交接，丢回执不立即取消原网络响应；后续按原 `client_request_id` 查询和恢复，不产生第二次 offer。
- 已收到可呈现的持久回执后，用户在桌面取消或拒绝不恢复浏览器副本。恢复查询得到 `rejected`、`canceled`、`expired` 仍保留桌面决定；交接失败或无法呈现则保留浏览器处理。该行为有既有跟进回归和新增回执恢复回归保护。

IDM 证据路径均相对于参考目录的 `src/idmmzcc3-6.43.1/deobfuscated/background.js`：

| 位置 | 实际代码行为 | 能证明的范围 |
| --- | --- | --- |
| 431–445、1452–1479 | `S` 写入宿主通道，`Y` 打包请求与响应证据 | 发送成功与宿主最终裁决不同 |
| 1480–1489 | `sc` 发送成功后返回 Promise，将 resolve/reject 挂在请求记录；发送失败标记放行 | 等待宿主裁决，不能仅凭写通道成功取消 |
| 1058–1062、1490–1504 | type 13 的回包按原发送记录取请求 ID；`e == 3` 才调用 `X(..., true)` 并 resolve `{cancel:true}`，其他裁决 resolve `{}` | 明确取消/放行网络请求；无法从扩展推定原生按钮与数值的映射 |
| 401–413 | 断开后清空连接与待确认消息索引，注销监听并尝试重连 | 该函数未按原业务 ID 查询持久结果；不能据此宣称 IDM 具有本项目相同的丢回执恢复机制 |

隔离执行实际 `sc`、`Fa`、`X` 函数（通道和浏览器接口为桩）验证：发送后尚未收到裁决时 Promise 不完成；裁决值 0、1、2、4 放行，3 取消；发送失败不返回阻断结果。此检查验证 JS 分支，未运行 IDM 本体，也未模拟浏览器真实断线时的内部处理。

IDM [官方主窗口帮助](https://www.internetdownloadmanager.com/support/using_idm/using_idm.html)将确认窗的 Cancel 解释为取消下载，将进度窗的 Cancel 解释为停止下载并关闭窗口；[官方接管例外说明](https://www.internetdownloadmanager.com/register/new_faq/how-to-prevent-IDM-from-taking-download.html)让用户通过按键、停用当前标签页接管或站点例外主动交回浏览器。结合源码，这支持项目保留“桌面已接收后取消结束本次下载”的行为；仍未验证原生取消按钮对应哪个协议裁决。

## 第一轮实际验证

- 扩展定向测试：`siteManifestHooks`、`siteManifest`、`downloadOwnership`、`requestChain`、`mediaOverlay`，5 个文件、105 项通过。
- 随后新增精确身份回归，并重跑 `siteManifestHooks`、`downloadOwnership`、`requestChain`，3 个文件、85 项通过（包括新增 2 项）。另外两个未变测试文件的 22 项此前通过。
- 扩展 TypeScript：使用已有 `.wxt` 配置执行 `tsc --noEmit`，通过。
- Rust 离线编译成功，首次重新编译本地依赖与 Core 用时约 6 分 10 秒；没有下载新依赖。
- Rust 新增定向回归：`core_completion_` 的 4 项及精确 URL 凭据身份的 1 项全部通过，覆盖实际识别请求、凭据异常、上下文合并和 DASH 继承。
- 复用编译产物检查原有凭据与 DASH 模块，排除上述已通过的新测试后，另外 38 项全部通过；覆盖 Cookie 作用域、DPAPI、跨域重定向、音视频分离、模板展开和预览。此次 Rust 共验证 43 个不同测试。
- `git diff --check` 通过；Git 的 CRLF/LF 提示不是校验失败。

Rust 验证命令（在项目工作区执行，`TEMP`/`TMP` 指向工作区的 `.tool-cache/test-tmp`）：

```powershell
cargo test --offline --manifest-path native_shell/Cargo.toml --lib core_completion_ -- --nocapture
cargo test --offline --manifest-path native_shell/Cargo.toml --lib replay_preserves_exact_media_identity_before_directory_defaults -- --nocapture
cargo test --offline --manifest-path native_shell/Cargo.toml --lib -- credentials::tests media::dash::tests --skip core_completion_ --skip replay_preserves_exact_media_identity_before_directory_defaults --nocapture
```

## 后续实际验证

- 扩展完整 Vitest：54 个文件、520 项全部通过，包含请求身份、Firefox 丢回执、写日志失败、已接收后取消、播放器匹配和遮挡回归。
- TypeScript `tsc --noEmit` 通过；Chrome MV3、Firefox MV3 两种生产构建通过，产物位于 `extension/.output/`。
- Firefox 真实浏览器：使用项目已有 `smoke_extension_media.py`、独立临时 profile 和本地媒体服务，7 个场景 `direct`、`dual-source`、`shadow`、`dynamic-shadow`、`hls-mse`、`dash-mse`、`multi-mse` 全部通过；覆盖实际播放、悬浮按钮归属、播放器移除重挂、扩展替换刷新，以及投屏/TVBox 菜单入口。报告保存在 `.tool-cache/browser-reference-20261104/firefox-media.json`，不含真实站点凭据。
- Edge 使用相同隔离脚本与场景时，在导航阶段返回 `WebDriverException: tab crashed`，没有完成验收，也没有成功报告。未将崩溃归因于扩展或浏览器，未重复运行相同失败命令。
- 本轮未新增 Rust 修改，复用第一轮的 43 项 Rust 定向验证；未重新编译 Core。

未执行桌面/Presenter 完整构建、Windows 候选打包、安装升级、云字幕或实体设备验证。以上检查不替代全产品验收。本次没有安装新依赖、提交、推送或发布。任务启动的编译器、浏览器驱动和本地媒体服务已退出，测试临时目录为空，保留项目常规构建缓存与上述报告。

## 按原生证据继续优化（2026-11-04）

本轮实际修改浏览器侧公共请求链，没有替换项目 Core、调用 IDM 的 DLL 或引入 `MSG#` 协议。参考的是已核对的 `Y/sc → 原生接收 → 裁决回传` 机制：请求身份随交接提交，接收与下载结果分开。字幕、TVBox、投屏和现有播放器功能继续使用项目已有入口。

- **子清单按请求选择身份**：后台 HLS/DASH 探测逐 URL 查找同 tab/frame 的精确请求链，保留各自 Authorization、自定义头和实际 Referer/Origin。已捕获的空头是有效身份，不再用顶层页面头覆盖；未知 CDN 不继承主清单令牌，Cookie 按目标 URL 和授权范围重新匹配。
- **限制额外网络消耗**：POST、204/205、重定向响应和扩展自身请求不触发后台清单探测。探测不自动跟随未观察的重定向，避免自定义头被转发；HLS 子清单解析失败仍保留主清单已解析的信息。HTTP 明确拒绝后释放响应，不再马上换头重复请求；浏览器抛出网络/头限制异常时仍最多回退一次，并共用 5 秒截止时间。
- **凭据快照一致**：一次交接的基础 Cookie 和各请求上下文复用同一 origin 快照，保留 Path 规则，避免轮换期间两次读取产生不同身份，也减少一次重复的 Cookie 采集。
- **减少接管通信**：已经在暂停或 Firefox 响应阻断前检查过桌面连接的路径不再二次 ping；不同下载仅合并尚未完成的检查，不缓存检查结果。离线结束后下一次点击会重新检查；检查成功仍不替代持久接收回执。

本轮验证：扩展完整 Vitest **54 文件、528 项通过**；TypeScript `tsc --noEmit`、Chrome MV3 与 Firefox MV3 生产构建、`git diff --check` 均通过，构建产物更新在 `extension/.output/`。新增/调整回归包括跨 CDN 与同目录子清单的独立令牌、frame 隔离、Cookie 轮换快照、无正文/POST/扩展请求不探测、HTTP 拒绝只请求一次、并发检查与离线恢复，以及本地真实 HTTP 重定向不转发令牌。该本地 HTTP 检查使用测试进程内临时服务器，结束时关闭；测试与构建进程均已退出，没有新增后台服务。

本轮没有修改 Rust，沿用此前 43 项 Rust 定向结果，没有重跑 Core 编译；没有新增真实浏览器/桌面联调或 IDM 本体运行验证，也没有速度、内存与 IDM 的对比基准。以上结果不能表述为获得 IDM 完整源码、完成 IDM 原版二次开发或所有功能/性能已等同。

## 带凭据投屏与 TVBox 链路补齐（2026-11-04）

- Native Host 接收投送时，沿用浏览器下载的 DPAPI 凭据库保存 Cookie、Authorization、自定义请求头、Referer、Origin、User-Agent 和各 URL 的请求上下文。公共事件、桌面协议和播放地址只携带不透明引用。非 GET 媒体请求明确拒绝投送，不把一次性 POST 改为 GET。
- 桌面选择设备或发布局域网地址时，把原投送请求 ID 交给 Core。Core 必须找到仍在等待的原请求并核对媒体 URL，再使用原凭据、站点规则和代理设置；取消后的请求、错误 URL 和缺失凭据均拒绝执行。
- 远程投送使用既有媒体服务的流代理，复用 HTTP 引擎的凭据作用域、重定向和代理处理。正文按 64 KiB 块转发，保留 HEAD、Range、Content-Range 和内容类型；不调用会先缓存整部媒体的 curl 文件回退。浏览器的过期条件请求头不进入设备请求。
- HLS 主清单、子清单、独立音轨、密钥、初始化片段和媒体片段改写为同一随机挂载下的地址；同一资源保留稳定地址，维持隐式 BYTERANGE 的含义。DASH 按 Representation 展开继承的 BaseURL 和寻址信息，保留时间线、SegmentList/SegmentBase 字节范围，以及 Number、Time、SubNumber 模板和数字补零。
- 设备只能访问已登记的清单引用，不能通过播放地址指定任意上游 URL。路由表最多 8192 项、上游 URL 总长度最多 4 MiB，使用 LRU 淘汰；单清单最多读取 8 MiB、改写 4096 个引用，XML 最多解析 10 万节点。停止、替换挂载、关闭服务或接收端断开时释放上游；不会把源 Cookie/Authorization 写入局域网地址。
- 投送终态和临时凭据删除在同一 SQLite 事务提交。运行中的媒体代理持有内存快照，投送成功后删除库内临时凭据不影响后续分片请求；事务失败则保留凭据和 pending 状态以便恢复。
- 浏览器识别出的 MIME/清单类型贯通投送合同；无后缀 HLS/DASH 地址使用对应播放后缀，DLNA 元数据与媒体类型一致，保留音频类型。

完整 Rust Core 回归在 MIME 补充前执行：**685 项通过、0 失败、12 项按既有条件忽略**。其中新增代理回归覆盖独立音视频凭据、跨域密钥、签名查询、稳定的字节范围地址、重定向后的清单基址、Range/HEAD、DASH 继承/时间线/模板、流式首块、停止和断线释放；另覆盖原生凭据保管及终态事务回滚。MIME 补充后，Core 与测试目标 `cargo check` 通过，投送、代理和 DLNA 元数据的 **26 项定向回归全部通过**，包括新增的无后缀 MPD→分片实际 HTTP 请求与原投送身份绑定。桌面 Kotlin 编译及 ProtocolTest、MediaPushUiStateTest 共 **31 项通过**。没有在小幅 MIME 补充后重复运行未受影响的整个 Core 套件。

Release Core 与 Native Host 构建通过，本地产物分别为 `.tool-cache/build-cache/cargo-target/release/hls-downloader-engine.exe`、`HLSDownloaderNativeHost.exe`。使用新的二进制在 A: 工作区隔离目录执行两帧 Native Messaging ping，冷启动首响应 **1064.93 ms**，通过原有 **1500 ms** 门槛；仅启动并保留 1 个 Core。Core 进入 main 到 IPC 就绪约 90.73 ms，主程序创建调用约 689.15 ms。这是此次单次隔离测量，未把与历史环境的耗时差异归因于本轮代码改动。报告与产物 SHA-256 见 `artifacts/optimization-20261104/native-host-startup.json`；隔离 Core 已停止，测试数据目录已移除。

本轮扩展源码未改动，扩展验证沿用上一轮的 54 文件、528 项、TypeScript 检查及 Chrome/Firefox 构建结果；没有将其计为本轮重跑。

构建结束后，使用系统安装版 Edge 156.0.4314.58 和现有 Chrome MV3 产物，完整本地媒体联调 **23 个场景全部通过**，用时约 56.8 秒：直链与多来源、srcdoc、Shadow DOM/动态 Shadow DOM、同源/跨源 iframe、广告播放器、SPA、MSE/切片/加密结果归属、Worker/独立 Worker/多 Worker、多播放器、HLS/LL-HLS、DASH/低清晰度/多播放器/未知表示及空视频源。报告见 `artifacts/optimization-20261104/edge-media-smoke.json`。先前构建期间的两个无插件基线分别未建立 DevTools 连接、发生 renderer 导航超时，失败证据仍保留在同目录；这不代表已确定失败根因，最终联调使用的安装版在构建结束后通过。浏览器隔离配置、测试服务和临时媒体均由既有脚本退出时清理。

## 项目整理与媒体观察收敛（2026-11-05）

继续检查 `A:\Ubuntu\逆向`，包括新增的 `idm643/RECONSTRUCTION.md`、恢复文件目录及原始插件。`idm643/work/IDMan.orig.exe` 的 SHA-256 为 `3b5dc9ce53eb31933effa3fd973bec068a78e0f8c1d15a919b081f0c4d9325ba`，与前述未修改样本一致。新增报告补充了 `MSG#` 编解码层，但仍明确保留下载调度、媒体解析、原生浮窗和部分协议消费端的未知；没有将该报告或重建代码当成完整 IDM 源码，也没有把专用协议移植到本项目。

本轮实际采用的机制来自原始插件 `src/idmmzcc3-6.43.1/deobfuscated/content.js`：`rb`（约 296 行）单独观察媒体样式及父容器，`V`（约 273 行）释放观察，`nb/S`（约 641–661 行）用 ResizeObserver、IntersectionObserver 更新几何。项目据此做了以下调整：

- `extension/entrypoints/content.ts` 将 `class/style/hidden` 观察从整页子树收敛到播放器及其渲染祖先；普通 DOM 发现仍监听节点和媒体 `src`。无关聊天/动画节点的属性变化不再生成该整页观察器的记录。
- 裁剪祖先加入尺寸观察，补齐 CSSOM 修改导致容器缩窄、视频本身尺寸不变且仍部分可见时的悬浮入口更新。媒体移除、重挂、slot 切换和已填充 Shadow 宿主搬移会同步更新并释放旧几何依赖。
- 保留页面原有资源识别、请求凭据、下载交接、字幕、投屏和 TVBox 入口。本轮未修改这些协议或重放规则。
- `scripts/validation/smoke_extension_media.py` 新增 `geometry` 场景，实际操作样式表、slot、Shadow 宿主及 3000 次无关属性变化，验证按钮位置、可见性、资源归属和按钮对象稳定性。

整理时确认项目根目录没有待迁移的松散脚本；保留既有源码改动、验证证据、下载/任务恢复数据、`outputs/private-user-backup` 和常规构建缓存。删除已结束进程 39056 的唯一已确认测试残留 `.tool-cache/test-tmp/hls-v7-advanced-defaults-39056`（一个 3 字节 `control` 文件及空父目录）。对应 `advanced_defaults_are_applied_and_host_scope_is_enforced` 测试改为先取消并等待下载线程退出，再检查目录删除成功，避免线程在清理后重新写入控制文件。

本轮实际验证：

- 扩展定向 Vitest：`mediaOverlay`、`resources`、`siteManifestHooks` 共 3 个文件、92 项通过；最终 TypeScript 检查通过。
- Chrome MV3、Firefox MV3 生产构建通过，产物位于 `extension/.output/`。
- Edge 完整本地媒体场景由 23 个增至 **24 个，全部通过**。Firefox 的 `geometry/direct/shadow/dynamic-shadow/iframe` **5 个场景全部通过**。两个浏览器均验证裁剪后按钮左坐标由 668 更新至 428，资源 ID 保持一致，slot/宿主移动及无关属性变化检查通过。JSON 报告位于 `artifacts/optimization-20261105/`。
- Rust 离线测试编译完成（含依赖约 5 分 03 秒），上述清理回归 **1 项通过**，用例运行约 0.05 秒；本轮只修改 Rust 测试，不重复 Core 全量回归或发布构建。`rustfmt --check`、Python 脚本语法检查及 `git diff --check` 通过。
- 浏览器、驱动和本地媒体服务已由脚本退出；本轮检查结束时 `.tool-cache/test-tmp` 为空。保留当前扩展构建及 JSON 验证证据，没有暂存、提交、推送或发布。

以上验证证明相关行为与回归结果，没有测量空闲 CPU、内存占用或与 IDM 的吞吐差异，不能将其表述为性能已达到 IDM。

## LL-HLS 投送重载与精确凭据（2026-11-05）

继续检查媒体代理时，确认接收端请求中的查询参数在本地路由阶段被丢弃，导致 LL-HLS 的阻塞重载和增量清单请求退化为普通清单请求。依据 [HLS 第二版工作草案 §6.2.5](https://datatracker.ietf.org/doc/html/draft-pantos-hls-rfc8216bis-22#section-6.2.5)，本轮在 `native_shell/src/media_proxy.rs` 和 `native_shell/src/http_engine.rs` 完成以下修复：

- 仅接收并校验 `_HLS_msn`、`_HLS_part`、`_HLS_skip`；重复、非法数值及缺少 msn 的 part 请求直接返回 400，不访问上游。接收端其他参数及鉴权头不能改写原始媒体身份。
- 保留源 URL 中签名参数的原始编码、顺序和空参数；替换旧重载指令，已知 HLS 的首次完整请求清除捕获地址中的旧指令。已知清单忽略 Range，避免不完整清单。
- 将用于选择精确凭据的原始 URL 与实际网络请求 URL 分开；加入重载指令后，同路径不同查询参数的音视频清单仍使用各自已捕获的 Authorization、Cookie 和请求上下文。重定向继续遵守既有凭据作用域。
- 上游 400、503 等 4xx/5xx 状态码原样返回，私有响应头和错误正文不外传；分片路由保持稳定，不继承清单重载指令。

本轮实际验证：

- 先加入 `ll_hls_proxy_` 三项 HTTP 集成回归，在修复前全部失败，分别复现增量请求未到达上游、无效指令仍返回 200、上游错误变为 502；修复后三项全部通过。
- 离线运行 `media_proxy::tests`、`http_engine::tests`、`credentials::tests`，排除上述已通过三项，另外 **99 项通过、8 项按既有条件忽略**。本轮合计 **102 个不同测试通过**；忽略项依赖 Windows Sandbox/curl-impersonate 环境。覆盖精确凭据、Cookie、跨域重定向、Range/HEAD/POST、续传、元数据限长、HLS/DASH 代理及停止/断线释放。
- 两个变更 Rust 文件的 `rustfmt --check` 和 `git diff --check` 通过。Core 与 Native Host 离线 Release 构建成功，用时约 2 分 51 秒；产物位于 `.tool-cache/build-cache/cargo-target/release/`。
- 更新产物的隔离 Native Messaging 启动检查通过：两帧 ping 正常返回，仅一次 Core 启动、一个常驻 Core；本次首次回执为 **1217.38 ms**，低于脚本的 1500 ms 门槛。报告见 [ll-hls-native-host-startup.json](../../artifacts/optimization-20261105/ll-hls-native-host-startup.json)。该单次启动结果不作为性能对比基准。
- 本轮没有重跑扩展、桌面 UI、实体投送设备或 Core 全量套件，也没有安装、提交、推送或发布。隔离测试目录与测试 Core 在检查后清理，保留构建产物和验证证据。

## 插件探测释放与下载响应复用（2026-11-05）

本轮聚焦浏览器插件与普通文件下载主体。再次核对原始 IDM 插件 `src/idmmzcc3-6.43.1/deobfuscated/background.js`：`nc`（1378 行）按响应状态、类型和附件头判定，`hc`（1320 行）在释放对应请求时中止关联 XHR，`X`（1490 行）在裁决和资源持有结束后移除请求记录。本轮采用其中的请求生命周期管理思路；以下 Core 修复由项目自身失败回归驱动，不将其描述为已还原的 IDM 原生下载算法。

- 插件 `inspectAdaptive` 原先仅通过页面令牌阻止旧结果保存，页面跳转、重载或关闭后，网络探测与清单正文读取仍继续。现在每个标签页共用可取消的页面上下文，页面失效时实际中止这些请求及正文读取；收集凭据前后检查取消状态，取消或超时后不再换头发起回退请求。保留逐 URL、逐 frame 的精确凭据选择及 HLS 子清单解析。
- Core `run_job_report` 原先在 Range 探测收到整文件 200 后销毁原响应，再由单流下载重新 GET。这会多发请求，并可能耗掉一次性地址。现在探测同时返回可消费的响应，首次、无续传偏移的 200 整包直接进入既有单流流程，继续执行正文完整性、暂停、取消和进度处理。支持 Range 的 206、镜像身份核对和已有续传拒绝规则保留。

本轮实际验证：

- 插件先运行新增取消检查与扩展的页面生命周期回归，**6 个用例在修复前失败、修复后通过**。另外新增真实本地 HTTP 测试，确认页面取消能中止未结束的响应正文。
- 扩展完整 Vitest **54 个文件、532 项通过**；TypeScript 检查及 Chrome/Firefox MV3 生产构建通过，产物位于 `extension/.output/`。
- Core 新增 `non_range_probe_preserves_the_only_download_response`，测试服务器仅服务一次就关闭，修复前因第二次请求被拒绝而失败；修复后有、无 Content-Length 两种 192 KiB 正文均逐字节正确，签名查询和 Authorization 保留。
- 随后运行 `http_engine::tests` 与 `mirrors::tests`，排除已通过的新用例，**79 项通过、8 项按既有条件忽略**；本轮合计 **80 个不同 Rust 测试通过**。覆盖 Range/续传、校验器、镜像、POST/HEAD、截断正文及暂停/取消。忽略项仍依赖 Windows Sandbox/curl-impersonate 环境。
- Core、Native Host 离线 Release 构建成功，用时约 2 分 30 秒。隔离 Native Messaging 两帧 ping 与单实例检查通过，首次回执 **1259.66 ms**，报告见 [request-lifecycle-native-host-startup.json](../../artifacts/optimization-20261105/request-lifecycle-native-host-startup.json)。未测量与 IDM 的速度、CPU 或内存差异。
- 使用项目已有 Firefox 与 geckodriver 缓存，`spa`、`hls-mse`、`dash-mse` **3 个真实浏览器场景通过**，验证页面切换后的资源归属、实际播放、悬浮入口及下载/TVBox/投屏菜单。报告见 [request-lifecycle-firefox-smoke.json](../../artifacts/optimization-20261105/request-lifecycle-firefox-smoke.json)，Firefox 测试目录和进程由脚本退出清理。
- Edge 本地媒体联调在退出清理时因隔离目录中的组件更新 BITS 文件被占用而失败，未生成成功报告，本轮不计为 Edge 联调通过。清理该更新任务及其测试目录的命令被自动审批以 `blocked by policy` 拒绝，后续限定到不含更新文件的目录清理也被拒绝；未绕过拒绝或改动系统更新设置。

本轮保留的清理残留为 `.tool-cache/test-tmp/hls-extension-smoke-arqg0ob0/`，包含生成的媒体夹具、日志与 `msedgeedge_BITS_14328_248828721/BIT2E16.tmp`；对应 BITS JobId 为 `bdb10542-b665-4061-af3f-04bf739e27c2`。检查时没有仍引用该目录的 Edge、WebDriver 或测试 Python 进程；更新任务释放文件后，该已确认测试目录可由用户清理。既有源码改动与验证证据均保留，没有暂存、提交、推送、安装或发布。

## 片段导航与分段响应复用（2026-11-05）

继续落实上一轮核查中能用项目回归验证的优化，没有移植未读的 IDM 原生代码：

- 后台记录已观察的顶层页面身份，忽略 URL 片段变化；页面资源保存和异步探测完成检查也使用相同规则。路径、查询变化、同地址重载和关闭仍会中止旧探测并清理请求身份；未知先前页面时保守清理。
- 请求链已有明确 `requestId` 时直接从现有 Map 取候选，避免展开整张表进行匹配和排序。URL、tab/frame、响应状态和元数据校验仍执行；ID 不存在时不借用其他请求。既有过期清理扫描保留，因此不宣称整个查找过程变为常数时间。
- 分段 worker 收到可接受的完整 200 响应后，将原流交回调用线程，仅首个终止结果可持有它；所有 worker 退出后再重建输出并消费该响应，不重新 GET、不在内存缓存整包。已有有效断点或续传偏移仍拒绝整包回退；ETag 冲突和已知 Last-Modified 冲突均拒绝复用，回退前检查暂停/取消。

先加入失败回归：片段变化错误取消探测的扩展用例在修复前失败；Core 的服务器只服务一次后关闭，旧分段回退因重新连接被拒绝而失败。扩展修复后完整 Vitest **54 个文件、534 项通过**，TypeScript 检查和 Chrome/Firefox MV3 构建通过。

Rust 测试目标重新编译后，`range_worker` 定向 **5 项通过**；随后执行其余 `http_engine::tests`，另外 **72 项通过、8 项按既有条件忽略**，本轮合计 **77 个不同 Rust 测试通过**。新增测试涵盖单/双连接且有/无 Content-Length 的完整响应复用、签名查询和 Authorization 保留、ETag/Last-Modified 冲突，以及收到响应时暂停/取消不写正文。已有 Range、检查点、镜像、POST/HEAD、跨域 Cookie、截断和停止回归通过。忽略项依赖 Windows Sandbox/curl-impersonate，没有计为验证成功。

使用更新的扩展构建执行 Firefox 的 `spa`、`hls-mse`、`dash-mse` **3 个真实浏览器场景通过**，各场景均无浏览器运行错误；验证实际播放、资源归属、悬浮入口及下载/TVBox/投屏菜单。报告见 [fragment-navigation-firefox-smoke.json](../../artifacts/optimization-20261105/fragment-navigation-firefox-smoke.json)。片段变化保留请求身份及带凭据下载由注册浏览器事件的回归覆盖，不能将这些浏览器场景表述为真实站点鉴权验收。

Core、Native Host 离线 Release 构建成功，用时约 2 分 17 秒，本地产物已更新。隔离 Native Messaging 两帧 ping、单次 Core 启动和单实例检查通过，首次回执 **1180.04 ms**，低于脚本既有 1500 ms 门槛；报告及产物 SHA-256 见 [fragment-range-native-host-startup.json](../../artifacts/optimization-20261105/fragment-range-native-host-startup.json)。这是单次启动检查，没有测量与 IDM 的吞吐、CPU 或内存差异。`rustfmt --check` 和 `git diff --check` 通过，没有安装、暂存、提交、推送或发布；`release_ready=false` 保留。

Firefox 和 Native Messaging 的临时进程及目录已由脚本退出清理。本轮结束时 `.tool-cache/test-tmp` 仅剩上一轮被自动审批拒绝清理的 `hls-extension-smoke-arqg0ob0`，未重试或绕过该拒绝；源码、构建产物及验证证据均保留。

清单重复探测本轮未改：主世界解析使用网页已取得的正文，但 HLS 主清单的子清单可能尚未被页面观察，后台探测仍能补齐时长和片段关系。当前缺少可安全取消单次后台工作的完整关联，直接按 URL 合并会混淆不同 frame 或鉴权请求；本轮没有新增队列、延迟窗口或仅按 URL 的复用机制。

## Firefox 明确附件的请求前交接（2026-11-05）

本轮交付目标继续为当前 `hls-downloader` 项目。只读核对 `A:\Ubuntu\逆向` 新归档的原版插件、Ghidra 伪代码和重建实现；后两者存在不能直接等同原版行为的证据边界，具体矛盾见 [IDM 静态核查补充](idm-native-handoff.md#重新归档的参考目录与请求前接管2026-11-05)。没有修改或运行参考目录内容。

新增 Firefox 专用的阻塞 `onBeforeSendHeaders` 路径：用户实际点击已列入附件类型的精确链接、当前为首次 GET 导航、没有已识别的非直连代理时，使用此次请求链提前交给 Core。此时 HTTP 数据尚未发出，Authorization、Referer、Origin、User-Agent 和签名查询参数来自本次请求；不会借用同 URL 的旧响应。POST、XHR、普通 MP4/PDF 预览、已重定向或已响应的请求继续原有流程。配置了最小文件大小时留到响应后处理，排除站点、Alt 绕过及站点抑制规则仍适用。

Cookie 继续要求站点单独授权，并从发起标签页的 Cookie store、分区和目标路径导出。只有快照与此次实际 Cookie 头一致时才提前交接；不一致时保留浏览器传输。校验和交接共享同一份 Cookie 快照，避免二次读取期间的轮换改变下载身份，未放开普通请求头对 Cookie 的过滤。

提前交接复用现有持久业务 ID：回执丢失时停止尚未发出的 HTTP 请求并提示用户；后续浏览器 requestId 改变或后台重载仍查询原 ID，桌面离线不能让未决交接另发原请求。连续点击被内容脚本合并、关闭接管或 Cookie 改变导致新请求不满足提前交接条件时，也先读取已有交接结果；这条查询不会另发 offer。首次连接明确离线或明确拒收时放行，并保留一次绕过意图，后续响应及 DownloadItem 不再重复弹出交接。桌面持久接收后的取消继续结束本次下载。这与“已经收到响应的 HLS/DASH 丢回执时保全原响应”是不同阶段，未修改后者。

本轮实际验证：

- 新增回归先因缺少请求头阶段监听而失败；复查又复现了缺失点击意图、关闭接管和 Cookie 改变时未保护旧交接的 3 项失败，修复后通过。`downloadOwnership.test.ts` 完整 **92 项通过**，完整扩展 **54 文件、554 项通过**。
- 回归覆盖精确 URL 和五类凭据、单份 Cookie 快照、未授权或不匹配 Cookie、策略关闭/按大小过滤、POST/XHR/MP4/代理保全、缺失/不匹配点击和 Alt、明确拒收后不重复接管、丢回执、后台重载及桌面离线。类型检查通过。
- Chrome MV3 与 Firefox MV3 构建通过；Firefox 使用更新产物运行 `spa`、`hls-mse`、`dash-mse` **3 个真实浏览器场景通过**，报告见 [firefox-pre-request-media-smoke.json](../../artifacts/optimization-20261105/firefox-pre-request-media-smoke.json)。这些场景核对媒体播放、识别、悬浮入口和菜单，不是附件请求前交接的真实账号端到端验证。
- 本轮没有修改 Rust、桌面协议或 Native Host，未重复其编译和上一轮已通过的定向验证。没有运行需要独立 Windows Sandbox 的真实 Host/Core 附件接管脚本，没有修改系统 Native Messaging 注册或用户浏览器配置。未执行真实账号、安装升级、签名、投屏设备及 IDM 性能对照。

浏览器烟测的临时配置和服务已由脚本清理；历史 `.tool-cache/test-tmp/hls-extension-smoke-arqg0ob0` 仍保留，未重试或绕过之前的自动审批拒绝。当前构建位于 `extension/.output/chrome-mv3` 和 `extension/.output/firefox-mv3`；未安装、暂存、提交、推送或发布，`release_ready=false` 保留。

## 最终复核与回归收尾（2026-11-05）

本轮针对“是否全部完成、检查好”的要求，复查当前实现并扩大到扩展、Rust 全目标、桌面测试和 Firefox 全部本地媒体场景。以下为本轮最终结果；上文各轮数字保留为历史记录。

复核发现 Firefox 请求前交接仍有准备阶段的并发窗口：第一次导航还在读取 Cookie、尚未登记 offer 时，第二次导航可能因没有新的点击意图而放行，形成浏览器与桌面重复传输。新增受控 Cookie Promise 回归先复现失败，再修复为同一 tab、frame、精确 URL（保留查询参数、忽略 fragment）共享正在准备的结果，完成后释放记录。第二次导航会随第一次交接结果处理，只创建一份 offer；原有持久回执、身份快照校验及桌面接收后取消规则保留。

| 检查 | 本轮最终结果 | 证据 |
| --- | --- | --- |
| 扩展完整测试及类型检查 | 54 文件、555 项通过；包含 WXT prepare 和 TypeScript | [extension-tests.log](../../artifacts/final-check-20261105/extension-tests.log) |
| Chrome / Firefox MV3 生产构建 | 两种构建均通过，产物在 `extension/.output/` | [extension-build.log](../../artifacts/final-check-20261105/extension-build.log) |
| Rust 全目标测试 | 713 项通过、0 失败、12 项按既有条件忽略 | [rust-all-targets-serial.log](../../artifacts/final-check-20261105/rust-all-targets-serial.log) |
| Presenter 独立编译 | `--no-default-features --features presenter-client --all-targets` 离线检查退出码 0 | [presenter-client-check.log](../../artifacts/final-check-20261105/presenter-client-check.log)（成功时无输出） |
| 桌面测试 | 全套 153 项中 149 通过、4 跳过；补跑侧栏及指针测试 3 项通过，合计 152 个不同测试通过、1 项跳过 | [desktop-results.json](../../artifacts/final-check-20261105/desktop-results.json)、[desktop-pointer-results.json](../../artifacts/final-check-20261105/desktop-pointer-results.json) |
| Firefox 真实浏览器媒体联调 | 24 个本地场景全部通过 | [firefox-media-full.json](../../artifacts/final-check-20261105/firefox-media-full.json) |
| 格式与改动检查 | `cargo fmt --check`、`git diff --check` 通过，暂存区为空 | 当前工作树 |

Firefox 场景覆盖直链、多来源、srcdoc、Shadow DOM/动态 Shadow DOM、几何与遮挡、同源/跨源 iframe、广告播放器、SPA、MSE/切片/加密结果归属、Worker/独立 Worker/多 Worker、多播放器、HLS/LL-HLS、DASH/低清晰度/多播放器/未知表示及空视频源。这些场景验证实际播放、媒体归属、悬浮入口和菜单，不替代真实账号的附件 Host/Core 端到端接管验收。

Rust 全目标验证使用项目既有 `CARGO_HOME`，执行 `cargo test --locked --offline --manifest-path native_shell/Cargo.toml --all-targets --jobs 1 --quiet -- --test-threads=1`。12 项忽略仍涉及 Sandbox、打包 FFmpeg/curl-impersonate、真实 UAC/AV 等环境条件，未计为通过。桌面剩余 1 项为需 `HLS_VM_ACCEPTANCE=1` 的真实窗口唤醒测试；侧栏及指针补测使用 `HLS_UI_AUDIT_SURFACE=tasks_1000`，不能据此宣称窗口、DPI、拖动等全部实机行为已验收。

保留初始失败与最终结果的区别：

- 首次 Cargo 离线解析误用默认缓存，缺少锁定的 `roxmltree 0.21.2`；改用项目已有缓存解决，未下载依赖。
- 首次实际 Rust 测试为 693 通过、1 失败、12 忽略。本机 `cmd` AutoRun 的 Conda 激活失败污染了 `mklink` 的退出码；另一处测试也可能因此跳过有效断言。仅在两处测试夹具的 `cmd` 参数中增加 `/D`，随后全目标串行回归通过；没有修改注册表、Conda 或产品下载行为。失败夹具已归档为 [rust-initial-failure-state.zip](../../artifacts/final-check-20261105/rust-initial-failure-state.zip)。
- 桌面首次 `.bat` 调用日志虽为 `BUILD SUCCESSFUL`，外层返回码受 AutoRun 影响；随后直接用项目 JDK 调用 Gradle wrapper，确认退出码 0。中途并行编译、Gradle 和浏览器造成内存压力，主动停止了本轮 Gradle daemon，该次中止不计成功；Firefox 首次 WebDriver 启动超时也不计成功。最终改为串行，分别完成桌面补测、24 个 Firefox 场景和 Rust 全目标验证。
- `cargo fmt --check` 发现两份已修改 Rust 文件的格式问题；仅对 `lib.rs`、`media/dash.rs` 收尾格式化，随后检查通过，没有新增产品行为变更。

本轮验证进程已退出，`.tool-cache/test-tmp/final-check-20261105` 已清理；日志、结果、失败归档和构建产物保留。历史 `.tool-cache/test-tmp/hls-extension-smoke-arqg0ob0` 仍保留，未重试或绕过此前的自动审批拒绝。

本轮未完成真实账号附件接管、原生窗口/DPI/任务拖动、当前安装包升级回滚和签名发布等全量验收，也没有与 IDM 做性能对照。投屏/TVBox 实机与云字幕验收继续遵守 `feature-parity.json` 中既有用户排除。该表仍为 23 项 `verified`、10 项 `partial`，`release_ready=false` 保留；本轮复核完成不等于整个产品已经发布就绪。没有安装、暂存、提交、推送、发布或修改参考目录。

## 原版响应判定与页面正文复用继续落实（2026-11-05）

本轮继续只读核对当前参考目录中的原版插件：`1_浏览器插件源码/idmmzcc3_6.43.1_deobfuscated/background.js` 的 `nc`（当前压缩排版第 84–90 行）只接受 200、206、304，并结合 Content-Type/Content-Disposition 判定候选；`document.js` 的 `Ma/P/aa` 及原型读取方法从页面实际读取的 text、JSON、二进制和 Blob 结果提取正文。本轮将这些可确认的机制用于项目的媒体识别路径，没有移植重建版的原生调度或私有通信协议。

实际修改：

- 后台 webRequest、MAIN-world fetch 和 XHR 统一响应状态判定。错误响应、重定向和 204/205 不再持久保存、探测或上报为媒体；304 仍保留原请求身份和凭据，不另发空正文探测。
- 清单地址的 URL 后缀不再压过明确的 HTML/XHTML、JSON、图片等非媒体 MIME，修复登录页或错误数据被显示为 HLS/DASH 的问题；通用 XML 的 DASH 清单仍可识别。
- 为 `Response.json()` 和 `Response.text()` 补齐受站点规则约束的播放器元数据识别。播放器在插件钩子前保存原始 fetch 时，仍能利用实际读取结果提取清单；普通 JSON 接口不触发站点清单提取，不为这条读取路径增加请求或重新消费原响应。
- 同一响应与其克隆通过 WeakMap 共享正文观察状态。页面读取和有界克隆探测只提交一份解析结果；探测发现已有结果或来源页面变化时停止后续读取并释放克隆 reader。同 URL 的另一 HTTP 响应独立处理，仍更新轮换的签名链接；保持正文返回值、原有 JSON 解析异常和页面身份隔离。

新增回归先复现 **10 项失败**，补查 MIME 后又复现 **2 项失败**，修复后完整扩展 **54 文件、572 项通过**，包含 WXT prepare 和 TypeScript 检查。回归覆盖后台缓存与页面消息、错误状态、304 凭据、HTML/JSON 伪清单、XML DASH、预存 fetch、JSON/text 读取、克隆去重、新响应签名更新、JSON 原始异常、普通接口保全和页面切换。证据见 [before-fix.log](../../artifacts/idm-response-reuse-20261105/before-fix.log)、[mime-before-fix.log](../../artifacts/idm-response-reuse-20261105/mime-before-fix.log) 和 [extension-tests.log](../../artifacts/idm-response-reuse-20261105/extension-tests.log)。

Chrome/Firefox MV3 生产构建通过。更新后的 Firefox 产物执行 `direct`、`spa`、`mse-sliced`、`mse-worker`、`hls-mse`、`dash-mse` **6 个真实媒体场景全部通过**，验证实际播放、来源归属、悬浮入口及菜单。报告见 [firefox-media.json](../../artifacts/idm-response-reuse-20261105/firefox-media.json)，源码与构建 SHA-256 见 [source-build-sha256.json](../../artifacts/idm-response-reuse-20261105/source-build-sha256.json)。浏览器 stderr 仅记录既有 Selenium `install_addon` 弃用提示，不将其描述为播放错误。`git diff --check` 通过。

本轮没有修改 Rust、桌面、Host 协议或请求凭据保管，不重复上一轮已经通过的相关编译与测试；它们的结果与未执行项见前节。本轮浏览器临时配置、媒体夹具、服务及进程已由脚本退出清理，保留报告与构建产物；历史被拒绝清理的目录保持原状。没有暂存、提交、推送、安装或发布，`release_ready=false` 保留。本轮不构成 IDM 原生浮窗、完整下载算法或性能等同的验证。

## 旧页面克隆探测及时释放（2026-11-07）

只读复核本地最新 IDM 还原材料，新增进展及仍有冲突的调度归属见 [IDM 参考增量](idm-native-handoff.md#本地最新参考增量复核2026-11-07)。本轮修改聚焦当前 HLS 插件的真实资源生命周期缺口，没有复制尚未证实的下载算法。

MAIN-world 清单探测此前只在 `reader.read()` 返回后检查页面身份。旧响应持续不返回时，SPA 换页无法立即停止这条克隆读取。现记录正在进行的探测，收到 History 导航、`popstate` / `hashchange` 时仅取消页面身份已变化的 reader，`pagehide` 时取消本页全部探测；完成、出错和超限同样收尾，取消幂等。只释放插件克隆分支，不等待 tee 的取消回执，不取消网页原响应。路径/查询变化及时释放，只改 fragment 保留探测。请求凭据、签名 URL、下载交接及桌面接收后的取消合同未改。

本轮实际验证与产物位于 `artifacts/idm-stream-lifecycle-20261107/`：

- `before-fix.log`：四项回归先复现 pushState、replaceState、popstate 和 pagehide 未取消等待中的克隆 reader；fragment 用例通过。修复后 `targeted-tests.log` 的 14 项通过。随后增强为先缓存一段清单、再等待未结束正文，`partial-body-tests.log` 的 14 项仍通过；每个场景均核对原响应完整文本及只发起一次原请求。
- `extension-tests.log`：WXT prepare、TypeScript 和完整 Vitest **54 文件、580 项通过**。增强后的部分正文夹具另跑上述定向回归，没有重复未受影响的整套测试。
- `extension-build.log`：Chrome / Firefox MV3 生产构建通过，产物在 `extension/.output/`；这是目录构建大小，未做安装包体积或 IDM 性能比较。
- `firefox-media.json`：最新 Firefox 构建的 **spa、hls-mse、dash-mse 三个真实本地媒体场景通过**，包括播放、资源归属、悬浮入口和下载/TVBox/投屏菜单，`browserErrors` 均为空。未结束响应的立即取消由受控流回归验证，这三个浏览器场景不替代该边界的真实网络复现，也不代表真实账号 Host/Core 接管已验收。
- Firefox 夹具进程、服务及 `.tool-cache/test-tmp/hls-extension-smoke-aif5lehb` 已退出清理。两处历史被拒绝清理的浏览器目录保留，未重试。本轮误建的空目录 `extension/artifacts/idm-stream-lifecycle-20261107/` 移动与删除命令被自动审批以 `blocked by policy` 拒绝，原样保留，未换工具绕过。

本轮未改 Rust 或桌面 UI，没有重复它们未受影响的编译。正式入口仍为 Compose，完整原生迁移未完成，`release_ready=false`；未安装、暂存、提交、推送或发布。

## 媒体合流成功后替换（2026-11-08）

重新核对本地新增的 IDM HTTP、连接和媒体后处理材料，调查快照已到 §160。具体参考函数、恢复文件中的标注冲突和采用边界见 [IDM 增量核查](idm-native-handoff.md#下载线程与媒体后处理增量复核2026-11-08)。本轮修改聚焦 `native_shell/src/media/merge.rs`，没有改浏览器接管、凭据、UI 或协议。

实际复现了直接输出的破坏行为：FFmpeg 用 PCM 音频写不支持该编码的 WebM，返回封装错误，但原先 24 字节的成品仍被改写成 262 字节的半成品。见 [before-fix.json](../../artifacts/idm-media-publish-20261108/before-fix.json)。这验证当前直接输出方式的风险，不是 IDM 运行对照。

修复后两个 FFmpeg 入口使用同目录、独占创建并保留后缀的暂存文件。进程成功退出、文件同步并再次检查暂停/取消后，调用既有原子替换函数；错误、暂停和取消只清理本次暂存，源轨道和旧结果保留，诊断日志继续保存。无额外轨道时复用安全拼接，支持不同路径表示同一输入文件。没有新依赖或额外转码步骤；成功提交使用同目录替换，不再额外复制整份 FFmpeg 输出。

本轮实际验证 **70 个不同测试通过，0 失败**：

| 检查 | 结果与范围 | 证据 |
| --- | --- | --- |
| Rust 合流定向编译与测试 | 离线锁定依赖编译成功；8 项常规测试通过，2 项真实 FFmpeg 测试先按注解忽略 | [merge-tests.log](../../artifacts/idm-media-publish-20261108/merge-tests.log) |
| 显式真实 FFmpeg 回归 | 上述 2 项随后在本机项目内临时目录显式执行，均通过：失败保全、暂停/取消及时回收、无暂存残留、保留中文输出名与容器后缀、同文件双输入/输出的成功封装，以及成功结果再次解码 | [real-ffmpeg-tests.log](../../artifacts/idm-media-publish-20261108/real-ffmpeg-tests.log) |
| 其余媒体模块回归 | 复用本次测试程序，排除已经执行的合流测试，60 项通过；覆盖 HLS/DASH 凭据、签名链接、断点复用、预览、分片错误处理及字幕 | [media-regression-tests.log](../../artifacts/idm-media-publish-20261108/media-regression-tests.log) |
| 格式与补丁检查 | 修改文件的 `rustfmt --check` 和 `git diff --check` 通过 | 当前工作树 |

参考输入的 SHA-256 见 [reference-sha256.json](../../artifacts/idm-media-publish-20261108/reference-sha256.json)，HLS 修改、测试程序和实际 FFmpeg 的哈希见 [source-build-sha256.json](../../artifacts/idm-media-publish-20261108/source-build-sha256.json)。真实 FFmpeg 测试是本机隔离夹具，未运行 Windows Sandbox、参考程序或真实账号接管。未修改扩展和 UI，因此没有重复未受影响的浏览器构建、烟测或桌面编译；未重建安装包，也未进行 IDM 速度、占用和体积对比。

正常错误、暂停和取消的暂存清理由回归验证；外部强杀或断电仍可能遗留未提交暂存，不能宣称断电恢复已完成。完整原生迁移及全量发布验收继续保持未完成，正式入口仍是 Compose，`release_ready=false`。没有安装、暂存、提交、推送或发布。日志、哈希和构建产物保留在项目既有产物目录。

## 辅助媒体请求身份与 frame 隔离（2026-11-08）

继续参考原版插件 `background.js`：`nc`（当前压缩排版第 84 行起）只按 200/206/304 进入有效媒体响应判定；`Y`（第 89–90 行）携带具体请求的序号、tabId、frameId 和请求头。将这种“按实际请求和来源关联”的思路用于 HLS 已复现的问题，没有移植私有 MSG# 协议或未经证实的重建调度。

确认的问题：主清单已按 frame 匹配，但 `RequestChainStore.contextsForPage` 收集辅助音轨、分片和密钥上下文时只筛标签页和页面。两个 iframe 来自同一页面、访问相同 CDN 路径时，较新的另一 frame 请求会覆盖所选播放器的辅助身份，甚至将另一 frame 独有主机的上下文导出。另一个问题是按更新时间取最新请求，会使较新的 403 或尚未确认的重试替换同一 URL 之前已经成功的凭据。

实际修改：

- `extension/lib/requestChain.ts` 支持按 frame 选上下文；排除已经收到非 200/206/304 响应的请求及响应前已经失败的请求。对于同一个精确 URL，优先使用最新的有效响应身份；没有有效响应时，仍保留正在进行的请求头作为备用，不新增网络探测。
- `extension/entrypoints/background.ts` 将实际所选主请求的 frame（无主请求时用资源 frame）传入辅助上下文收集，保留既有精确 URL、目录身份、Cookie 快照和 32 项上限。
- 回归覆盖顶层 frame 和子 frame、相同 URL 的不同 Authorization、另一 frame 的独有主机、204/205/302/401/403/500、未完成的重试、响应前失败及后续 304 更新。通过真实注册的后台事件监听器检查最终 offer 中音轨、视频和密钥的身份，未只测独立筛选函数。

实际验证：新增/扩展场景在修复前复现 **12 项失败**；修复后请求链和下载接管定向 **145 项通过**；扩展全套 **54 个文件、591 项通过**（包含前述定向测试），同一 `pnpm run test --maxWorkers 1` 流程中的 WXT 类型生成与 `tsc --noEmit` 通过。`pnpm run build` 的 Chrome MV3、Firefox MV3 构建均成功，修改文件的 `git diff --check` 通过。

日志位于 `artifacts/idm-request-contexts-20261108/`：`regression-before.log`、`request-context-tests.log`、`extension-full-tests.log`、`extension-builds.log`；原版输入、修改源码和构建文件哈希见 `source-build-sha256.json`。构建产物保留在扩展既有 `.output/chrome-mv3/`、`.output/firefox-mv3/` 中。

本轮没有新增依赖、修改 UI、Rust 下载引擎或参考目录，没有重建桌面安装包、安装插件或进行真实账号站点端到端验收；后台事件联调使用本地模拟 Native Messaging，不等于真实浏览器/Host/Core 全链通过。未进行 IDM 的速度和内存对照，`release_ready=false` 保持原有状态。

## 尚未闭环

- 凭据投屏已补齐本地实现与模拟上游回归，但未进行实体 DLNA/Chromecast/TVBox 验收。代理不支持外部 XML 引用的 MPD、未解析的 HLS 变量及 DRM 解密；不能将这些情况算作支持。没有与 IDM 进行速度或内存对比基准。
- 浏览器验证范围与失败记录见上述各轮说明，历史报告保留；真实站点账号、安装升级、云字幕、实体 TVBox、签名和 CI 等验收没有在本轮执行。此前的[收敛记录](convergence.md)保留各自证据与缺口。
- `config/release/feature-parity.json` 的 `release_ready=false` 和当前全量证据表保持原状。本次定向检查不代表整个产品或安装包已通过全量验收。
