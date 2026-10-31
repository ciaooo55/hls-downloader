# 免费版主流程对标实施记录

用户已确认实施 Windows x64 免费版计划。保留网站 Cookie/请求头，不新增账号、会员、激活、企业认证或收费 API 前置。录制、多平台、驱动级接管、真实投屏/TVBox 和收费云字幕验收不在本轮范围。不得推送、发布或签名。产品和竞品只在 Windows Sandbox 中运行。

## 阶段

| 阶段 | 当前状态 |
| --- | --- |
| P0 安装器与证据 | bafddff6 候选 r21 同版本运行中升级、故障回滚、64 组件、续传、空闲及运行中卸载通过；完整便携包升级/回滚和活动检查点通过。r24 运行中卸载返回 0，6 个成品保持哈希。历史 1603 原样保留。 |
| P1 UI 与原生操作 | bafddff6 的 r21 固定抓站创建、确认、浅深主题和设置保存/Core 读回/工作台重启通过限定检查。r24 在 1024×600 逻辑窗口、实际 125% OS 比例完成新建 Return、设置滚动、抓站固定底栏、中文 ZIP 选择/预览、批量、导入/导出、列表全选/Esc 和队列编辑/重开。用户人工慢拖确认窗口可缩小，回复未绑定包哈希。完整 OS DPI、同包任务拖动、强制最小尺寸边缘拖动及完整主题/控件组合仍开放。 |
| P2 浏览器交接 | bafddff6 的 r21 Firefox、r23 Edge 各 11 项及 r21 两浏览器初始离线通过。r21 Edge 重载等待失败和 r22 媒体夹具缺失失败保留，不能由补测成功认定每次无延迟。DownloadItem 按入口记录。 |
| P3 分段与恢复 | bafddff6 的 r21 HTTP 13 项、普通文件/HLS/DASH 成品及解码帧、直连/系统代理活动拆分恢复通过。活动拆分收益不稳定，默认关闭；r16 的 36 个全产品性能样本属于旧 f53019e 包，仍显示性能差距，不冒充新包测量。 |
| P4 CLI | 已进入安装与便携包。bafddff6 的 r21 免费整套 11 组通过，含单 Core、中文路径、生命周期、重启续传、队列启停、导入导出、退出码及显式删文件。 |
| P5 抓站与同步 | bafddff6 的 r21 整套通过：循环去重、预览确认、恢复、静态改写、跨域边界、页面/深度限制、ETag/失败保旧/原子替换、队列时间窗、请求上下文及自动周期。r21 原生确认 3 项完成，r24 最小窗口固定底栏可达；完整原生组合矩阵未完成。 |
| P6 ZIP 预览 | bafddff6 的 r21 本地/远程、中文、空包、ZIP64、损坏拒绝、资源变化、非 Range 拒绝、有界读取及条目上限通过；r24 同包原生中文选择、121 项目录、滚动/固定底栏/Esc 通过。完整主题/OS 缩放组合未完成。 |

## P0 首轮诊断边界

工作目录为 `.tool-cache/free-parity-20261030/`。`results/` 首轮使用自定义管道，而 MSI 助手未继承该环境，不能作为生产默认入口的失败复现；旧 MSI 返回 1603，直接助手随后成功，结果原样保留。

`matrix-results/` 改用生产默认管道：旧 MSI 安装/运行中卸载返回 0；诊断 MSI 安装/运行中卸载返回 0。日志显示 checkpoint 命令返回 0。诊断 MSI 仅替换安装助手用于定位问题，不是完成本计划的新候选，不回写旧包来源。

后续同包通过和新增失败见下节。旧候选的 1603 失败保留，不能用多次成功覆盖新的失败。

## 新候选 bafddff6：r21 回归、r23 Edge 及 r24 原生补测完成，完整 UI 门槛仍开放

