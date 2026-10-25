# 已安装产品的正常管道与并发冷启动验收

## 本批目的与源码变化

继续核对历史首次安装 Engine 在 15 秒测试 TCP 期限内未就绪的问题。该次失败没有入口日志，既有证据不足以确定根因。本批补充此前安装验收未覆盖的正常 Windows 命名管道入口，而不是延长连接期限、吞掉错误或把再次成功当作历史问题已修复。

生产代码和桌面成品没有变化。复用当前 MSI SHA-256 `e6c49715dfd122c1858f8b568c69ec45e4273c9ba6c58ce3f0b4e60f696493cf`，Engine SHA-256 `8d9200cb821c899f604cf8b420eb742e6650df3a840f55dbcab9f37ab3e77f9e`，NativeHost SHA-256 `27d6c619e6489fd46aa61a5598dff8a79779621364d43b1b0a4d8f65ab6e7c92`。

在既有 `smoke_v7_installed_footprint.py` 增加可选 `--verify-native-host-startup`，复用 Native Host 帧读写和命名管道夹具。仅在 Windows Sandbox 安装并运行产品，不在宿主安装、不调用云端。

## 方法与实际结果

新启动的沙盒从含空格自选路径安装最终 MSI，核对组件和插件 ZIP 哈希。产品进程不传任何测试 IPC、测试数据目录或自定义 Engine 路径，仅打开已有的阶段诊断开关。四个 Host 通过 barrier 同时发起，每个连续发送两个真实 Native Messaging ping。

| 场景 | 四个 Host 首次响应范围 | Core 启动次数 | 常驻 Core |
| --- | ---: | ---: | --- |
| 首次安装后冷启动 | 1182.95–1185.66 ms | 1 | 唯一 PID 5100 |
| Core 已常驻时重新接入 | 70.41–74.33 ms | 0 | 沿用 PID 5100 |
| 正常关闭后重新拉起 | 126.60–129.23 ms | 1 | 唯一新 PID 1848 |

共 12 个 Host、24 个帧响应全部成功，协议版本正确；每组都核对默认命名管道 hello 的 PID 与实际安装路径下唯一 Engine 一致，加载的默认下载位置为系统 Known Folder 的 Downloads。关闭请求通过同一正常管道发出，确认进程实际退出后才进入下一阶段。沿用现有 Native Host 冷启动验收的 1500ms 目标，未放宽期限。

冷启动阶段日志显示：Core 入口到 IPC 就绪约 1.058 秒，其中 `database_open` 阶段约 1.049 秒；正常关闭后再次启动的 Core 入口到 IPC 就绪约 7.5ms。这只能定位本次冷启动的阶段开销，没有证明它就是历史 15 秒超时的根因。源码检查确认现有建表和版本标记已在同一事务中，未加入无证据的重复优化。

之后同一安装继续通过默认下载目录、公开 MDN 文件内容 SHA、HTTP 故障重试与队列、六组 HLS 普通/范围故障恢复、完成及删除缓存清理、正常卸载和产品注册移除。原始结果及阶段诊断见 [results/installed-native-startup.json](results/installed-native-startup.json)。

## 限制与收尾

本批没有打开真实浏览器、安装扩展或验收桌面按钮；Native Messaging 协议进程验收不能替代浏览器端到端接管验证。没有重建 UI/插件/Core 或改动桌面成品，未增加安装体积。640 个回归的结果仍是[前批同一 Engine 的验证](../hls-range-body-20261025/README.md)，本批没有重复执行它们。

历史首次启动超时仍未复现并确定根因，不能记为已修复；指定直播站点 403、原始 HLS 卡死现场、浏览器 MV3/POST/Blob、设备/云端和长期负载边界仍保留。不能宣称全功能与 IDM/AB 全面对等。

宿主只读核查及本批进程收尾另见 results。构建缓存删除此前被自动审批拒绝（`blocked by policy`），本批不换方法重试；任务创建的运行脚本与报告暂存仍在项目 `.tool-cache/request-control-20261025`，不声称目录只剩源码。没有创建分支或 worktree，没有改动用户下载、密钥、备份或其他项目。

## 后续：拆分 SQLite 初始化耗时

[本地文件交接包](../local-offer-20261025/README.md)首次安装验收出现 1645.09ms 超标。随后旧包→新包对照复用了已有任务库，不能作为首次初始化故障已修复的证据。本轮保留原 1500ms 门槛和 SQLite 事务、同步策略，仅在 `HLS_V7_STARTUP_PROFILE` 既有开关下增加目录、连接、版本读取、PRAGMA、建表与提交的计时；正常模式不输出这些阶段信息，没有增加依赖、播放引擎、配置项或常驻线程。

