# IDM 架构对齐复核与优化路线（7.0.7）

> 本文为用户提供的外部 AI 分轮评审记录，包含当时的实现状态和其自述验证结果；本次实际整合、Windows 验证及仍存限制见 [整合记录](source-integration-20261117.md)。未完成同条件原版对照，不能从材料未提及某机制推断 IDM 没有该机制。

日期：2026-11-16。范围：Rust Core（`native_shell/`）、协议与客户端（`crates/`）、Native Host/CLI（`native_clients/`）的架构与性能；UI（`native_ui/`、`extension/`、`desktop_ui/`）由另一项工作重做，本文只给出与 UI 相关的进程边界建议，不改其文件。

IDM 侧依据：参考目录 `README.md`、`RECONSTRUCTION.md`（§5、§9–§12、§27、§82–§83）、`2_项目主体源码/download_engine/` 的恢复注释，以及本仓库已有的 [源码核查](../validation/idm-source-audit.md)、[原生交接追踪](../validation/idm-native-handoff.md)、[逐项验证](../validation/idm-port-validation.md)。只借鉴**行为与架构**；反编译代码和其中的标识符不进入本项目，本轮代码均按通用原理自行编写。

> 结论先行：本项目的引擎层具备可对照 IDM 参考材料的机制（单文件定位写入、响应身份校验、自适应连接、尾段切分、原子发布）。与 IDM 的真正差距不在“切分算法”，而在**控制面和持久化面过重**：进度经文件中转、每 200 ms 一次 SQLite 写事务、全局 Core 互斥、控制信号靠文件轮询、媒体分片逐个 fsync。“热路径放内存、减少落盘频率”是本项目可验证的优化方向，尚不能据此确定 IDM 性能优势的主要来源。路线图据此排序。

---

## 1. 分层对照

### 1.1 下载引擎：分段、动态切分、连接数

| 维度 | IDM（参考材料可确认的部分） | 本项目 | 判断 |
| --- | --- | --- | --- |
| 初始切分 | 按连接数切初始区间，64 位 Range/If-Range 请求，解析 Content-Range（RECONSTRUCTION §27） | `download_ranges`（`native_shell/src/http_engine.rs:1458`）按 `连接数×4` 均衡切块、最小 256 KiB，扣除已完成区间 | 行为方向相近，未实测等效 |
| 动态切分/连接“抢活” | 有“剩余量最大的活动区间再对半分”的行为；原函数归属、锁范围在参考材料中曾有冲突，后已更正（见核查文档） | `RangeScheduler::claim`（`:1848`）待办为空时从活动区间里找**安全尾部**（跳过已预留读取）对半切；仅在有 ETag/Last-Modified 校验器时启用（`active_split_minimum`，`:1651`）；响应通过身份校验后才允许被切 | 本项目有明确保护；IDM 对应行为未证实，不作优劣结论 |
| 开连接节奏 | 有“剩余量门槛”控制是否新开连接 | `AdaptiveConnectionController`（`:1726`）从 2 连接起步，每成功若干区间 +1，遇 429/5xx/写盘阻塞 ≥250 ms 立即减半 | 本项目有背压；IDM 对应机制尚未核实 |
| 重试 | 有状态码映射与重连（细节未完整恢复） | 每区间最多 5 次，指数退避，尊重 `Retry-After`（`defer_requests`）；0 进展计失败、有进展清零 | 相当；本轮补抖动（见 §3） |
| 连接复用 | WinInet/自有 socket，细节未核 | WinHTTP session 缓存 + 按 origin/代理分键的空闲 connect 句柄池（`IdleHandlePool`，`:35`，每键 8、总 32） | 行为方向相近，未实测等效 |
| 限速 | 全局限速，设置存注册表 | `net_policy` 令牌桶：全局/任务/时段 + 小时配额，逐块消费可施加网络背压 | **本项目更丰富** |

### 1.2 文件写入：预分配、写缓冲、检查点

| 维度 | IDM | 本项目 | 判断 |
| --- | --- | --- | --- |
| 数据落点 | 参考材料显示每用户 `DwnlData\<任务>` 目录保存分段临时数据，完成后有合并阶段（§27；恢复 C 的合并函数不完整，合并细节未逐行核实） | 单个输出文件 `set_len(total)` + Windows 稀疏标记（`mark_file_sparse`，`:1293`），各 worker 按偏移 seek 写 | 省去本地整文件合并拷贝；实际延迟与写入量需测量 |
| 写缓冲 | 有发送/接收两类缓冲，按需分配（`two_buffers_recovered.c` 注释） | 每 worker 惰性分配 1 MiB `BufWriter`，256 KiB 读块；首次领到区间才打开文件 | 行为方向相近，未实测等效 |
| 断点记录 | 任务目录内的续传状态 | `native-engine.ranges.json`：先 flush+`sync_data` 再原子替换登记（`persist_range_progress`，`:2109`），多 worker 合并提交 | **本项目更可靠**（断电后不会把未落盘数据当完成）；但代价高，见下 |
| 检查点代价 | 未见逐段 fsync | 旧逻辑：每 worker 每 8 MiB 一次（payload `sync_data` + 记录 `sync_all` + rename）。已测 32 MiB fsync 中位约 0.65 s（`idm-port-validation.md` 同步诊断） | **本轮已改**：全任务至多每秒一次、64 MiB 硬上限（§3） |

### 1.3 进程模型