源码 `bafddff6b2d185be08a9f3522d952bd347f5c285` 包含固定抓站创建按钮修复 `948f6595`。新包保存在 `artifacts/v7-productization/candidate-bafddff6/`，原 f53019e 候选目录完整保留。[构建来源](results/candidate-bafddff6/BUILD-PROVENANCE.json)、[交付清单](results/candidate-bafddff6/ARTIFACT-MANIFEST.json)、[64 组件清单](results/candidate-bafddff6/expected-components.json)及[构建结果](results/candidate-bafddff6/build-result.json)已归档。

| 文件 | SHA-256 |
| --- | --- |
| EXE | `77c62529b507f91c318f5ea3b6656073d383a8263ed0fd336a15f6a463015ec7` |
| MSI | `78105372fb19e85aedca2588518db285418d1f162fa2a4cb31ef27fe7d13ecd9` |
| 便携 ZIP | `247306440fdc7c1e4584e8b86e57d7f838a6b42a14238257ce43b0c61faaba11` |

首次构建因遗漏现有 FFmpeg 缓存路径在依赖检查处停止。补入已核对哈希的缓存后，Gradle 编译与打包成功（5 分 53 秒），安装器后处理和 EXE 内嵌 MSI 核验通过；但最后候选目录交换被仍在运行的 r20 沙盒目录映射锁住，完整构建命令返回 1。保留该失败，直接从已生成输出恢复并核对 EXE/MSI，使用原有 `create-v7-portable.ps1` 重新生成便携包，未重编译或改写构建来源。重新核对源码、工作区、64 组件、Skiko 包装及两浏览器扩展。新生产 JAR 不含临时鼠标事件诊断。

r21 新候选同包验收已结束；[主结果](results/candidate-r21/acceptance.json)保留 `passed:false`，原因是 Edge 首轮重载等待失败和完整原生矩阵未完成。原始结果不被后续成功覆盖。[证据清单](results/candidate-r21/evidence-manifest.json)绑定本包 MSI、64 个安装组件和验收材料。旧包 r14–r18 证据不替代新包验收。

- [安装生命周期](results/candidate-r21/lifecycle/lifecycle.json)：从 f53019e 同版本运行中升级返回 0，故障注入预期 1603 后旧任务保留，恢复续传成功，空闲卸载返回 0。[完整便携包](results/candidate-r21/portable/portable.json)升级/回滚保留数据；[活动检查点补测](results/candidate-r21/portable-shutdown-followup.json)在文件锁失败时拒绝交换，保存 1/2 MiB 检查点，续传 32 MiB 哈希正确。便携文化环境仍限定 en-US。
- [免费功能](results/candidate-r21/free-features.json)整套 11 组、[HTTP](results/candidate-r21/http-boundaries.json)13 项、[拆分恢复](results/candidate-r21/split-recovery.json)及[非录制媒体](results/candidate-r21/media.json)通过。活动拆分保持默认关闭；本轮不重新声称性能领先，也没有用 r16 旧包性能样本冒充本包测量。
- [Firefox](results/candidate-r21/firefox/firefox.json)11 项及两浏览器[Firefox 离线](results/candidate-r21/firefox/initial-offline/firefox.json)、[Edge 离线](results/candidate-r21/edge/initial-offline/edge.json)通过。[Edge 首轮](results/candidate-r21/edge/edge.json)完成前 9 项，在重载后弹窗运行环境就绪处超时。原夹具只等待任意扩展 worker，可能把尚未退出的旧实例当作重载完成；该轮没有记录返回 worker 身份，不能把此推断当成已证实的产品根因。
- 修正夹具在重载前记录 worker 身份，并要求新 worker ID，记录弹窗 URL、文档状态及 runtime ID；交接、单请求、源文件保留、任务去重与成品哈希断言不变。[r22 补测](results/candidate-r22/acceptance.json)因漏准备媒体夹具，Node 在读取不存在的 video.m3u8 时退出，尚未到重载阶段；该测试准备错误原样保留。补入仅媒体夹具生成后，[r23 Edge](results/candidate-r23/edge/edge.json)11 项全部通过，确认 worker 已替换、弹窗 runtime ID 可用，扩展及浏览器重启后原交接恢复且无重复任务。[r23 卸载](results/candidate-r23/acceptance.json)返回 0；[证据清单](results/candidate-r23/evidence-manifest.json)绑定相同候选组件。该结果证明修后夹具在本次运行通过，不保证每次无延迟。
- [r21 原生检查](results/candidate-r21/native-finished.json)实际完成浅色抓站固定底栏空值禁用、有效地址启用、滚动固定和按钮创建，预览访问 2 页、发现 3 资源，确认前 5 个任务、确认后 8 个任务，新增三项全部完成。浅深主题观察到的弹窗、输入框、开关、列表和菜单未见圆角遮挡；深色创建底栏保持可达。Tab 转移焦点、Esc 关闭通过。并发设置从 1 改为 2，保存后 Core 读回、工作台重启后读回及设置重开显示一致，见[重启后设置](results/candidate-r21/native-settings-after-ui-restart.json)。这是工作台重启，不是 Core 重启。
- r21 运行中卸载返回 0，[4 个成品](results/candidate-r21/output-hashes.json)与卸载前哈希一致。沙盒已关闭；完整原始日志留在项目工作目录。r23 补测沙盒也已关闭。

