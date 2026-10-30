# 免费版主流程对标实施记录

用户已确认实施 Windows x64 免费版计划。保留网站 Cookie/请求头，不新增账号、会员、激活、企业认证或收费 API 前置。录制、多平台、驱动级接管、真实投屏/TVBox 和收费云字幕验收不在本轮范围。不得推送、发布或签名。产品和竞品只在 Windows Sandbox 中运行。

## 阶段

| 阶段 | 当前状态 |
| --- | --- |
| P0 安装器与证据 | f53019e 候选 r14 同版本运行中升级、故障回滚、64 组件、续传、空闲及运行中卸载通过；完整便携包升级/回滚和活动检查点通过。5 个原生成品卸载后保持哈希。历史 1603 原样保留。 |
| P1 UI 与原生操作 | r14 真实输入复核普通 URL Enter、空输入禁用、多行 cURL 换行、Esc、批量、导出/独立 URL 文件导入、Ctrl+A/Esc、并发设置保存/Core 读回/工作台重启、浅深主题及 ZIP 滚动。完整 OS DPI、有效最小窗口、拖动及主题/控件组合矩阵未完成。 |
| P2 浏览器交接 | r14 Edge、Firefox 各 11 项及两浏览器初始离线通过。Firefox 重载及重启的清理分别约 748/14 ms；r13 超时保留，不能由一次成功认定延迟彻底消除。DownloadItem 按入口记录。 |
| P3 分段与恢复 | r14 HTTP 13 项、普通文件/HLS/DASH 成品及解码帧、直连/系统代理活动拆分恢复通过。三轮活动拆分性能不稳定，默认关闭；全产品性能对照未完成。 |
| P4 CLI | 已进入安装与便携包。f53019e 的 r14 免费整套 11 组通过，含单 Core、中文路径、生命周期、重启续传、队列启停、导入导出、退出码及显式删文件。 |
| P5 抓站与同步 | r14 整套通过：循环去重、预览确认、恢复、静态改写、跨域边界、页面/深度限制、ETag/失败保旧/原子替换、关闭队列时间窗零请求、请求上下文及 60 秒自动周期。完整原生 UI 矩阵未完成。 |
| P6 ZIP 预览 | r14 本地/远程、中文、空包、ZIP64、损坏拒绝、资源变化、非 Range 拒绝、有界读取及条目上限通过。同包原生中文路径 121 项、滚动/关闭通过；完整主题/缩放组合未完成。 |

## P0 首轮诊断边界

工作目录为 `.tool-cache/free-parity-20261030/`。`results/` 首轮使用自定义管道，而 MSI 助手未继承该环境，不能作为生产默认入口的失败复现；旧 MSI 返回 1603，直接助手随后成功，结果原样保留。

`matrix-results/` 改用生产默认管道：旧 MSI 安装/运行中卸载返回 0；诊断 MSI 安装/运行中卸载返回 0。日志显示 checkpoint 命令返回 0。诊断 MSI 仅替换安装助手用于定位问题，不是完成本计划的新候选，不回写旧包来源。

后续同包通过和新增失败见下节。旧候选的 1603 失败保留，不能用多次成功覆盖新的失败。

## 当前候选 f53019e

源码 `f53019e0968c2d919d4c1ace2aa099f18679c678`，打包前工作区干净，包含便携升级检查点修复 `425ab01`。新包位于 `artifacts/v7-productization/candidate/`；原始[构建来源](results/candidate-f53019e/BUILD-PROVENANCE.json)、[交付清单](results/candidate-f53019e/ARTIFACT-MANIFEST.json)与[64 组件及便携助手清单](results/candidate-f53019e/expected-components.json)已保存。

| 文件 | SHA-256 |
| --- | --- |
| EXE | `fba69d349d85991476009069754dc6232948c4daa60c33c6901d08595bc23ec9` |
| MSI | `c46b241618a1b05bb412f367ffb05b31e7c0d1ce4c2dc41d9880db4da5947734` |
| 便携 ZIP | `da1e067d959faca049674c69f19ef8e331c1f74ac7cb97d446657807dda604c9` |