| | IDM | 本项目 |
| --- | --- | --- |
| 常驻进程 | `IDMan.exe` 一个进程：MFC UI + 下载线程 + 队列 + 设置（注册表） | `HLSDownloaderEngine.exe`（Rust Core）：下载、调度、SQLite、托盘；**无 UI** |
| UI | 同进程 | `hls-downloader-presenter`（Slint，软件渲染）按需启动、经命名管道连 Core；`HLSDownloader.exe` 为短命启动器 |
| 浏览器桥 | `IDMMsgHost.exe`（Native Messaging）经 COM 调主程序；另有 `IDMNetMon.dll` 管道/1001 端口 WebSocket（§10–§12） | `native_clients` 的 Native Host 经 `\\.\pipe\HLSDownloader.v7`（长度前缀 JSON，owner/SYSTEM DACL）连 Core，Core 不在时拉起 |
| 媒体 | 自带 TS→MP4 重封装（进程内） | FFmpeg 子进程合流；libmpv 播放器独立 `--player-process` 子进程 |

判断：**整体形态与 IDM 一致**（一个常驻引擎 + 薄浏览器宿主），差别是 UI 拆成独立进程。这是正确取舍：UI 关闭即释放全部渲染内存，Core 常驻时只剩引擎占用；UI 崩溃不影响下载。应保留，不要为了“像 IDM”把 UI 合回 Core。需要补的是让 Core 本身像 IDM 的引擎部分一样“热路径不碰磁盘”。

### 1.4 IPC 与浏览器接管

- IDM：插件 type 13 请求 → NetMon → COM → 主程序；COM 返回 `S_OK` 映射为裁决 3，插件据此取消浏览器下载；主程序有“投递后即返回”的异步分支（[交接追踪](../validation/idm-native-handoff.md)）。即 IDM 的“接收成功”不代表已持久化。
- 本项目：Host → Core 持久 handoff，持久接收且可呈现后才转移责任，按原业务 ID 可查结果，凭据按 origin 隔离（`docs/architecture/v7-architecture.md`）。**这比 IDM 更稳**，保留。
- 帧大小：Core 4 MiB、Native Messaging 入 4 MiB / 出 1 MiB 上限，均有界。保留。

### 1.5 启动与占用

- IDM `InitInstance`（§82.3）：先建单实例互斥体，**之后**才做注册表与 regsvr32 注册；重复启动直接被互斥体挡下。
- 本项目旧顺序（`native_shell/src/bin/hls_downloader_engine.rs`）：每次启动**先**写 Native Host 清单与注册表，**再**抢实例锁。浏览器 Host 拉起 Core、开机自启和用户双击都会重复这段 I/O，且它位于 `ipc_ready` 之前，直接拖慢 Host 冷启动首个响应（计划门槛 ≤1500 ms）。**本轮已改**（§3）。
- 存储：SQLite WAL + `synchronous=NORMAL`（`store.rs:746`），合理。
- 包体：Core ~15 MiB、Presenter ~27 MiB（其中字体 ~16 MiB）、FFmpeg/libmpv 解压后 ~158/114 MiB（`native-migration-plan.md` 基线）。常驻内存与包体无关，二者要分开报告。

---

## 2. 主要问题（按影响排序）

1. **进度热路径写 SQLite**。HTTP worker 线程每 200 ms 读一次进度 JSON（`download_worker.rs:2826`），调用 `mark_progress_speed`（`:3076`）→ `UpdateProgress`（`core_service.rs:84`）→ `store.apply_events` 一次写事务，且持有全局 `Mutex<PersistentCore>`。N 个活动任务 = 5N 次事务/秒，并与所有 IPC 命令争同一把锁。IDM 进度只在内存，停止/退出时才写状态。
2. **控制与进度靠文件中转**（同进程内线程之间）。引擎每 200 ms 原子写 `progress.json` 和 `native-engine.active.json`；每个范围 worker 每 50 ms 打开读取控制文件（`read_control`，`http_engine.rs:213`），16 连接 ≈ 320 次 open/s。Windows 上每次 open 都要过 Defender 等过滤驱动，CPU 与延迟都不可忽略。
3. **HTTP 检查点 fsync 过密**（已在本轮缓解）。
4. **HLS 分片逐个 fsync + rename**（`media/hls.rs:2473` `save_segment`）。长视频上千分片 = 上千次 fsync + 上千个小文件，最后再交 FFmpeg。已测完成吞吐 64.79→19.73 MiB/s，低于 20 MiB/s 门槛（`idm-port-validation.md`）。
5. **启动关键路径上的注册写入**（已在本轮修复）。
6. **重试无抖动**：同一任务的多连接常因同一次抖动同时失败，固定 200/400/800/1600 ms 会同步重连、再次同时撞上限流（已在本轮修复）。
7. **浏览器身份路径每响应一个 curl 子进程 + 读线程**（`fetch_via_curl_impersonate`，`http_engine.rs:3371`）。多连接时进程数随连接数线性增长，是 IDM 没有的开销。
8. **文档滞后**：`v7-architecture.md` 仍以 Compose 为主工作台，与 7.0.6 起的原生包不符，容易误导后续架构决策。

---

## 3. 本轮已实施的改动（均在 `native_shell/`，未改协议和数据库结构）