用户随后直接回复“已拖动，窗口变小”，见[人工确认](results/manual-resize-user-confirmation.json)。此回复未绑定包哈希，不提升为完整同包最小尺寸或 OS DPI 通过；也不再把自动拖动的零位移当作已证实的产品缩放故障。r24 已完成下述同包最小窗口定向检查，OS 比例保持实际 125%，没有强制 JVM 比例冒充系统 DPI。完整矩阵和 `release_ready` 仍未通过。

### bafddff6 的 r24 最小窗口与原生流程补测

[主结果](results/candidate-r24/acceptance.json)和[原生结果](results/candidate-r24/native-finished.json)已归档，共 21 项限定检查通过，完整矩阵仍为 `passed:false`。安装后 64 组件与候选清单一致。窗口以 1024×600 逻辑尺寸启动；初始沙盒工作区较小，框架报告最小高度夹为 567，随后最大化沙盒外框以显示产品全窗口，见[窗口几何](results/candidate-r24/native-window-after-maximized-sandbox.json)。这验证该尺寸下的布局，不冒充拖到强制最小尺寸的验收。

- 浅色新建弹窗空值禁用、有效地址启用，显式聚焦 URL 输入框后 Return 创建 task-6 并完成 32 MiB。设置导航与滚动有效，保存栏固定可达；Esc 关闭。抓站长表单可以滚到底部同源/静态离线开关，创建按钮保持固定可见。
- 深色 ZIP 经真实文件选择器选择中文路径，显示中文条目和 121 项目录、15,735 字节元数据；列表滚动时底部按钮可达，Esc 关闭。所观察的浅深列表、输入框、开关、菜单及弹窗未见圆角遮挡，不外推至全部控件组合。
- [批量](results/candidate-r24/native-batch-completed.json)新增 task-7/8，均完成；列表聚焦后 [Ctrl+A 全选 8 项](results/candidate-r24/native-select-all.json)，[Esc 清为 0](results/candidate-r24/native-selection-cleared.json)。真实保存选择器导出[8 项 JSON](results/candidate-r24/native-tasks.json)，独立 URL 文件导入[新增 task-9/10 并完成](results/candidate-r24/native-import-completed.json)。
- 队列 01 并发从 1 改为 2、移至队列 02 后，保存后 [Core 读回](results/candidate-r24/native-queue-saved.json)和弹窗重开一致。本轮没有重启 Core，不能据此声称全部设置重启持久化通过。
- 一次自动任务拖动使列表滚动并显示预置资源错误提示，未确认顺序改变；[Core 记录](results/candidate-r24/native-drag-attempt.json)及 [UI 记录](results/candidate-r24/native-drag-ui-state.json)保留，不计通过，也未证实为产品拖动故障。r18 成功属于旧候选，不能替代本包。
- 键盘首次发给沙盒内已打开的记事本卡片；最小化卡片并显式聚焦产品后 Return 正常。部分 `type_text` 先粘贴旧剪贴板，核对并改正后才保存。这些自动化限制记录于原生结果，没有归为产品错误。
- 运行中卸载返回 0，[6 个成品](results/candidate-r24/output-hashes.json)保持预期 SHA-256，其中 5 个由本轮原生创建，另 1 个为协议准备夹具。[证据清单](results/candidate-r24/evidence-manifest.json)绑定包和材料；[任务沙盒已关闭](results/candidate-r24/sandbox-cleanup.json)，完整原始日志留在项目工作目录。未重新构建或测量本包性能。