固定 pnpm 11.7.0、现有 JDK 21、Cargo 和 FFmpeg 缓存完成构建，Gradle `BUILD SUCCESSFUL in 8m 2s`，安装器后处理、EXE 内嵌 MSI 哈希及便携内容核验通过；完整构建命令最终退出 0。最初 PATH 指向 pnpm 11.25.0 被构建门禁拒绝；随后因遗漏原 Cargo 缓存配置触发依赖获取，停止该次构建后使用既有缓存重建，原始日志保留。构建后的 Gradle 停止命令超时，仅清理本任务确认的 Gradle/Kotlin 进程后，构建会话退出 0。

r14 同包回归已正常结束，限定套件全部通过。包内 Core、CLI、Host 哈希与前候选不同，本节使用新包实际结果。完整 UI 和性能门槛仍未关闭，总结果为 `passed:false`，`release_ready:false`。未推送、发布或签名。

### f53019e 的 r14 同包结果

- [主验收](results/candidate-r14/acceptance.json)：运行中卸载返回 0，5 个 32 MiB 成品的[哈希](results/candidate-r14/output-hashes.json)均为 `e09320c5b00b34bb704802136c599a95b3996332ba84d7c7f21112b6231b6bd0`，与端点预期一致，卸载后保留。总结果未通过的原因是完整原生矩阵未完成，不能改判整体通过。
- [安装生命周期](results/candidate-r14/lifecycle/lifecycle.json)：从 f4b0f53b 同版本运行中升级、故障注入 1603 回滚、64 组件核验、32 MiB 续传及空闲卸载通过。原始失败保留。
- [完整便携包](results/candidate-r14/portable/portable.json)：真实数据库、未完成检查点及成品在升级/回滚后保持哈希，限定 `en-US`。另从新包提取助手和脚本做[活动下载补测](results/candidate-r14/portable-shutdown-followup.json)：文件锁失败拒绝交换并保存 1 MiB 检查点，运行中升级保存 2 MiB 检查点，重启续传 32 MiB 哈希正确，运行中回滚保留成品。补测使用最小应用目录，不冒充第二套完整便携 UI 验收。
- [免费功能](results/candidate-r14/free-features.json)整套 11 组、[HTTP 边界](results/candidate-r14/http-boundaries.json)13 项、[拆分恢复](results/candidate-r14/split-recovery.json)和[非录制媒体](results/candidate-r14/media.json)通过；后者含普通文件/HLS/DASH 成品与解码帧核对。实验拆分默认仍关闭。
- [Edge](results/candidate-r14/edge/edge.json)、[Firefox](results/candidate-r14/firefox/firefox.json)各 11 项及 [Edge 初始离线](results/candidate-r14/edge/initial-offline/edge.json)、[Firefox 初始离线](results/candidate-r14/firefox/initial-offline/firefox.json)通过。只在扩展重载/浏览器重启场景观察最长 90 秒持久恢复，普通清理仍观察 20 秒；每秒记录项目和持久交接状态。Firefox 本轮重载/重启清理实测 748/14 ms，未触及 90 秒上限，但不能覆盖 r13 的 20 秒超时或保证每次即时清理。未放松源文件保留检查。
- [原生输入](results/candidate-r14/native-finished.json)：普通 URL Return 创建并完成下载，空输入 Return 不创建，多行 cURL Return 只换行；真实批量两项完成；导出[8 项 JSON](results/candidate-r14/hls-downloader-tasks.json)，重导保持 8 项并提示重复。原有失败夹具按既有规则触发重试并报错，该步只作为去重证据；另选独立 URL 文本文件成功导入并完成任务 9。Ctrl+A 选中 9 项、Esc 清为 0；[并发值快照](results/candidate-r14/native-settings-persisted.json)为 2，重开设置及仅重启工作台后仍显示 2。这不等于本轮重启 Core。
- 原生 ZIP 中文路径显示 121 项、读取元数据 15,735 字节，列表滚动时固定底部按钮保持可达，Esc 关闭；浅色设置及深色列表/菜单未见所观察窗口中的圆角遮挡。拖动没有建立有效主窗口缩放结果，最小窗口不计通过。原生抓站完整流程、队列编辑/拖动、完整主题控件组合及 Windows OS DPI 仍未完成。
- [证据清单](results/candidate-r14/evidence-manifest.json)关联同包 MSI 和归档 JSON 的哈希。完整日志留在 `.tool-cache/free-parity-20261030/candidate-results-r14/`；r14 沙盒已关闭。

