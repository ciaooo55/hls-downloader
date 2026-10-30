# 免费版主流程对标实施记录

用户已确认实施 Windows x64 免费版计划。保留网站 Cookie/请求头，不新增账号、会员、激活、企业认证或收费 API 前置。录制、多平台、驱动级接管、真实投屏/TVBox 和收费云字幕验收不在本轮范围。不得推送、发布或签名。产品和竞品只在 Windows Sandbox 中运行。

## 阶段

| 阶段 | 当前状态 |
| --- | --- |
| P0 安装器与证据 | 当前候选安装、运行中升级、故障回滚、暂停任务恢复及空闲/运行中卸载有通过证据；r6、r7 清理中再次卸载 1603。r7 已定位到 PowerShell 子进程创建后未进入脚本。直接 Win32 助手在 r10 诊断 MSI 完成冷安装/卸载、真实检查点恢复、运行中卸载与数据保留；尚未进入正式新候选，完整交付门槛未关闭。 |
| P1 UI 与原生操作 | 当前候选真实输入完成普通、批量、导入下载，导出、多选、Esc、详情、队列编辑、Tab、拖动及重启持久化；新 ZIP/抓站对话框原生操作通过。完整 OS DPI、最小窗口、长文本及错误状态矩阵未完成。 |
| P2 浏览器交接 | 同包 Edge r5、Firefox r4 各 11 项通过，Host 初始离线保全另有 r2 证据；DownloadItem 创建按入口记录，严格零创建未通过且不作为可靠性交接的替代条件。 |
| P3 分段与恢复 | HTTP 13 项边界、拆分暂停恢复通过；三轮 18 样本 Core 对照显示慢尾段收益，普通/限速场景没有收益。实验开关 `HLS_HTTP_ACTIVE_SPLIT=1` 默认关闭；全产品竞品对照仍待完成。 |
| P4 CLI | 当前候选无工作台单 Core、中文空格路径、生命周期、重启续传、导入导出、队列启停、退出码、默认留文件及显式删文件通过。CLI 已进入安装与便携包。 |
| P5 抓站与同步 | 当前候选循环去重、确认前无任务、暂停重启恢复、静态离线改写、跨域跳转对端零请求、ETag 无重复 GET、更新失败保旧及重试原子替换通过；限额/定时组合完整矩阵未完成。 |
| P6 ZIP 预览 | 当前候选本地/远程、中文、空包、ZIP64、损坏拒绝、资源变化、非 Range 拒绝及有界中央目录读取通过；原生中文路径预览 121 项及滚动/关闭通过。 |

## P0 首轮诊断边界

工作目录为 `.tool-cache/free-parity-20261030/`。`results/` 首轮使用自定义管道，而 MSI 助手未继承该环境，不能作为生产默认入口的失败复现；旧 MSI 返回 1603，直接助手随后成功，结果原样保留。

`matrix-results/` 改用生产默认管道：旧 MSI 安装/运行中卸载返回 0；诊断 MSI 安装/运行中卸载返回 0。日志显示 checkpoint 命令返回 0。诊断 MSI 仅替换安装助手用于定位问题，不是完成本计划的新候选，不回写旧包来源。

后续同包通过和新增失败见下节。旧候选的 1603 失败保留，不能用多次成功覆盖新的失败。

## 当前候选来源与同包验收

产品源码提交 `c2a9bb6a7b58939bd457fdef57ed05acd9d8748e`，tree `95c258fae9b698dc66a084ba28d5c3770e96a0af`。后续验证脚本和报告提交不会改变此候选的源码来源。原始构建快照见 [BUILD-PROVENANCE](results/candidate-provenance/BUILD-PROVENANCE.json) 和 [ARTIFACT-MANIFEST](results/candidate-provenance/ARTIFACT-MANIFEST.json)。包位于 `artifacts/v7-productization/candidate/`，均未签名或发布。

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

回归入口为 `scripts/validation/installer_preflight_acceptance.py`，包含安装目录边界、版本 Host、锁失败、检查点失败保 Core、其他安装管道拒绝及失败后重试；进程夹具只能在 Sandbox 中运行。诊断 MSI 仅替换 Binary 表助手并刷新 PackageCode，不能冒充上表中的正式候选。正式新候选仍需重新构建及验收。

- r8 诊断 MSI SHA-256 `c95546703be6839afe58846f6d04baa8c776882f5e97b0ac6484a3987590443d`：[脚本报告](results/diagnostic-native-r8/native-msi.json) 在 240 秒超时，[完整 MSI 日志](results/diagnostic-native-r8/cold-uninstall.log) 随后记录卸载成功、`MainEngineThread is returning 0`。助手步骤约 1 秒，不能把整个脚本改判为通过。后续 MSI 日志先写沙盒本地再归档，避免共享目录逐行写入干扰。
- r9 诊断 MSI SHA-256 `ceb7b99a690cc57edf49b641a701e8d9ffcf748e317442fbc8150507cb644aa4`：[8 组助手边界通过](results/diagnostic-native-r9/helper/result.json)，[全新安装及冷卸载返回 0](results/diagnostic-native-r9/native-msi.json)。真实 Core 检查点返回 0 后，退出中的工作台路径查询短暂拒绝访问，助手误判失败；[原日志](results/diagnostic-native-r9/Temp-HLSInstaller-2544-1793366012065797400.log) 保留。已增加退出信号短暂等待及最终停机轮询重试，r10 已复核通过。

- r10 诊断 MSI SHA-256 `06818aadaae4b3082fd8583f6ee05fed68055131992cd7a61af2566f7c71ce9b`：[全部限定检查通过](results/diagnostic-native-r10/native-msi.json)，含冷安装/冷卸载、再次安装、真实 Core 检查点、32 MiB 续传 SHA-256、运行中卸载及数据库/成品保留；[8 组边界](results/diagnostic-native-r10/helper/result.json) 通过。受测助手 SHA-256 `381946f0eca9b3a5c0764e441b94b500f2c7ad7840fec8bcecc1c63b5e90bc3a`。这是修复版诊断包证据，正式候选仍需重新构建、升级/回滚及同包矩阵。

抓站组合验收已加入 `scripts/validation/free_parity_acceptance.py`：页面数/深度边界、关闭队列时间窗零请求、页面与 HEAD/文件请求的站点上下文、60 秒周期同步。新增检查目前仅完成语法检查，不能作为功能已通过的证据。

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

功能清单保留逐项状态和限定证据，移除了整体“完成率”百分比；`release_ready` 继续为 `false`。新功能均暂列 `partial`，直到当前候选的必要验证完成。没有产品账号、激活或收费服务新增。使用说明见 [免费版指南](../../free-edition.md)。未推送、发布或签名。