[r20 临时诊断](results/candidate-r20/acceptance.json)已结束：自动拖动按下事件被处理时，全局鼠标已到终点，框架读到零位移，见[事件记录](results/candidate-r20/resize-automated-burst.json)。这只能说明该自动输入时序无效，不能认定真人缩放失效，因此不据此重写产品缩放逻辑。r20 当轮未执行人工拖动；用户后续确认见上文。按用户要求继续自主验收，不再等待用户操作。卸载返回 0、成品保留，沙盒已关闭；[证据清单](results/candidate-r20/evidence-manifest.json)保留开发覆盖范围和原始结果。

## 旧候选 f53019e

源码 `f53019e0968c2d919d4c1ace2aa099f18679c678`，打包前工作区干净，包含便携升级检查点修复 `425ab01`。新包位于 `artifacts/v7-productization/candidate/`；原始[构建来源](results/candidate-f53019e/BUILD-PROVENANCE.json)、[交付清单](results/candidate-f53019e/ARTIFACT-MANIFEST.json)与[64 组件及便携助手清单](results/candidate-f53019e/expected-components.json)已保存。

| 文件 | SHA-256 |
| --- | --- |
| EXE | `fba69d349d85991476009069754dc6232948c4daa60c33c6901d08595bc23ec9` |
| MSI | `c46b241618a1b05bb412f367ffb05b31e7c0d1ce4c2dc41d9880db4da5947734` |
| 便携 ZIP | `da1e067d959faca049674c69f19ef8e331c1f74ac7cb97d446657807dda604c9` |

固定 pnpm 11.7.0、现有 JDK 21、Cargo 和 FFmpeg 缓存完成构建，Gradle `BUILD SUCCESSFUL in 8m 2s`，安装器后处理、EXE 内嵌 MSI 哈希及便携内容核验通过；完整构建命令最终退出 0。最初 PATH 指向 pnpm 11.25.0 被构建门禁拒绝；随后因遗漏原 Cargo 缓存配置触发依赖获取，停止该次构建后使用既有缓存重建，原始日志保留。构建后的 Gradle 停止命令超时，仅清理本任务确认的 Gradle/Kotlin 进程后，构建会话退出 0。

r14 同包回归已正常结束，限定套件全部通过。包内 Core、CLI、Host 哈希与前候选不同，本节使用新包实际结果。r16 已完成性能对照，完整 UI 门槛仍未关闭；r14 总结果仍为 `passed:false`，`release_ready:false`。未推送、发布或签名。

### f53019e 的 r14 同包结果