## 历史候选 f4b0f53b

源码 `f4b0f53b2da18ee704fde8763b0f169168f94004`，打包前工作区干净。完整候选构建已成功，包含普通 URL 框 Enter 修复。当前安装包及便携包位于 `artifacts/v7-productization/candidate/`。原始[构建来源](results/candidate-f4b0f53b/BUILD-PROVENANCE.json)、[交付哈希](results/candidate-f4b0f53b/ARTIFACT-MANIFEST.json)和[64 组件清单](results/candidate-f4b0f53b/expected-components.json)保留。

| 文件 | SHA-256 |
| --- | --- |
| EXE | `ec0020b959706f4ea556bf360656bcf48d10b1c57b45e551ab4464b61bfc24db` |
| MSI | `35cf261fa70fccd3a2b0eecf9338e85c722ffe7f7ba5e3bec8b6f3efad3d2b5b` |
| 便携 ZIP | `3209b358753a06a9ed71f83a6ae9da3284a039d4ec353e5c16f2f1c8ce982219` |

工作台 JAR SHA-256 `6c6e2ef831973b0ffcdad4a9e545d5ac1e671e8b9fa5341090a2c703e2689b49`。Core、CLI、Host 和 Presenter 与 69849b51 的哈希一致；重建的扩展 ZIP 已单独记录新哈希。

[r12 生命周期](results/candidate-r12/lifecycle/lifecycle.json)通过同版本运行中升级、注入 1603 回滚、64 组件核验、32 MiB 续传及空闲卸载，数据和成品保持哈希。随后沙盒在便携检查阶段中断，未写出便携终态和主套件结果；[中断记录](results/candidate-r12/interruption.json)保留，不能将未结束项目改判通过。r13 已完成同包定向回归，具体通过与失败见下节；完整 UI、浏览器重载连接及性能门槛仍未关闭。

另发现便携升级脚本强制停止 Core，已改为复用安装器原生停机助手，检查点或文件锁检查失败就拒绝目录替换。该修复尚未进入 f4b0f53b 候选；脚本定向验证和下一次候选包验证分别记录。

## f4b0f53b 的 r13 定向回归

[主验收](results/candidate-r13/acceptance.json)正常结束，总结果仍为 `passed:false`；运行中卸载返回 0，[成品哈希](results/candidate-r13/output-hashes.json)保持一致。Firefox 清理和完整原生矩阵仍未关闭。

