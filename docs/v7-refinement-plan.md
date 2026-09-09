# v7 全方位精进计划与模块功能分类（2026-08-31）

本文档是本轮"从头把项目落地"工作的总纲：先摸底、再分类、后迭代。所有结论来自
2026-08-31 对四个活跃模块的全面调研（多子代理并行审读 + 文件级核对）。
迭代过程与验收结果追加在 `docs/v7-iteration-log.md`。

## 一、摸底结论（按模块）

| 模块 | 技术栈 | 规模 | 测试 | 成熟度 |
| --- | --- | --- | --- | --- |
| native_shell | Rust 单包多 bin（engine / native host / updater） | 50 模块，download_worker 7989 行 | cargo --lib 356 个 | 功能完整，零骨架；可靠性/并发有结构性缺口 |
| desktop_ui | Kotlin 2.4 + Compose Multiplatform 1.11（JDK 21） | Main.kt 3922 行等 6 文件 | gradle 66 个 | 功能面宽；连接层与线程治理有硬伤 |
| extension | WXT 0.20 MV3 + 原生 DOM popup | background 1720 行 + content 1198 行 | vitest 222 个 | 功能完整；若干正确性缺陷 |
| presenter_ui | Rust + Slint 1.16 | hot_main.rs ~1500 行 | 3 单测 + 视觉夹具 | 完成度高；与标题文案强耦合 |

工作树与 AGENTS.md 一致：历史实现（v3/v5/v6）只存在于 git tag，无残留目录。
`artifacts/v7-productization/feature-parity.json`：verified 24 / partial 4 / blocked 0，
`release_ready=false`。**按 AGENTS.md 约束，parity 全绿前不生成正式安装包。**

## 二、模块 × 功能分类

### native_shell（唯一数据所有者，SQLite + 常驻 Core）
- 接入层：`core_ipc.rs`（命名管道 \\.\pipe\HLSDownloader.v7，4B LE + JSON 帧）、`core_server.rs`（dispatch / WaitEvents 长轮询 / 守护线程）、`native_host.rs`（浏览器 Native Messaging stdio 桥）
- 契约：`contract.rs`（v7 线协议类型，`hls-downloader-v7-core` / v1）
- 状态机：`core_runtime.rs`（EventEnvelope 序列）→ `core_service.rs`（事件落库）
- 存储：`store.rs`（schema v6，WAL）、`v6_migrate.rs` / `migrate.rs`（升级迁移）
- 引擎：`download_worker.rs`（CoreCoordinator 总调度）、`http_engine.rs`（HTTP/HTTPS/WinHTTP/断点）、`media/hls.rs`、`media/dash.rs`、`media/merge.rs`（ffmpeg）、`torrent_engine.rs`、`ftp_engine.rs`、`sftp_engine.rs`
- 策略与安全：`net_policy.rs`（SSRF/限速/配额）、`credentials.rs` + `crypto_lite.rs`（DPAPI）、`av_scan.rs`、`site_rules.rs`
- 周边：`updater.rs`、`tray.rs`、`startup.rs`、`instance.rs`（单实例锁）、`core_spawn.rs`、`clipboard.rs`、`cast.rs`

### desktop_ui（Compose 主工作台，永不开库）
- 连接层：`Protocol.kt`（EnginePipeClient：每命令一连接 + hello 握手）
- 状态与事件：`Main.kt` AppShell（快照循环 / 事件循环 / 序号失序全量重同步）
- 界面：工具栏、侧栏、任务表、新建/批量/抓取/队列/详情/投屏/Handoff/更新等弹窗群
- 设置：`SettingsV7.kt`（10 页签）+ 站点规则编辑器
- 辅助：`UiDiagnostics.kt`（诊断 JSONL）、`UiTestApi.kt`（门控自动化）、`WorkbenchComponents.kt`（自绘组件库）

### extension（MV3 扩展，嗅探 + 接管 + 下发）
- 嗅探：`background.ts` webRequest 请求链 + `hooks.content.ts`（MAIN world fetch/XHR/MSE/Blob 钩子）+ `content.ts`（页面证据 + 悬浮 UI）
- 决策：`takeover.content.ts` + `lib/clickIntentStore.ts`（下载点击意图）+ `lib/pausedHandoffFollowups.ts`
- 下发：`lib/nativeBridge.ts`（Native Messaging 主通道）+ `lib/directBackend.ts`（loopback 快通道）
- 配置：`wxt.config.ts`、`native-host/{chrome,firefox}.json`