- [主验收](results/candidate-r14/acceptance.json)：运行中卸载返回 0，5 个 32 MiB 成品的[哈希](results/candidate-r14/output-hashes.json)均为 `e09320c5b00b34bb704802136c599a95b3996332ba84d7c7f21112b6231b6bd0`，与端点预期一致，卸载后保留。总结果未通过的原因是完整原生矩阵未完成，不能改判整体通过。
- [安装生命周期](results/candidate-r14/lifecycle/lifecycle.json)：从 f4b0f53b 同版本运行中升级、故障注入 1603 回滚、64 组件核验、32 MiB 续传及空闲卸载通过。原始失败保留。
- [完整便携包](results/candidate-r14/portable/portable.json)：真实数据库、未完成检查点及成品在升级/回滚后保持哈希，限定 `en-US`。另从新包提取助手和脚本做[活动下载补测](results/candidate-r14/portable-shutdown-followup.json)：文件锁失败拒绝交换并保存 1 MiB 检查点，运行中升级保存 2 MiB 检查点，重启续传 32 MiB 哈希正确，运行中回滚保留成品。补测使用最小应用目录，不冒充第二套完整便携 UI 验收。
- [免费功能](results/candidate-r14/free-features.json)整套 11 组、[HTTP 边界](results/candidate-r14/http-boundaries.json)13 项、[拆分恢复](results/candidate-r14/split-recovery.json)和[非录制媒体](results/candidate-r14/media.json)通过；后者含普通文件/HLS/DASH 成品与解码帧核对。实验拆分默认仍关闭。
- [Edge](results/candidate-r14/edge/edge.json)、[Firefox](results/candidate-r14/firefox/firefox.json)各 11 项及 [Edge 初始离线](results/candidate-r14/edge/initial-offline/edge.json)、[Firefox 初始离线](results/candidate-r14/firefox/initial-offline/firefox.json)通过。只在扩展重载/浏览器重启场景观察最长 90 秒持久恢复，普通清理仍观察 20 秒；每秒记录项目和持久交接状态。Firefox 本轮重载/重启清理实测 748/14 ms，未触及 90 秒上限，但不能覆盖 r13 的 20 秒超时或保证每次即时清理。未放松源文件保留检查。
- [原生输入](results/candidate-r14/native-finished.json)：普通 URL Return 创建并完成下载，空输入 Return 不创建，多行 cURL Return 只换行；真实批量两项完成；导出[8 项 JSON](results/candidate-r14/hls-downloader-tasks.json)，重导保持 8 项并提示重复。原有失败夹具按既有规则触发重试并报错，该步只作为去重证据；另选独立 URL 文本文件成功导入并完成任务 9。Ctrl+A 选中 9 项、Esc 清为 0；[并发值快照](results/candidate-r14/native-settings-persisted.json)为 2，重开设置及仅重启工作台后仍显示 2。这不等于本轮重启 Core。
- 原生 ZIP 中文路径显示 121 项、读取元数据 15,735 字节，列表滚动时固定底部按钮保持可达，Esc 关闭；浅色设置及深色列表/菜单未见所观察窗口中的圆角遮挡。拖动没有建立有效主窗口缩放结果，最小窗口不计通过。原生抓站完整流程、队列编辑/拖动、完整主题控件组合及 Windows OS DPI 仍未完成。
- [证据清单](results/candidate-r14/evidence-manifest.json)关联同包 MSI 和归档 JSON 的哈希。完整日志留在 `.tool-cache/free-parity-20261030/candidate-results-r14/`；r14 沙盒已关闭。

### f53019e 的 r16 性能对照

r15 的沙盒进程在关机中断后已退出，未产生样本；原报告保留。r16 使用新的沙盒与输出目录，候选安装及 64 组件核验通过，默认配置和相同连接上限各三轮、轮换顺序，共 36 个样本全部完成且 SHA-256 正确，无测试错误；清理时卸载返回 0。见[运行结果](results/candidate-r16/performance/performance.json)、[默认全部样本](results/candidate-r16/performance/default/comparison-clean.json)、[同连接全部样本](results/candidate-r16/performance/matched/comparison-clean.json)及[资源汇总](results/candidate-r16/performance/summary.json)。