- [免费功能](results/candidate-r13/free-features.json)整套 11 组正常结束并通过，包括已修正独立 URL 和队列恢复的 CLI 检查、ZIP、抓站限制、队列时间窗/请求上下文/自动周期组合及 Rust 定向测试。
- [HTTP 边界](results/candidate-r13/http-boundaries.json)和[非录制媒体](results/candidate-r13/media.json)通过，成品及解码帧哈希正确；[安装组件](results/candidate-r13/installed-critical-components.json)与本候选 64 项清单一致。
- [Edge](results/candidate-r13/edge/edge.json)11 项通过，包括扩展重载、浏览器重启及原身份导入；[Edge 离线](results/candidate-r13/edge/initial-offline/edge.json)和[Firefox 离线](results/candidate-r13/firefox/initial-offline/firefox.json)通过。
- [Firefox](results/candidate-r13/firefox/firefox.json)前 9 项及重载 ping 通过，随后 `Browser transfer released` 20 秒超时。重载后持久记录包含 `handoffId` 和 60 秒后的 `retryAt`；Native Host 轨迹显示启动期间 2.5 秒状态查询触发重试。成品哈希校验已通过，自动清理仍需覆盖完整重查周期后确认。原失败保留。
- [便携脚本补测](results/candidate-r13/portable-shutdown-followup.json)检查锁失败拒绝目录替换、运行中 Core 的升级/回滚及成品哈希，通过。使用真实候选 Core、最小应用目录和新脚本，不是修后完整便携包验收；此轮夹具未锁定响应未完成状态，不能据此宣称已验证活动下载检查点。后续夹具用事件保持响应未完成，并记录暂停字节。
- [原生阶段](results/candidate-r13/native-finished.json)未完成：工具窗口绑定为沙盒，连续截图却返回宿主浏览器，未向产品发送鼠标或键盘输入；请求前台协调后继续后台工作。不能据原生测试 API 的状态快照认定 Enter 或完整 UI 通过。

## 历史候选 69849b51 与 r11 验收

源码提交 `69849b516ec0eef6f6d426e46ef6f27454bfc353`，tree `8ad97ad9febb2d01ddbbfc209861da336f2ddee7`，打包前工作区干净。原始 [构建快照](results/candidate-69849b51/BUILD-PROVENANCE.json)、[交付哈希](results/candidate-69849b51/ARTIFACT-MANIFEST.json) 与 [64 组件清单](results/candidate-69849b51/expected-components.json) 保持不变。当前包目录已经由 f4b0f53b 替换，历史结果与来源不变。未签名、未发布。

| 文件 | SHA-256 |
| --- | --- |
| EXE | `6096486967d0c85b006d823018ac13410b786429b903bd491d66d6d88951433c` |
| MSI | `06efd23a16eb10c577edec48f5046a153e812b33777e7183a6bd1b22f4c905da` |
| 便携 ZIP | `9446dc91850dcff9964b60207d19a79d96b81f061b5c30d6e2fc4ea39695b090` |

- [r11 安装生命周期](results/candidate-r11/lifecycle/lifecycle.json)：从 c2a9bb6a 同版本升级、故障注入回滚、旧任务保留、64 组件核验、32 MiB 续传及空闲卸载通过，数据库和成品保持哈希。
- [r11 便携生命周期](results/candidate-r11/portable/portable.json)：真实数据库及未完成检查点在升级/回滚后保持哈希，限定 `en-US` 运行环境。
- [r11 主验收](results/candidate-r11/acceptance.json)：运行中卸载返回 0，[成品哈希](results/candidate-r11/output-hashes.json) 保留。总结果仍为 `passed:false`，不能据安装生命周期的通过结果宣称全计划完成。
- [HTTP 边界](results/candidate-r11/http-boundaries.json) 13 项和[非录制媒体](results/candidate-r11/media.json)通过；[原生输入](results/candidate-r11/native-finished.json)确认普通创建按钮、Esc、长错误 URL 的详情滚动及深色主题，完整 P1 矩阵未通过。普通 URL 框 Enter 不提交已复现，源码修复尚未进入此候选。
- [免费功能](results/candidate-r11/free-features.json)在前 9 组检查通过后，Python 进程在定时 faulthandler 诊断输出中以 `3221225477` 退出；根因未证实，整套不能计为通过。已移除重复线程栈计时器，保留父进程及命令超时。独立补测中又发现队列夹具复用导入任务 URL，按既有去重规则复用了已有任务；改为独立 URL，并在失败时恢复队列，待复核。
- [Edge](results/candidate-r11/edge/edge.json)在扩展重载后的文档等待超时；[Firefox](results/candidate-r11/firefox/firefox.json)在 NewSession 超时，均未达到整套通过。补测分别等待扩展 worker 就绪、延长首次建会话并保留浏览器日志。原始失败不覆盖。
- [活动拆分三轮 18 样本](results/candidate-r11/active-split.json)中普通、限速、慢尾段 off/on 中位秒数为 2.907/1.140、2.203/3.359、2.828/3.906。结果未证明稳定收益，实验继续默认关闭。[拆分恢复](results/candidate-r11/split-recovery.json)未等到助手连接；夹具改为事件同步并增加失败快照，待定向补测。
- [系统 DPI 限制观察](results/candidate-r11/os-dpi-observation.json)：沙盒“显示设置”打开系统首页但没有显示/缩放入口，尚未切换 Windows DPI；JVM 缩放不算 OS DPI。