### presenter_ui（原生热弹窗，非第二工作台）
- ConfirmWindow / ProgressWindow / CompleteWindow（`ui/hot.slint`）
- 事件泵：4ms Slint Timer + 管道长轮询；单实例锁；冷启动拉起核心

### 衔接主干（单一事实来源：`native_shell/src/contract.rs`）
```
extension --NativeMessaging--> native_host --pipe--> core_server --> 引擎/SQLite
desktop_ui --pipe(直连)--> core_server
presenter_ui --pipe--> core_server
事件回流：core_runtime EventEnvelope → store 落库 + WaitEvents 广播 → 三个客户端
```

## 三、问题登记表（本轮迭代要消化的项）

| ID | 严重度 | 位置 | 问题 |
| --- | --- | --- | --- |
| D1 | 高 | desktop_ui/Protocol.kt:682 | 每条命令新开管道连接+hello+关闭；批量操作逐任务新建客户端（Main.kt:680/1119/1372），事件循环每 20s 重开连接 → 连接风暴 |
| D2 | 高 | desktop_ui/UiDiagnostics.kt:24-51 | 诊断写盘在 UI 调用线程同步执行，磁盘卡顿直接卡 UI |
| D3 | 中 | desktop_ui/Main.kt:792-796/1004-1056 | refreshKey 全量刷新会覆盖本地先行修改的深色模式/排序状态（回跳竞态） |
| D4 | 中 | desktop_ui/Main.kt:1710 | 未知任务状态静默归为"排队中"，过滤计数失真 |
| D5 | 中 | desktop_ui/Main.kt:407/414-421 | 无系统托盘、关窗即退出；presenter 探测无退避（5s 无限重试） |
| D6 | 中 | desktop_ui/Main.kt:3198 | 错误仅 3.6s Toast，无错误中心/历史可找回 |
| D7 | 低 | desktop_ui/Main.kt:2806-2847 | 旧版 SettingsDialog 死代码；缺关于页；material-icons 版本错配 |
| E1 | 高 | extension/lib/nativeBridge.ts:55-66 | 低优先级请求被抢占后未出队即二次 postMessage → 重复执行窗口 |
| E2 | 中 | extension/entrypoints/popup/main.ts:325-394 | 按钮状态机用中文文案字符串当 key，改文案即破坏状态 |
| E3 | 中 | extension/entrypoints/content.ts:745-755 | change 处理器引用其后才声明的 button，构造顺序脆弱 |
| E4 | 低 | extension/lib/directBackend.ts:41-42/108 | accept/reject_handoff 在豁免清单但 HTTP 端无实现，隐式契约 |
| N1 | 中低 | native_shell/src/media/hls.rs:1678-1705 | HLS 分片单次尝试失败即中止整批（HTTP 引擎有 5 次重试），弱网效率差 |
| N2 | 低 | native_shell/src/download_worker.rs:3011,1832 | 生产错误文案残留 "v6" 误导排障 |
| N3 | 低 | native_shell/src/torrent_engine.rs:897-1561 | 长度检查后 try_into().unwrap() 写法脆弱 |
| N4 | 中 | native_shell/src/core_ipc.rs:450-473 | 每连接一线程无上限、读循环无超时，慢客户端可耗尽线程 |
| N5 | 中 | native_shell/src/download_worker.rs:1385,2659,4144,4183 | dispatch 请求线程内做同步网络探测（BT/probe），慢源卡住整条连接 |
| P1 | 中 | presenter_ui/src/hot_main.rs:210,391,965,1172 | Win32 辅助按硬编码中文窗口标题查找窗口，与 hot.slint 文案强耦合 |
| P2 | 低 | presenter_ui/src/hot_main.rs:128-130 | 单实例锁错误靠字符串匹配 "already running" |
| P3 | 低 | presenter_ui/src/hot_main.rs:846 | 恢复 offers 时按 presentation=="fallback" 过滤，对核心契约隐式依赖 |
| G1 | 低 | 仓库 | desktop_ui/.kotlin/ 未入 .gitignore；tag v7.0.0 已打但文档称待门禁（登记即可） |

### 当前复核状态

上表保留为第二轮精进的原始问题基线，不再代表当前待办。当前源码的
权威状态如下：