AB 与 IDM 默认连接数通过真实界面观察并保存后分别读回 `threadCount=8`、`MaxConnectionsNumber=8`；同连接组逐产品保存并读回上限 4，详见[初始化证据](results/candidate-r16/performance/native-setup.json)。本项目默认设置由候选包使用；同连接组显式设为 4。活动慢尾段拆分保持默认关闭。

下表为各配置各产品三轮的耗时中位数，单位秒；快速端点 128 MiB，限速端点 64 MiB、每连接 4 MiB/s 和 80 ms 延迟，均在完整成品 SHA-256 正确后停止计时。

| 配置 / 场景 | 本项目 | AB | IDM |
| --- | ---: | ---: | ---: |
| 默认 / 快速 | 2.498 | 4.019 | 2.048 |
| 默认 / 限速 | 5.207 | 2.802 | 3.896 |
| 上限 4 / 快速 | 2.463 | 3.634 | 2.174 |
| 上限 4 / 限速 | 6.650 | 4.680 | 5.126 |

同连接上限 4、限速场景的全产品采样私有内存峰值中位数为本项目 245.9 MiB、AB 313.0 MiB、IDM 21.0 MiB；CPU 时间中位数分别为 2.734、6.078、0.547 秒。本项目在这些夹具中比 AB 使用更少 CPU/内存，但限速传输更慢，CPU/内存也明显高于 IDM，不能认定普遍领先。全部样本、范围和采样指标保留于汇总与原始结果。

冷启动统计按 HLS/AB IPC 可用、IDM 进程可见判断，并不使用相同的 UI 就绪条件，不能用其数值比较用户可操作速度；完整 UI 响应和公网场景未纳入本轮。没有新增网络依赖，沙盒外部网络禁用。r16 沙盒在归档后已关闭。

### f53019e 的 r17 原生补测

[本轮结果](results/candidate-r17/acceptance.json)正常结束，完整原生矩阵仍为 `passed:false`。实际完成：队列 01 并发从 1 改为 2、移至队列 02 后，保存后 Core 读回与弹窗重开一致；仅重启工作台后 Core 设置保持，见[重启后设置](results/candidate-r17/native-queue-after-ui-restart.json)。这不代表重启 Core。

抓站从真实输入创建同源、深度 2、500 页、并发 2、静态离线项目，发现 2 页与 3 个资源；[确认前快照](results/candidate-r17/native-crawl-before-confirm.json)仍为 5 个任务。只勾选 image.bin 后，仅新增一个任务并完成，[成品 SHA-256](results/candidate-r17/output-hashes.json)与夹具一致。随后确认全部发现时，两个 HTML 被 `RANGE_UNSUPPORTED` 拒绝：原生夹具 HEAD 继承了 32 MiB 二进制响应头，GET 却返回短 HTML。这是已定位的夹具不一致，本轮不计静态页面保存通过；失败证据保留。

边缘拖动未观察到有效缩放，不能计最小窗口通过；Compose 1.12 自带边缘缩放实现，原因仍需定位。完整 Windows DPI、主题控件及任务拖动尚未关闭。运行中卸载返回 0，两个成品卸载后保持哈希。[证据清单](results/candidate-r17/evidence-manifest.json)关联同包 64 组件及原生报告；原始日志留在 `.tool-cache/free-parity-20261030/candidate-results-r17/`，沙盒已关闭。

### f53019e 的 r18 原生抓站与拖动补测

[本轮结果](results/candidate-r18/acceptance.json)正常结束，仍保留完整矩阵 `passed:false`。修正原生夹具 HEAD/GET 长度与 Range 一致性，先通过[HTTP 夹具自检](results/candidate-r18/fixture-http-contract.json)，再实际创建静态离线项目、预览 2 页与 3 个资源、确认全部下载。确认前为 5 个任务，确认后仅增加 3 个；两页 HTML 和 32 MiB 资源全部完成。