### r11 定向补测与待打包修复

- [拆分恢复补测](results/candidate-r11/followup/split-recovery.json)通过，Engine SHA 与本候选一致：直连及系统代理路径均在活动拆分后暂停、保留持久检查点、恢复及校验完整输出。测试初始慢尾段现在用事件保持到助手出现，避免受负载影响提前完成。
- [免费功能补测](results/candidate-r11/followup/free-features.json)已正常结束，仍为失败。队列夹具的 URL 与前一检查导入任务相同，复用了已接收字节的任务；断言失败后队列未恢复，后续默认队列检查也因此失败。独立命名队列的关闭时间窗、页面/HEAD/文件请求上下文、60 秒自动周期同步及 Rust 定向测试通过。已修夹具 URL 和 `finally` 恢复，但尚未复测修后整套。
- [Firefox 补测](results/candidate-r11/followup/firefox/firefox.json)及 [Edge 补测](results/candidate-r11/followup/edge/edge.json)各完成前 9 项后，在重载后 ping 的 15 秒自动化脚本等待超时；[Firefox 初始离线](results/candidate-r11/followup/firefox/initial-offline/firefox.json)通过。重载 ping 已改为发送与状态轮询分开观察，允许覆盖 NativeBridge 的有界重试时间，待验证，不能据此认定产品已恢复连接。
- 普通 URL 框 Enter 修复提交 `d50d425`，`gradlew compileKotlin --offline` 实际编译通过（2m 1s）；保留多行 cURL 换行和忙碌/无效输入限制。候选 69849b51 不含此修复；新候选及真实输入回归尚待完成。
- [性能准备记录](results/candidate-r11/performance/performance.json)：竞品安装及首次启动执行，但默认连接设置未完成原生保存，等待标记到期退出；未产生性能样本，不作为性能通过证据。清理卸载返回 0。r11 任务沙盒已关闭，原始失败及日志留在项目工作目录。

## 旧候选 c2a9bb6a 的限定验收

产品源码提交 `c2a9bb6a7b58939bd457fdef57ed05acd9d8748e`，tree `95c258fae9b698dc66a084ba28d5c3770e96a0af`。后续验证脚本和报告提交不会改变此候选的源码来源。原始构建快照见 [BUILD-PROVENANCE](results/candidate-provenance/BUILD-PROVENANCE.json) 和 [ARTIFACT-MANIFEST](results/candidate-provenance/ARTIFACT-MANIFEST.json)。这是历史候选；当前包目录已由 69849b51 替换，旧 MSI 在项目内保留供升级验证。均未签名或发布。

| 文件 | SHA-256 |
| --- | --- |
| HLSDownloader-7.0.3-Windows-x64-candidate.exe | `5ce8feeb0f0df463f8aabd55337815e43d31eaaa8ccc14d5c1c398467849a9b8` |
| HLSDownloader-7.0.3-Windows-x64-candidate.msi | `84a314f9c91980d3a7739775dc71b4944201ed1a19587a886be2db3dc0fa0f98` |
| HLSDownloader-7.0.3-Windows-x64-Portable-candidate.zip | `fd016f2464c0c3b5cdd2a2240178641f47c7fc1430d34fb7218af573472af765` |