诊断 Engine SHA-256 为 `9a86196a97ecbd46130435ba5a4ee0dc9c3e65db7283fb9fb51e0df6e52ed4aa`，在宿主完成离线 release 编译，产品仅在沙盒运行。既有 `smoke_v7_native_host.py` 按预定诊断版→原版→诊断版→原版顺序执行四组，每组复制并 flush 二进制、创建独立新任务库，四个 Host 并发各发两个 ping。四组首响应最慢分别为 300.11、166.04、169.38、167.84ms，均保持唯一 Core；两组诊断任务库初始化分别为 13.985、12.006ms。记录为 [执行结果](results/sqlite-stage-v2-results.json)及同前缀的四份逐 Host JSON。样本少且首次执行有缓存差异，不据此声称诊断版更快或更慢。

首次独立夹具没有生成有效测量：Windows PowerShell 的 `ErrorActionPreference=Stop` 只保留 Python traceback 首行，见[失败原文](results/sqlite-stage-harness-first-failure.txt)。随后修正原生命令的错误采集，并在独立测试输入旁补上安装包既有的 `vcruntime140.dll`，再执行上述固定四组。第一次完整异常未被采集，不能确定缺少运行库就是该次失败的唯一原因，也不将它计算为产品功能失败或通过。

同一沙盒的默认任务目录及默认管道另测三阶段，通过首启 245.78ms、常驻 75.80ms、正常关闭后重启 140.68ms，见[独立运行记录](results/sqlite-stage-default.json)。它不经过 MSI，且执行前已有四组独立运行，不能代替安装现场。

因此另开全新 Windows 沙盒，先记录[安装前默认数据库不存在](results/sqlite-stage-pristine-before-install.json)，再安装仅替换诊断 Engine 和 provenance 的 MSI。诊断 MSI SHA-256 为 `36b9d7912ddd8d7584bb6f7acc745677234f91f0c3794acdf32db52f55042a14`。安装夹具现在记录 `database_existed_before_first_launch`，本次在安装完成、首次 Host 启动前仍为 `false`，避免将安装后第一次启动和新任务库混为一谈。

最终[安装现场记录](results/sqlite-stage-pristine-installer.json)通过自选含空格路径、五组件与双插件 ZIP 一致性、默认 Downloads、并发唯一 Core、公开 MDN 成品字节和 SHA-256、完成缓存清理、测试成品删除、闲置测量及卸载注销。首启最慢 1309.48ms、常驻 75.83ms、正常关闭后重启 185.53ms。首次 SQLite 连接和版本读取累计 0.878ms，PRAGMA 阶段 465.107ms、建表阶段另 104.235ms、事务提交另 604.791ms，整个打开/初始化 1175.117ms；同一任务库重开为 2.389ms。这将慢点进一步收窄到新库初始化写入，不能证明具体文件同步调用、虚拟磁盘、缓存或杀毒是根因。没有降低持久化保护、提前伪报就绪、延长门槛或认定先前 1645.09ms 及历史 15 秒超时已经解决。

本轮不是性能优化包，也没有再次运行完整 Core/UI/插件回归或 UI、云端、实体设备、长期负载验收。诊断 MSI 未交付到桌面或 GitHub Release；桌面 MSI 仍为 `2c87bdc8af83100538350809befae63324540e0b430e23acd8b2ce7408432946`。两个沙盒及产品进程均已退出，见[收尾记录](results/sqlite-stage-handoff.json)；任务生成物仍集中于既有 `.tool-cache`，没有绕过历史删除拒绝。本轮代码改动仅诊断计时与夹具的数据库存在性记录，未修改下载或持久化行为。

## 后续：当前版本任务库重开不再重复写版本