实际点击暂停、继续及立即检查更新；[更新前](results/candidate-r18/native-crawl-saved-before-sync.json)与[更新后](results/candidate-r18/native-crawl-after-sync.json)均为 8 个任务，没有重复新增。[保存的 HTML](results/candidate-r18/saved-html.json)中页面互链、片段和图片引用均已改成本地名称，[引用检查](results/candidate-r18/static-offline-check.json)确认目标存在且内容哈希与卸载后成品一致。运行中卸载返回 0，四个成品保持哈希。

真实拖动任务 1 到任务 3 后，Core 与列表顺序均成为 2、3、1，见[拖动快照](results/candidate-r18/native-task-drag.json)。最大化/还原实际生效；边缘拖动仍未改变 1214×600 尺寸。只读窗口数据确认当前 OS 比例 125%；沙盒显示设置入口打开系统首页，没有显示比例选择，100%/150%/200% 未执行，完整 DPI 与最小窗口验收仍开放。创建按钮需滚到长表单底部才可见，随后源码已将它移至固定底栏，须单独验证新 UI。

[证据清单](results/candidate-r18/evidence-manifest.json)保留同包组件哈希及[原生夹具源码](results/candidate-r18/runner.py)。该轮沙盒已关闭，原始日志留在 `.tool-cache/free-parity-20261030/candidate-results-r18/`。

### 固定抓站创建操作的 r19 开发 UI 验证

抓站表单的“创建并发现”从滚动内容移到共享弹窗固定底栏，沿用原有校验及创建逻辑。离线 `compileKotlin` 与 `jar` 均实际成功；[开发来源](results/candidate-r19/development-ui.json)记录基线、差异和 JAR 哈希，[差异](results/candidate-r19/development-ui.patch)保留验证时源码。此次在 f53019e 安装后覆盖开发 UI JAR，**不是重建候选包，也不构成同包发布验收**。

[真实输入结果](results/candidate-r19/native-finished.json)：空地址禁用、粘贴本地地址启用、滚动后按钮仍可见，点击固定底栏成功创建项目，界面显示访问 2 页、发现 1 个文件资源；未确认下载时任务仍为 5 个。浅深主题中底栏可达，Esc 可关闭弹窗。只读诊断确认浮动窗口可缩放、无原生装饰、最小尺寸 1024×600，但拖边仍未产生尺寸变化，不能计最小窗口通过。

[本轮结果](results/candidate-r19/acceptance.json)记录运行中卸载返回 0，成品卸载后保持哈希；完整原生矩阵仍为 `passed:false`。OS DPI、有效边缘缩放和完整主题控件组合继续开放。[证据清单](results/candidate-r19/evidence-manifest.json)关联原安装组件与开发 UI，沙盒已关闭，原始日志留在 `.tool-cache/free-parity-20261030/candidate-results-r19/`。

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

当前交付包为 bafddff6，r21 回归、r23 Edge 及 r24 最小窗口限定验收已结束。完整 OS DPI 100%/150%/200%、完整主题/控件组合、同包任务拖动及强制最小尺寸边缘拖动仍未关闭。当前沙盒无法提供此前查找的显示比例选择入口，不用 JVM 比例替代。实验活动拆分继续默认关闭；性能未证明普遍超过竞品，新包也没有重测。

历史 r15 在 2026-10-31 02:25 被 Windows 计划关机提示中断，[记录](results/candidate-r15/interruption.json)保留，未产生样本；随后 r16 已完成旧 f53019e 包的三轮对照，详见上文。r15 原始材料留在 `.tool-cache/free-parity-20261030/candidate-results-r15/performance/`，不再作为当前正在等待的任务。

功能清单保留逐项状态和限定证据，移除了整体“完成率”百分比；`release_ready` 继续为 `false`。新功能均暂列 `partial`，直到当前候选的必要验证完成。没有产品账号、激活或收费服务新增。使用说明见 [免费版指南](../../free-edition.md)。未推送、发布或签名。