候选 Engine `162a9883bc7e3f87a280b59085b8062bb86b6c2f108bd31b88b139739ec3bb40`，CLI `9399d40d03ffb9e6b6b776ed7c4483ae0a36152be1a6e7cdc16d96180a52c04d`，Host `d9d494228d9729012a84f7579aae1e31a6614ee0610a1b82213b4b7f4aa5659c`。安装后核对 61 个关键组件；空 Skiko manifest 包的文件名变化单独核对内容，真正的 Skiko DLL 仍核对哈希。

| 范围 | 实际结果和证据 |
| --- | --- |
| 安装、升级、回滚 | [r1 lifecycle](results/candidate-r1/lifecycle/lifecycle.json)：旧包安装 0；注入 deferred failure 返回预期 1603，旧 ProductCode、组件和暂停任务恢复；旧工作台/Core 运行时升级 0，61 组件一致；32 MiB 续传 SHA 正确；空闲卸载 0，数据保留。 |
| 运行中卸载 | [r2 acceptance](results/candidate-r2/acceptance.json)、[r5 acceptance](results/candidate-r5/acceptance.json)：工作台/Core 运行中卸载 0，成品哈希保留。报告总 `passed:false` 保留未完成的原生矩阵，不影响这些限定检查的原始结果。 |
| 便携升级与回滚 | [r3 portable](results/candidate-r3/portable/portable.json)：真实数据库、未完成检查点及成品保留。限定使用 `CurrentUICulture=en-US`；沙盒中文 Archive 模块缺资源文件导致此前失败，未修改生产升级脚本绕过。 |
| 免费新功能 | [r2 free features](results/candidate-r2/free-features.json)：8 组，含 CLI 生命周期/队列/导入导出、ZIP、抓站、同步及 5 项 Rust 定向测试。 |
| HTTP 和拆分 | [r2 HTTP](results/candidate-r2/http-boundaries.json)、[split recovery](results/candidate-r2/split-recovery.json)：13 项边界及 raw HTTP/winhttp-proxy 活动拆分暂停续传，输出哈希正确。 |
| 非录制媒体 | [r4 media](results/candidate-r4/media.json)：普通文件、HLS、DASH 成品和解码帧哈希正确。 |
| 浏览器可靠性交接 | [r5 Edge](results/candidate-r5/edge/edge.json)、[r4 Firefox](results/candidate-r4/firefox/firefox.json)：各 11 项，含 GET/POST/Blob/redirect/HLS、拒绝保源、丢回执幂等、真实接收进度、扩展重载和浏览器重启；[r2 初始离线](results/candidate-r2/edge/initial-offline/edge.json)。逐入口查看原始 DownloadItem 记录。 |
| 原生操作 | [r2 native](results/candidate-r2/native-finished.json)、[r5 native](results/candidate-r5/native-finished.json)：真实输入下载与工具操作；[导出文件](results/candidate-r5/hls-downloader-tasks.json)、Core/设置快照及最终输出哈希保留。JVM 1/1.25/1.5/2 布局观察不是 OS DPI 验收。 |

### 拆分收益的限定结论

[三轮 18 样本](results/candidate-r2/active-split.json) 是 Core 对照，off/on 中位耗时分别为：普通 0.453/0.469 秒、单连接限速 1.281/1.297 秒、慢尾段 1.500/0.875 秒。只说明夹具中的慢尾段有收益；不支持普遍更快、全产品更省资源或已经超过 IDM/AB 的结论。默认仍关闭。

### 保留的候选失败及夹具修正