| # | 改动 | 位置 | 预期影响 | 验证 |
| --- | --- | --- | --- | --- |
| A | **检查点节流**：字节门槛 8 MiB 不变；达到后，同一任务所有 worker 合计至多每 1 s 提交一次；未同步量 ≥64 MiB 无条件提交；边播边下（存在 `playback-byte-priority`）时保持按字节提交。只改变“何时登记”，仍是先 flush/sync 后登记，断电后每连接至多重下 64 MiB 未登记数据 | `http_engine.rs` `DURABLE_CHECKPOINT_INTERVAL/MAX_BYTES`、`RangeCheckpoints::{since_last_commit,mark_committed}`、`durable_checkpoint_due`、`fetch_range` | 高速多连接下 fsync 次数从“每 worker 每 8 MiB”降为“每任务约每秒 1 次”；例如 8 连接 200 MiB/s 时约 25 次/s → ≤1–3 次/s。吞吐提升幅度**未实测** | 新增 `durable_checkpoints_are_paced_but_bounded`；HTTP 引擎 87 项全过 |
| B | **重试抖动**：退避基准 200/400/800/1600 ms 不变，实际取 `[基准, 1.5×基准]`，种子用 `RandomState`+线程+时间，无新依赖；`Retry-After` 逻辑不变 | `http_engine.rs` `range_retry_delay_ms`、`retry_jitter_seed`、`wait_before_range_retry` | 降低多连接同步重连和对限流站点的冲击；最短等待不缩短 | 新增 `range_retry_delay_keeps_exponential_floor_and_bounded_jitter`；429 退避回归通过 |
| C | **启动顺序**：先取单实例锁，重复启动立即退出；Native Host 清单/注册表修复移到后台线程，不再阻塞 store 打开和管道就绪。启动剖析阶段名 `registration` 改为 `registration_dispatched`（无脚本依赖该名） | `native_shell/src/bin/hls_downloader_engine.rs` | 重复启动零注册 I/O；首个实例的 `ipc_ready` 不再等待注册表写入。Windows 上**未实测**耗时 | `cargo build --bin hls-downloader-engine` 通过；Linux 上注册为空操作，Windows 行为需在下一轮安装验收中确认 |

注意事项：C 之后，若在 Core 运行期间把安装目录换到别处，注册修复要等下一次 Core 启动才发生；升级流程本身会先关停 Core（`--shutdown`），不受影响。

---

## 4. 保留什么

- 单文件定位写入 + 稀疏预分配（减少本项目的合并拷贝）。
- 响应身份校验（ETag/Last-Modified/Content-Range 起点、终点、总长一致）后才写盘、才允许切分。
- 自适应连接数 + 写盘背压 + `Retry-After`；限速令牌桶与小时配额。
- 先同步后登记的检查点语义、成品原子发布、失败不发布残缺文件。
- 持久 handoff 合同、凭据按 origin 隔离、Host/Core 有界帧。
- Core 与 UI 分进程、播放器独立进程、FFmpeg 按需。
- 惰性 writer/缓冲、播放 seek 优先领取。

## 5. 要改什么（路线图）

### P0（稳定性/性能收益最大，风险可控）

1. **进度与持久化解耦**（问题 1）。**状态（2026-11-16）：已实施，Linux 单测验证；Windows 与多任务实测未做。** 实际做法与下段原建议不同：进度事件**仍进内存事件环、仍占序号、事件名不变**（旧客户端按序号拉取的路径零改动），只是不再每次写 SQLite；为避免崩溃重启后重用已发布序号，跳过写库前先把持久检查点预留到 `当前序号+1024`（每 1024 个易失事件一次小写入）。SQLite 写入条件：首次进度、状态/阶段变化、非活动状态、距该任务上次写库 ≥5 s、以及 `PersistentCore` 正常释放时冲刷。见 §8。原建议：进度事件按 5 Hz 只更新内存快照并推送订阅者；SQLite 只在状态变化（开始/暂停/失败/完成）和每 ≥5 s 时写一次 `downloaded_bytes`。需要处理的合同：事件序号目前与持久事件环绑定（`v7-architecture.md`“提交成功后才发布”），建议新增“易失进度事件”类别不进事件环，客户端用最新快照覆盖；协议向后兼容（旧客户端仍收到同名事件）。预期：活动任务多时 Core 锁竞争和 WAL 增长显著下降，UI 命令响应更稳。
2. **任务内存控制令牌**（问题 2）。**状态（2026-11-16）：已实施，Linux 单测+协调器端到端验证；Windows 未验证。** 控制令牌按控制文件路径注册（`task_control.rs`），Core 写原子量同时写文件；已注册路径的文件至多每秒对账一次。`progress.json`/`native-engine.active.json`/媒体进度在运行任务内降为 ≤1 Hz 落盘、进程内读者读内存副本。见 §8。原建议：每个运行任务注册一个 `Arc<AtomicU8>`（run/pause/cancel），Core 收到命令时同时写原子量和控制文件（文件仍用于崩溃恢复与外部工具）；引擎热路径只读原子量。进度同理改为共享原子计数，`progress.json` 降为 1 Hz 或仅在退出时写。预期：每任务每秒数百次文件 open 降为个位数，暂停/取消延迟从 ≤50–200 ms 降到 ≤一次 read。
3. **HLS/DASH 分片组提交**（问题 4）。**状态（2026-11-16）：HLS 点播已实施（每 32 个分片 / 64 MiB / 1 s 一批），直播与 DASH 未改；吞吐未实测。** 见 §8。原建议：保留“同步后才计为完成”的语义，但改为每 1 s 或每 N 个分片批量 `sync` 并一次性更新完成记录（与 §3-A 同一思路）；进一步可把分片顺序追加到单个暂存文件并记录偏移表，减少小文件和 rename。验收按现有五轮中位数脚本，目标完成吞吐回到 ≥20 MiB/s 门槛之上。

### P1