进一步检查发现：每次打开现有版本 6 的任务库，`CREATE ... IF NOT EXISTS` 后仍无条件执行 `PRAGMA user_version=6`，即使表和版本完全一致，也会产生写提交。扩展既有 `settings_are_typed_and_persistent` 回归，在沙盒用真实文件保存设置、建立独立观察连接、关闭再打开任务库，检查设置恢复及同一观察连接的 `PRAGMA data_version`。旧代码实际失败，观察值由 2 变 3，见[失败原文](results/store-reopen-red.log.txt)。[SQLite 文档](https://www.sqlite.org/pragma.html#pragma_data_version)说明同一连接的该值可用于检测其他连接的写提交；没有用延迟阈值或 sleeps 推测发生了写入。

修复复用打开时已经读到的版本号，只在初始化版本不匹配时在同一建表事务写入当前版本。未知版本仍在建表前被拒绝；当前版本仍执行既有的缺失表/索引补建；事务提交、WAL 和 NORMAL 同步策略均保留，没有新增依赖或配置。初次新库仍需要可靠落盘，这个改动不解决先前约 2.25 秒的新库初始化慢样本。

宿主离线编译成功，产品仅在沙盒运行：634 项常规 Core 回归、4 项 curl 和 4 项 WinHTTP 回归通过；还有 4 项 ignored 未执行。文件重开回归包含在上述 634 项中，不能重复计数。见[分组结果](results/store-reopen-test-results.json)及同前缀构建和测试日志。本轮尚未测得实际启动延迟的改善幅度；最终安装包和桌面交付状态后续单独记录。

对照资料仅用于理解取舍：[AB 作者的历史架构说明](https://github.com/amir1376/ab-download-manager/discussions/318)描述使用文件系统保存数据，不能据此推断当前版本所有持久化实现或性能；[IDM 官方选项说明](https://www.internetdownloadmanager.com/support/using_idm/options.html)提供自动启动等用户选项，未披露其存储实现。本项目需要保持现有 SQLite 任务、设置和凭据的一致性，不因竞品使用不同存储就整体迁移。以上不是实际竞品性能对测。

### 最终候选包的安装验证

源码 `b674a58` 构建的 Engine SHA-256 为 `2e0effa9f148869b3b51fc6b57857e81378b7a528000b8396bb88e86ed3df873`，Native Host 为 `c0d3e28df3ddf129b02eec59f864564baffe1f9abae02403365f35452b5f3140`。候选 MSI SHA-256 为 `f1e92a17a6ba3108697fc4851f250192cc3ec2f70fd119a5e653b558edda215c`，203444224 字节，比前一桌面 MSI 多 12288 字节。未重建 UI/JRE/播放器/Updater/Presenter；双插件使用文件名等待修复后已验证的同一 ZIP。

新沙盒先运行上述回归，再安装原桌面 MSI 并直接覆盖为本候选。安装前及最终 Host 首次启动前默认任务库均不存在；不等于整个操作系统和文件缓存完全冷态。最终包九项验收通过：同哈希本地包、自选含空格路径、五组件与插件 ZIP 一致、默认命名管道并发唯一 Core、系统 Downloads、公开 MDN 文件内容及 SHA-256、完成缓存删除及测试成品删除、闲置采样、卸载注销。覆盖后旧产品登记和旧可执行文件均消失，最终卸载后新登记也消失。见[安装报告](results/store-reopen-installer.json)和[覆盖报告](results/store-reopen-upgrade.json)。

四个 Host 的首响应最大值分别为首次新库 135.97ms、常驻接入 78.91ms、关闭后重开 120.80ms；启动计数分别为 1/0/1。没有放宽 1500ms 门槛。单次样本不证明历史新库初始化慢点已消失，更不能将它全部归功于只影响现有版本重开的优化。安装共 469603729 字节、222 文件；仅 Core 五秒闲置样本 RSS 为 16891904 字节，8 个逻辑 CPU 上整机占用 0.0390625%，未运行 UI，不代表整个桌面程序的占用。

随后对同哈希 MSI 补跑已安装 Engine 的故障验收：HTTP 403 不自动重试、503 重试、异常 206 处理与后续队列继续；HLS 普通/范围分片的 403、503 与截断恢复六组均通过，每组测试任务、成品与分片缓存均移除，最后卸载注销通过。见[已安装包故障报告](results/store-reopen-faults-installer.json)。

桌面 `HLSDownloader/最新-20261025` 已替换为本套未签名 MSI、Chromium/Firefox 插件及两份插件源码，仍为 `release_ready=false` 候选，未更新 GitHub Release。七项成品资产和 SHA256SUMS 校验通过；完整插件源码的 124 个文件逐一匹配 HEAD 的 Git blob。首次直接比较工作文件字节在 `hooks.content.ts` 报错，确认仅为工作树 454 个 CRLF 与 Git 导出 LF 差异，换行归一后文本相同，未修改源码内容。见[桌面清单](results/store-reopen-desktop-delivery.json)和[交付复核](results/store-reopen-delivery-audit.json)。最终沙盒产品进程为空并已关闭，见[收尾记录](results/store-reopen-vm-handoff.json)。未在宿主安装；历史缓存删除拒绝仍未绕过，生成物保留在项目既有 `.tool-cache`。