- r2 DASH 失败来自 FFmpeg 分片写入 CWD、MPD 却位于 fixtures，实际分片请求 404；修夹具 cwd 后 r4 通过，未改产品引擎。
- r2 浏览器进度夹具从普通 tab 内 popup.html 发请求，产品按规则拒绝 sender.tab；改为真实 `action.openPopup()` 后 Firefox r4、Edge r5 通过。r4 Edge 还曾因浏览器窗口未聚焦导致 openPopup 失败，修测试焦点后通过，未放松产品权限校验。
- [r5 性能准备失败](results/candidate-r5/performance/performance.json)：IDM 安装返回 0 时路径尚未出现；未产生性能样本。
- [r6 性能准备失败](results/candidate-r6/performance/performance.json)：IDM 可执行文件存在但仍被安装过程占用，CreateProcess 返回 WinError 32；清理当前候选时卸载返回 1603，MSI 显示 `V7StopRunningApplications` 约 30 秒超时。此失败重新打开 P0 阻塞；不能声称卸载已彻底修好。
- [r7 性能及卸载失败](results/candidate-r7/performance/performance.json)：默认配置未完成全部三轮，IDM 首次语言选择阻塞首轮、AB API 在第三轮超时；配置未落盘，未运行同连接上限对照。保留 [原始样本](results/candidate-r7/performance/default/comparison-clean.json)，不据此宣称完整性能比较通过。[助手日志](results/candidate-r7/performance/installer-diagnostics/Temp-HLSInstaller-876-1793363968866543300.log) 记录 `stage=spawn` 和 30 秒 `stage=timeout`，没有脚本 `shutdown stage=entry`；阻塞在 PowerShell 启动阶段，不能归因于 Core 检查点或文件锁。

### 原生安装助手修复的验证边界

安装助手改用 Win32 进程枚举、路径核验和窗口关闭，直接调用 Engine 的 `--shutdown` 保存检查点，不再生成或启动 PowerShell 停机脚本。进程终止限于目标安装目录的客户端；Core 检查点失败时不强杀 Core，独占文件锁失败时保留原安装和用户数据。`shutdown-running.ps1` 仍有便携升级等调用，保留该脚本。

回归入口为 `scripts/validation/installer_preflight_acceptance.py`，包含安装目录边界、版本 Host、锁失败、检查点失败保 Core、其他安装管道拒绝及失败后重试；进程夹具只能在 Sandbox 中运行。诊断 MSI 仅替换 Binary 表助手并刷新 PackageCode，不能冒充上表中的正式候选。新候选已构建，见前述 69849b51 节；其余同包矩阵继续进行。

- r8 诊断 MSI SHA-256 `c95546703be6839afe58846f6d04baa8c776882f5e97b0ac6484a3987590443d`：[脚本报告](results/diagnostic-native-r8/native-msi.json) 在 240 秒超时，[完整 MSI 日志](results/diagnostic-native-r8/cold-uninstall.log) 随后记录卸载成功、`MainEngineThread is returning 0`。助手步骤约 1 秒，不能把整个脚本改判为通过。后续 MSI 日志先写沙盒本地再归档，避免共享目录逐行写入干扰。
- r9 诊断 MSI SHA-256 `ceb7b99a690cc57edf49b641a701e8d9ffcf748e317442fbc8150507cb644aa4`：[8 组助手边界通过](results/diagnostic-native-r9/helper/result.json)，[全新安装及冷卸载返回 0](results/diagnostic-native-r9/native-msi.json)。真实 Core 检查点返回 0 后，退出中的工作台路径查询短暂拒绝访问，助手误判失败；[原日志](results/diagnostic-native-r9/Temp-HLSInstaller-2544-1793366012065797400.log) 保留。已增加退出信号短暂等待及最终停机轮询重试，r10 已复核通过。

- r10 诊断 MSI SHA-256 `06818aadaae4b3082fd8583f6ee05fed68055131992cd7a61af2566f7c71ce9b`：[全部限定检查通过](results/diagnostic-native-r10/native-msi.json)，含冷安装/冷卸载、再次安装、真实 Core 检查点、32 MiB 续传 SHA-256、运行中卸载及数据库/成品保留；[8 组边界](results/diagnostic-native-r10/helper/result.json) 通过。受测助手 SHA-256 `381946f0eca9b3a5c0764e441b94b500f2c7ad7840fec8bcecc1c63b5e90bc3a`。这是修复版诊断包证据，69849b51 已重新构建且 r11 升级/回滚通过，其余同包矩阵继续进行。