4. **curl 身份路径的进程复用**（问题 7）：同一任务多连接共享一个 curl 进程（`--parallel` 或多 URL 范围），或在服务器不要求浏览器指纹时自动回落 WinHTTP；至少把 curl 连接数单独封顶。**状态（2026-11-16）：部分——只做了封顶。** 复用进程未做：curl CLI 启动后不能再接收新请求，`--parallel`/多 URL 无法配合动态领取与尾段切分，也无法保证逐响应身份校验后才写盘/才允许切分；改为每任务 ≤4、全局 ≤16 个存活 curl 进程（`curl_slots.rs`）。见 §9。
5. **Core 锁拆分**：下载线程只需要读 spec/设置与上报进度，不应与 IPC 命令共用一把 `Mutex<PersistentCore>`；进度走 P0-1 的无锁通道后，剩余写路径改为单写线程 + 通道。**状态（2026-11-16）：热路径部分已完成。** 周期进度改走合并信箱、非阻塞批量应用（`progress_relay.rs`）；检查点读取与边播边下优先级改为内存（`range_cache.rs`、`media/preview.rs`）。未做：把 `PersistentCore` 拆成多把锁或独立写线程；`progress_event` 仍在锁内做两次文件元数据查询（`preview.m3u8` 存在性、输出文件长度）。见 §9。
6. **启动剖析常态化**：已有 `HLS_V7_STARTUP_PROFILE`，建议在安装验收中固定记录 `entry→instance_lock→store_ready→ipc_ready`，并把 `CoreServer::open_default` 中的迁移检查、历史事件加载做成惰性（首个订阅时再加载）。
7. **FTP 多连接**：沿用 HTTP 的 `RangeScheduler`（REST 偏移 + 每连接独立数据通道），补齐与 IDM 的已知差距。**状态（2026-11-16）：已实施（明文 FTP）。** 未复用 HTTP 的 `RangeScheduler`（其预留/切分与 HTTP 响应身份绑定），改为 FTP 内的固定分段队列 + 资源身份绑定的完成表；FTPS 仍单连接。见 §9。

### P2

8. **自带轻量 TS/fMP4 → MP4 重封装**（IDM 的做法；**2026-11-16 先做了可选的 FFmpeg `-c copy` 重封装设置 `hls_remux_ts_to_mp4`，默认关，进程内封装器未做**），仅对“单视频+单音频、无需转码”的常见 HLS 走进程内流式封装，其余仍交 FFmpeg；可省去 FFmpeg 进程启动和一次全文件读写。需按现有“合法媒体解码哈希”标准验收后才能替换。
9. **包体**：Presenter 字体子集化（~16 MiB → 按常用汉字集 2–4 MiB）；FFmpeg 定制裁剪（只保留 demux/mux/aac/h264/hevc 拷贝相关组件）。只影响安装体积，不影响常驻内存。
10. **文档**：更新 `v7-architecture.md` 的进程图（Slint Presenter 取代 Compose），避免后续评审基于旧图。

### 预期效果（定性，未实测）

| 项 | 主要收益 | 风险 |
| --- | --- | --- |
| 已做 A | 高速大文件吞吐、磁盘 I/O 抖动 | 崩溃/断电后每个连接至多重下未登记的 64 MiB（正常为约 1 s 的数据） |
| 已做 B | 限流站点成功率、重连风暴 | 无（最短等待不变） |
| 已做 C | Host 冷启动首响应、重复启动开销 | 运行中迁移安装目录时注册延后 |
| P0-1/2 | 多任务时 CPU、锁竞争、UI 响应；Windows 上 AV 扫描开销 | 需协议上区分易失进度事件 |
| P0-3 | HLS 完成吞吐（当前未达门槛） | 组提交窗口内崩溃需重下少量分片 |

## 6. 诚实边界

- 本项目有身份校验、先同步后登记和持久 handoff 保护；与 IDM 的正确性及性能差距仍需同条件对照，不能据参考材料的缺失作优劣判断。
- 没有做同机同条件的 IDM 对照测试（吞吐、CPU、内存、启动），本文的性能判断是机制推断，不是测量结果。
- 本轮在 Linux（aarch64）上验证；WinHTTP、DPAPI、注册表、稀疏文件等 Windows 路径只做了编译检查，未运行。

## 7. 本轮验证记录