| 问题组 | 当前状态 | 证据 |
| --- | --- | --- |
| D1-D7 | 已关闭 | `8baa840` 连接池；`d25a1dd` 异步诊断/设置代数/未知状态；`58d0567` 托盘/通知中心/关于页。 |
| E1-E4 | 已关闭 | `e946789` 抢占出队、稳定状态 token、构造顺序与具名 Native-only 操作集。 |
| N1-N3 | 已关闭 | `bb44253` HLS 重试、中性错误文案与有界大端读取。 |
| N4-N5 | 已关闭 | `c6f7ebe` 统一 64 连接上限；本轮把 TCP/命名管道的空闲帧头等待限制为 120 秒，完整帧头后的帧体共用 15 秒绝对截止，命名管道按可用字节分块消费；BT/磁力 probe 立即应答并通过单槽后台任务发布结果。 |
| P1-P3 | 已关闭 | `8875b2a` 结构化单实例错误，`60b1cd3`/`09b3c52` 显式 handoff owner/lease，`6da0405` 由 Rust 单一设置实际窗口标题。 |
| X1 | 已关闭 | 浏览器 `activate` 会确保启动 Compose；主工作台持有跨进程文件锁，重复启动只发送 `open_main`；Presenter 指令改为后台 IPC 并显示忙碌/成功/失败。 |
| X2 | 已关闭 | 构建门禁核对三端 v7 协议；本机安装覆盖前验证 `E:\h` 所有权；Portable 升级/回滚要求扩展身份连续。 |
| X3 | 已关闭 | handoff/media-push 旁路行与事件、任务快照、checkpoint 在同一 SQLite 事务提交；handoff 仅在 resolved 持久化成功后从内存移除。 |
| X4 | 已关闭 | 浏览器本地等待超时不再伪造 Core 终态；不确定所有权保持暂停并由持久 alarm 复核，用户已恢复/取消/完成的项目停止跟进，查询失败保留记录重试。 |
| X5 | 已关闭 | 工作台系统关闭与自绘标题栏关闭共用托盘驻留语义；显式“退出”仍结束应用。 |
| X6 | 已关闭 | NativeBridge 严格匹配 v7 request id；首个 postMessage 同步失败会断开已创建端口再重试。 |
| X7 | 已关闭 | 启动恢复写失败会阻止 Core 带错误状态启动；安装停服改走 v7 pipe Shutdown，先暂停并等待 worker/checkpoint，再唤醒阻塞的命名管道 accept 有序退出。 |
| G1 | 部分关闭 | `.gitignore` 已覆盖 `desktop_ui/.kotlin/`；已存在的 `v7.0.0` tag 与当前 `release_ready=false` 属发布治理项，未经明确授权不改写标签。 |

## 四、十轮迭代计划

| 轮 | 主题 | 验收标准 |
| --- | --- | --- |
| 1 | 摸底整合（本文档） | 文档齐、问题登记可追踪、分支建立 |
| 2 | desktop_ui 连接层复用 | 批量操作/事件循环复用常驻连接，断线自动重建，语义不变 |
| 3 | desktop_ui 治理 | 诊断异步落盘；深色/排序不再回跳；未知状态显式标注；presenter 探测退避；死代码移除 |
| 4 | desktop_ui 体验 | 托盘驻留（关窗可最小化）、关于页、错误中心面板 |
| 5 | extension 修复 | 抢占不再重复执行；状态机改稳定 key；content 时序加固 |
| 6 | native_shell 引擎 | HLS 分片重试 + 退避；v6 文案清理；torrent unwrap 加固 |
| 7 | native_shell 并发 | 管道连接数上限 + 读超时；阻塞探测缓解 |
| 8 | presenter_ui 加固 | 窗口标题单一来源；单实例锁结构化判定；panic 崩溃日志 |
| 9 | 文档与门禁 | module-map/iteration-log 更新；parity 复核；改动脚本 PS5.1/7 自查 |
| 10 | 验证与收口 | 按最新要求不做本地编译、测试、打包或安装，仅执行静态一致性检查；分支合并回 main 并推送 GitHub |

## 五、硬约束（全程有效）

1. 项目内容只允许存在于本工作目录；唯一例外：桌面扩展发布文件夹（每次更新先删旧副本、只保留一个版本）与 `E:\h`（唯一本机安装位，当前不存在、parity 未全绿前不新装）。
2. `feature-parity.json` 全绿前不生成正式安装包；本轮以代码与测试可靠性为主。
3. PowerShell 脚本必须 PS 5.1 + PS 7 双兼容；JSON/manifest 一律 UTF-8 无 BOM。
4. 本轮不执行本地编译、测试、构建、打包或安装；只做一次静态一致性检查，禁止过度验证与重复返工。
5. 协议主干（contract.rs）为单一事实来源，任何接口改动必须三端同步评估。
6. 本轮按最新要求只修复并同步 GitHub，不执行 `E:\h` 安装或桌面扩展发布。