抓站组合验收已加入 `scripts/validation/free_parity_acceptance.py`，页面数/深度边界及定时/上下文组合的实际结果见 r11 与定向补测；不得将限定检查通过换算为整套通过。

完整原始 MSI 日志、浏览器配置、夹具及诊断保留在项目内 `.tool-cache/free-parity-20261030/candidate-results-r*/`；选定可审查 JSON 入库。没有删除或改写原始失败。

## 免费功能定向验收

脚本为 `scripts/validation/free_parity_acceptance.py`，只允许在 Windows Sandbox 中执行；禁外部网络，两个 loopback HTTP 端点提供全部测试数据。宿主仅编译。结果见 [第六轮开发组件结果](results/component-results-r6/acceptance.json)。这不是完整候选包的验收结果，后续链接/离线引用改写修正仍需在新候选复核。

| 实际通过范围 | 证据 |
| --- | --- |
| CLI 启动与单实例 | 无工作台，CLI 启动唯一 Core，多次调用保持一个进程 |
| CLI 生命周期 | 原子暂停创建、中文空格目录/文件名、暂停/重启/恢复、8 MiB SHA-256、导入/导出、不可覆盖导出、退出码、删除任务保留文件 |
| ZIP | 中文名称、空包、ZIP64、损坏拒绝；8,388,843 字节远程归档只读 65,577 字节；2200 项中央目录；身份变化拒绝；非 Range 拒绝 |
| 抓站与离线 | 两页循环去重、4 个资源、确认前无下载任务、暂停后重启恢复、HTML/CSS 本地引用改写 |
| 同源边界 | 302 跳往第二 origin 前被拒绝，对端请求数为 0 |
| 同步 | 相同 ETag 时文件 GET 数增量 0；503 更新失败保留旧字节；重试后新字节完整原子替换 |
| Rust 定向测试 | 5 项在沙盒通过：抓站 URL/静态引用、ZIP64 目录、schema 6→7 保旧任务与设置、活动拆分安全区 |

开发 Core SHA-256 为 `c2b1b0a4e945f49a7307be0c95b4282d2991ab84c23ab0e9ce1c17f64a2aa1a6`，CLI 为 `6f0495f29bc9dd7c0f8f749035508cb7782491b57c07f2c373aec02e73730213`。完整包将使用优化构建，不能沿用这些组件结果声称同包通过。

保留的失败包括：

- 第一轮缺少开发组件运行库，返回 `0xC0000135`，功能未执行。
- 第四轮 Python 堆栈与进程证据表明 CLI 已退出，但 Core 继承了捕获管道，使调用者等不到 EOF。CLI 启动 Core 前清除原标准句柄的继承标记，第五、六轮捕获调用正常结束。
- 第五轮同步发布因 Windows 只读句柄无法 `FlushFileBuffers` 而失败；改用可写句柄后第六轮通过。抓站夹具当时误写 5 个资源，实际应为 4，原失败保留。

## 当前交付边界

2026-10-31 02:25，r15 同包性能初始化被 Windows 计划关机提示遮挡；重新激活沙盒后提示仍在，已停止原生输入并请用户处理。[中断记录](results/candidate-r15/interruption.json) 保留实际进度：候选安装和 64 组件核验完成，IDM 安装返回 0，AB 配置文件已生成，但 IDM 默认连接设置尚未落盘。未创建开始采样标记，未产生本轮性能样本，默认配置与相同连接上限的三轮对照仍未完成。原始材料保留于项目内 `.tool-cache/free-parity-20261030/candidate-results-r15/performance/`。

功能清单保留逐项状态和限定证据，移除了整体“完成率”百分比；`release_ready` 继续为 `false`。新功能均暂列 `partial`，直到当前候选的必要验证完成。没有产品账号、激活或收费服务新增。使用说明见 [免费版指南](../../free-edition.md)。未推送、发布或签名。