- `cargo check -p hls-native-shell`、`cargo build -p hls-native-shell --bin hls-downloader-engine`、`cargo clippy -p hls-native-shell --all-targets`：通过；无新增警告（`fetch_range` 参数过多为既有警告）。
- `cargo test -p hls-native-shell --lib http_engine::tests`：87 通过 / 0 失败（含 2 项新增）。
- `cargo test -p hls-native-shell --lib`（全量）：667 通过 / 10 失败 / 6 忽略。10 项失败全部依赖 Windows（WinHTTP、DPAPI、`LOCALAPPDATA`、`C:\` 路径）或计时（播放慢读者、BT 选择超时）；把 `http_engine.rs` 恢复为改前版本后，同一批中可在 2 分钟内跑完的 9 项同样失败，属于平台基线，不是本轮回归。
- 未运行：Windows 安装验收、真实网络吞吐、五轮性能脚本、浏览器端到端。

## 8. P0 实施记录（2026-11-16，第二轮）

范围仅 `native_shell/`；未改 IPC 协议、数据库表结构、UI。

### P0-2 任务内存控制令牌（已完成）

- 新模块 `native_shell/src/task_control.rs`：进程级注册表，键为任务控制文件路径，值为原子状态（run/pause/cancel/其他）。`run_task_with_throttle` 在任务运行期间注册（RAII，可嵌套，结束即注销）。
- `TaskPaths::set_control`（Core 的开始/暂停/取消/计划停止等全部入口）改为 `task_control::set`：先改原子量，再写控制文件，二者在同一把注册表锁内，避免与注册读取交错。
- 读者：`http_engine::read_control`（范围 worker、进度线程、curl 读取、FTP 轮询）、`media/hls.rs`、`media/dash.rs`、`sftp_engine.rs` 的 `read_control`、BT `check_torrent_control`、`ensure_publish_allowed` 均先读令牌；未注册路径保持原文件语义（引擎单测、子请求临时控制文件、直播音轨 `audio.control` 不受影响）。
- 已注册路径每秒至多重读一次文件：文件值与最近一次写入/读取不同则采纳（兼容外部工具写文件）；空内容（截断写入中）忽略；令牌无法表示的值（如 `finish`）回落到逐次读文件。
- 结果：运行任务的控制文件 open 从“每 worker 每 50 ms”降为“每任务约 1 次/秒”；新测试在 600 ms 停滞期内统计到 ≤2 次文件读取，暂停/取消在 <900 ms 内（实测远低于此）停止停滞的传输。

### P0-1 进度与 SQLite 解耦（已完成）

- `PersistentCore::handle(UpdateProgress)`：若任务仍处于上次写库时的同一活动状态/阶段且距上次写库 <5 s，则只更新内存快照、追加内存事件环并唤醒订阅者，不开 SQLite 事务；否则走原事务路径。任何其他持久事件（暂停、失败、完成、改设置等）写库时都带上最新内存快照，所以 `downloaded_bytes` 在状态变化时一定落库。
- 序号合同：易失事件仍按 `latest+1` 分配并进入事件环，`event_batch_after`/`events_after` 与通知游标不变。为保证“崩溃重启后不重用已发布序号”，序号超出已持久检查点时先执行 `CoreStore::reserve_event_sequence(序号+1024)`（只升不降的单行 UPSERT）；重启后 Core 从预留值继续，旧游标落在环外，客户端走既有 `resync_required` 重同步。
- `PersistentCore` 释放（`Drop`）时调用 `flush_volatile_progress` 把仍在内存中的快照写库；崩溃时最多丢失每任务 ≤5 s 的进度字节（续传数据本身由引擎检查点保证，不受影响）。
- 副作用：SQLite 任务日志中的进度行由每 200 ms 一行变为约每 5 s 一行（内存 `log_tail` 不变；状态变化行照常写入）。
- 进度文件：`write_progress`、`write_media_progress`、`write_active_ranges` 经 `task_control::publish_progress_file`，在已注册任务目录内至多每秒落盘一次，终态（非 downloading、媒体全部完成、merging）立即落盘；`download_worker::read_progress`、Core `progress_event` 的媒体计数、`connection_parts` 的活动区间读取优先用内存副本。删除活动区间文件、`cleanup_task_metadata`、删除任务目录前先丢弃内存副本；注销时仅当文件仍存在才补写最后一个值，不会复活已清理的文件。
- 未改：下载线程仍每 200 ms 取一次全局 Core 锁（现在不含 SQLite 事务）；`progress_event` 仍在锁内读 `native-engine.ranges.json` 与播放可用性检查；HTTP 进度线程仍每 200 ms 尝试读 `playback-byte-priority`。这些属于 P1“锁拆分”。

### P0-3 HLS 分片组提交（HLS 点播已完成）

- `save_segment` 增加 `sync` 参数：点播分片只 flush 到系统缓存后即原子改名发布；初始化分片、字幕、直播分片保持逐个 `sync_data`。
- `download_prepared_segments` 维护批次：每 32 个分片、64 MiB 或最早一个待提交分片满 1 s（以先到者为准）对整批 `sync_data`（Unix 另同步所在目录），成功后才写入 `vod_segments.json` 检查点。批次同步失败则整批从完成集合移除、不进检查点，续传时重下。
- 暂停/取消/失败：返回前先同步剩余批次，返回集合中的每个分片都已落盘，调用方照旧为它们写检查点，保持“停止保留已完成分片”的语义。
- 新增保护：即将重下的槽位先从检查点删除旧记录并保存，防止旧的同 identity/同大小记录为未同步的新文件担保。
- 崩溃语义比改前更好：改前点播在运行中途不写检查点，崩溃后本轮全部分片重下；现在可复用已提交批次；阈值在分片完成时检查，单个大分片、在途分片或停顿可能使未提交数据量/时间超过这些阈值。
- 未做：DASH 分片、把分片顺序追加到单一暂存文件、Windows 上 `MoveFileExW(WRITE_THROUGH)` 逐分片改名的开销（仍保留）。

### 验证

- `cargo check -p hls-native-shell`、`cargo clippy -p hls-native-shell --all-targets`：通过，本轮新增代码无 clippy 警告（剩余均为既有 Windows 专用死代码与 `fetch_range` 等参数过多警告）。
- `cargo test -p hls-native-shell --lib`（全量）：685 通过 / 10 失败 / 6 忽略（改前 667/10/6，+18 为新增）。10 项失败与改前完全同名，均依赖 Windows 系统代理/WinHTTP、DPAPI、`LOCALAPPDATA`、`C:\` 路径或计时（播放慢读者、BT 选择——其 tracker 请求走系统代理）。分项：`task_control` 8/8、`core_service` 21/21、`store` 14/14、`media::hls` 34/34（1 忽略为基准测试）、`http_engine` 89/89、`ftp_engine` 25/25。
- 新增测试 18 项：`task_control` 8 项；`core_service` 4 项（稳态进度只进内存且状态变化落库、5 s 间隔落库、模拟崩溃后序号不重用、正常释放冲刷）；`http_engine` 1 项（已注册令牌停止停滞传输且不轮询文件）；`media::hls` 3 项（批次上限、未同步批次不担保、重下槽位删除旧记录）；`download_worker` 2 项（直连多连接任务经内存控制/进度完成并注销令牌；协调器暂停经令牌到达停滞 worker 且控制文件仍写入）。既有测试 `persisted_progress_preserves_runtime_lifecycle_and_other_tasks` 原先断言“每次进度后 SQLite 与内存完全一致”，这正是 P0-1 要改变的合同，已改为：无易失任务时完全一致，否则状态/阶段一致且冲刷后完全一致。
- 未验证：Windows（`FlushFileBuffers` 需写权限打开、`MoveFileExW`、Defender 对 open 次数的实际影响）、真实网络吞吐与五轮性能脚本、多任务并发下锁竞争的实测、旧版 Presenter/Host 在 Core 重启后序号跳跃时的重同步表现（按 `resync_required` 合同推断应正常）。


## 9. P1 实施记录（2026-11-16，第三轮）

范围仅 `native_shell/`；未改 IPC 协议（事件名、序号与重同步规则不变；设置键列表新增一项为追加）、数据库表结构、UI。改前副本与 diff：`/workspace/work/hls/baseline-p1/`（工作区路径，不随源码发布）。

### P1-5 Core 锁热路径减负（已完成热路径部分）

- **进度信箱**（新增 `progress_relay.rs`）：`run_http_file`、`poll_media_progress`（HLS/DASH/FTP/SFTP/BT）的 200 ms 采样与本地复制的逐 MiB 进度改为 `post`：写入按 `Arc<Mutex<PersistentCore>>` 区分的信箱（每任务只留最新值），若距该 Core 上次批量应用 ≥150 ms，则 `try_lock` 一次并在同一次持锁中应用全部排队样本；锁忙即返回，样本留给下一次 `post` 或同步路径。效果：worker 线程从不阻塞在 Core 锁上；N 个活动任务的周期进度合计约 ≤6.7 次加锁/秒（原 5N 次）。
- **顺序保证**：所有应用都在持有 Core 锁时进行。同步路径（`mark_progress_speed`、`current_progress`、任务结束时的状态写入）在持锁后先 `flush_task_locked` 应用该任务排队样本，再执行自己的命令，因此排队的“下载中”样本不会落在之后的“校验/合并/暂停”之后；事件仍经 `CoreCommand::UpdateProgress` 同一处理函数，P0-1 的易失/写库策略不变。信箱用 `Weak` 绑定 Core，Core 释放后地址复用不会继承旧样本。
- **检查点缓存**（新增 `range_cache.rs`）：`save_completed_ranges` 原子替换成功后写入内存视图；`connection_parts`（`progress_event` 锁内绘制）、`playable_http_total`、播放增长文件 `downloaded_range_end` 改读缓存。进程内写入 1 s 内直接使用，之后与磁盘读入的条目一样按长度+修改时间校验（stat，不读文件），变化才重读；删除经 `range_cache::remove`，`forget_progress_dir`/`cleanup_completed_media` 丢弃整个任务目录的缓存；上限 512 条。引擎启动时的 `load_completed_ranges` 仍直接读文件（权威来源）。
- **边播边下优先级**（`media/preview.rs`）：`request_byte_priority` 写文件成功后更新内存值；HTTP 进度线程（原每 200 ms 读文件）与检查点节流判断改读 `byte_priority`/`has_byte_priority`，文件每任务至多每秒重读一次；任务清理时丢弃。传输结束时决定是否保留检查点的两处 `exists()` 仍读文件（一次性）。
- 未做：`PersistentCore` 内部分锁/独立写线程；`progress_event` 锁内仍有 `preview.m3u8` 存在性与输出文件长度两次 stat。

### P1-4 curl 子进程（部分：封顶）

- 评估：curl CLI 不能在运行中追加请求；`--parallel`/多 URL 只能预先给定全部范围，与 `RangeScheduler` 的动态领取、活动区间尾段切分、`Retry-After` 退避冲突，而且多个响应共用一条 stdout/一个头文件后难以逐响应做身份校验（ETag/Content-Range 起止与总长）再写盘。为不破坏浏览器身份下载，本轮不做进程复用。
- 实施（新增 `curl_slots.rs`）：`fetch_via_curl_impersonate` 在启动 curl 前按任务控制文件取名额，名额随 `CurlBody` 存活（子进程结束并回收后才释放）。默认每任务 4、全局 16，`HLS_CURL_MAX_PER_TASK` 可调；等待时每 50 ms 检查暂停/取消；同一线程已持有名额时不等待（重定向/重试与上一响应重叠时不自锁）；无任务的探测请求只计全局。
- 影响：浏览器身份任务实际并发连接 ≤4（原为任务连接数），少开进程与读线程；对必须走 curl 的站点，>4 连接的吞吐可能低于改前。Windows 未实测。

### P1-7 FTP 多连接（已实施，明文 FTP）

- 条件：连接数 >1（取任务连接数，上限 8）、非 FTPS、`SIZE` 与合法 `MDTM` 都有（与单连接续传相同的资源身份）、文件 ≥4 MiB、`FEAT` 返回 `REST STREAM`。否则走原单连接路径，命令序列与改前一致（原 25 项 FTP 测试不变通过）。
- 流程：首次分段运行时截断并 `set_len(size)` 预分配，删除单连接续传状态 `*.ftp-state.json`（其语义是“追加到文件末尾”）；完成表 `*.ftp-ranges.json` 绑定资源身份，`sync_all`+原子替换。分段长度 `size/(连接数×4)` 夹在 1–16 MiB。主连接（已登录）在调用线程运行，首段 `REST`+`RETR` 被接受后才在作用域线程中开辅助连接（各自登录、各自数据通道、继承限速上下文）；`REST` 被拒且无已登记数据时返回内部回落标记，`download_ftp_with` 以单连接从头下载。
- 每段：EPSV/PASV → `REST 起点` → `RETR` → 定位写入；非末段读够即关闭数据连接，再发 `NOOP` 并读取直到 200（期间接受 226/250/425/426/450/451）以重新同步控制连接；末段多读 1 字节确认 SIZE 边界并要求 226/250。结束、暂停、取消或出错时，已写部分先 `sync_data` 再登记，未完成的尾部放回队列；worker 只有在队列空且无在途分段时才退出，辅助连接失败（如 421）由其余连接接手；主连接出错即整体停止并返回该错误（暂停/取消语义与单连接一致）。
- 未做：FTPS 多连接（Schannel 会话复用需 Windows 实测）、活动分段的动态尾段切分、主动模式。

### P2-8（可选）TS→MP4 重封装

- 已有 FFmpeg 路径（`media/merge.rs`）。新增 `remux_ts_to_mp4`（`-f mpegts -dn -sn -c copy -movflags +faststart`，经既有 `run_ffmpeg_to_output` 暂存+原子替换）与设置 `hls_remux_ts_to_mp4`（默认 false）。仅对“全部为 TS、无初始化分片、无不连续标记”、原本直接拼接的点播生效：先拼接到 `merged.ts` 再重封装为 `merged.mp4`；FFmpeg 缺失时不启用，失败时把拼接结果原样改名为成品（不因设置导致下载失败），暂停/取消照常返回。

### 验证

- `cargo check -p hls-native-shell`、`cargo clippy --all-targets -p hls-native-shell`：通过，新增代码无警告（剩余为既有 Windows 专用死代码、`fetch_range`/`with_media_response` 参数过多等）。`cargo check -p hls-native-shell --tests --target x86_64-pc-windows-gnu`：通过（仅类型检查）。
- 全量库测试（Linux aarch64）：708 通过 / 10 失败 / 6 忽略（P0 后 685/10/6；+23 为新增）。10 项失败与 P0 记录同名同因（WinHTTP/系统代理、DPAPI、`LOCALAPPDATA`、`C:\` 路径、播放慢读者计时、BT 选择超时）。分项：`progress_relay` 4、`range_cache` 4、`curl_slots` 4、`media::preview` 2、`ftp_engine` 32（新增 7）、`http_engine` 89、`media::` 92（含新增 2，其中 1 项用系统 FFmpeg 实际重封装）。
- 新增测试要点：锁被占用时 `post` 不阻塞且样本稍后落地；8 个任务一个窗口只加锁一次；同步状态变化前先应用排队样本且之后无旧样本覆盖；Core 地址复用不继承样本；检查点缓存对未变文件只解析一次、外部改写/删除下一次读取即可见、进程内写入不触盘；字节优先级内存命中不读文件、外部改写 ≤1 s 可见、目录已删时不留内存值；curl 名额每任务上限、跨任务不互相阻塞、同线程重入、暂停解除等待；FTP 4 连接分段完整且每段一次 REST、无 REST STREAM/小文件/单连接不分段、REST 被拒回落单连接、辅助连接被 421 拒绝仍完成、暂停后完成表只含已写入的远端字节且续传不重复请求已登记字节；TS 重封装产出 `ftyp`、失败不覆盖旧成品、HLS 层无效 TS 时回落拼接结果。
- 需 Windows 验证：curl 封顶下浏览器身份站点的实际吞吐与进程数；FTP 分段对真实服务器（含 IIS FTP、vsftpd、ProFTPD、Pure-FTPd）的提前断开/`NOOP` 重同步行为与每 IP 连接限制；`sync_data`/`FlushFileBuffers` 在分段登记上的开销；多任务时 IPC 命令延迟的实测对比；打包 FFmpeg 的重封装与 Defender 扫描影响。

## 10. P2 实施记录（2026-11-17，第四轮）

范围仅 `native_shell/`（另含 `native_clients/src/native_host.rs` 的测试门控）；未改 IPC 协议与数据库表结构。改前副本：`/workspace/work/hls/baseline-p2/`（工作区路径，不随源码发布）。§9 中“未做：PersistentCore 内部分锁/独立写线程；progress_event 锁内仍有两次 stat”和“未做：FTPS 多连接”两项，本轮处理如下。

### 锁内文件 I/O 移出（已完成）

- 新增 `progress_facts.rs`：运行中任务登记路径；worker 在取 Core 锁/投递进度前收集 `progress_event` 所需事实（媒体分片数、是否可预览/播放、连接分布区间），锁内只读内存，连接图绘制为纯计算（`connection_parts.rs` 拆分绘制与读文件）。无新鲜事实（未运行或超过 2 s）时回落原地收集，结果不变。
- `task_throttle_context` 在加锁前解析路径；`reuse_duplicate` 在文件存在性检查前释放锁；`download_worker/paths.rs` 计算任务目录不再触盘。
- 仍在锁内（有意保留）：开始/暂停/取消命令写控制文件、创建任务目录——一次性命令，顺序重要，不在周期热路径。

### 单一进度写入线程（已完成，文件数据库）

- 新增 `progress_writer.rs`：5 秒一次的进度落库改由独立线程使用自己的 SQLite 连接写入；事件仍按易失进度事件发布，序号在发布前预留。写入仅在排队序号新于库中序号时生效，不会覆盖更新的同步写入，也不会复活已删除任务；日志行仅在更新生效时写。
- 状态/阶段变化、失败、完成、设置仍在锁内同步写。主连接改为 IMMEDIATE 事务，避免两个连接写同一 WAL 库时立即 busy。关闭时写入线程先清空队列再执行原有最终 flush。内存数据库保持原同步路径。
- 未做：`PersistentCore` 完整拆分。存储经约 175 处调用访问，置于独立互斥量后存在隐蔽自死锁风险；序号预留（每 1024 个事件一次）仍同步。

### FTPS 多连接（已实施，待 Windows 实测）

- `segmented_eligible`：TLS 不再排除分段；其余条件不变（>1 连接、SIZE+MDTM、≥4 MiB、REST STREAM、上限 8）。
- 每个辅助连接独立建立控制通道 TLS；各分段数据通道复用该连接自身的控制会话（满足 `require_ssl_reuse` 类服务器）；主连接首次数据通道 TLS 握手成功后才启动辅助连接。
- 回落：新增 `SegmentError::DataTls`；本次运行尚无分段开始即握手失败则回落单连接并删除分段表；已有分段成功后视作普通错误，重试从已登记区间续传；辅助连接 TLS 失败只退出该连接，其余连接接手。

### Linux 测试基线清零

- 原 10 项失败：WinHTTP/系统代理（3 项）改为非 Windows 下使用直连；`C:\Windows` 路径改为非 Windows 用 `/etc`；DPAPI cookie、站点凭据、`LOCALAPPDATA`、native host 推送等仅平台相关断言门控到 Windows，并为非 Windows 增加“拒绝明文存储/拒绝推送”的等价测试，Windows 下测试内容不变。
- 两处真实缺陷顺带修复：播放服务对不读取数据的客户端，Linux 下部分写超时后可无限占用连接名额——新增 `write_all_within`，每 64 KiB 块总预算 5 s；`reject_path_escape` 与 `av_scan` 现在同时按 `\` 切分，`..\Windows` 类路径不再绕过下载目录检查。

### 验证

- 全量库测试（Linux aarch64）：727 通过 / 0 失败 / 6 忽略（第三轮 708/10/6）。新增：`progress_facts` 3、`core_service` 3（写锁被占时进度更新立即返回；过期/迟到行被忽略且不复活已删任务；两连接并发写无 busy）、FTPS 门控与结果映射等。
- `cargo clippy --all-targets -p hls-native-shell`：本轮改动文件无警告（剩余为既有文件：`http_engine` 参数过多、`sleep_inhibit`、`updater` 与 Windows 专用死代码）。`cargo check --tests --target x86_64-pc-windows-gnu -p hls-native-shell`：0 错误 0 警告（仅类型检查）。
- 需 Windows 验证：FTPS 分段对真实服务器（vsftpd `require_ssl_reuse`、FileZilla Server、IIS）含 TLS 下提前断开与 NOOP 重同步；NTFS + WAL 下写入线程与主连接的锁竞争；慢速局域网/电视客户端的播放写预算；Windows 门控测试实跑。


## §11 第五轮（2026-11-17）

- **DASH 多 Period（点播）**：按 Period 解析 BaseURL 和时间范围，在首个 Period 选轨，后续匹配类型、语言、格式与码率。初始化段变化时，局部清单标记不连续并切换初始化段，由 FFmpeg 无重编码封装。编码族变化、选定轨道缺失或整文件 SegmentBase 跨 Period 时会报错。
- **DASH 多 Period（直播）**：刷新后按 Period ID 或明确的开始时间继续跟踪。分片 URL 和字节范围在同一 Period 内去重，切换 Period 时为音视频标记各自的边界。回归使用真实音视频验证滑动窗口、旧清单、共享及独立初始化段，逐帧比较封装后的时间戳和校验和。字幕目前只保存录制起始 Period 的轨道。参考 [DASH-IF 清单更新约定](https://dashif.org/Guidelines-TimingModel/#mpd-updates)。
- **无 indexRange 的 SegmentBase**：整个 BaseURL 作为一个分段下载，时长取 Period 时长；外置 RepresentationIndex 与嵌套 SIDX 仍拒绝。
- **FTP 主动模式**：EPSV/PASV 被拒或被动端口连不上时，自动在控制连接本地地址上监听并发送 EPRT（IPv4 不支持时回落 PORT）；只接受来自控制连接对端 IP 的数据连接，30 秒超时并响应暂停/取消。同一任务内记住主动模式，分段辅助连接直接使用。FTPS 主动模式在 accept 后建立数据 TLS（仅编译验证）。
- **curl 浏览器身份分支**：确认已是有界管道边收边写，响应头校验先于写入，逐块限速与进度；补 5 项测试（退出前已落盘、限速、中途取消回收进程与名额、身份不符不写入）。
- Clippy：清理 http_engine / sleep_inhibit / updater 的既有警告。
- Linux 全量 lib 测试 740 通过 / 0 失败 / 6 忽略；Windows 目标 `cargo check --tests` 0 错误。
