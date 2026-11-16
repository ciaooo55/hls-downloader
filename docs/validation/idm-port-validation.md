# 发布后源码验证记录

日期：2026-11-16。正式发布版本仍为 [v7.0.6](https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.6)，受测发布源码为 `1c1dc06b7dc63e9c728ae75e5dcff9772781cd7d`。本文记录在基线 `7636ad417afec41cce81cf078af0a40598db1fff` 上完成的后续源码修改，不能替代新成品的完整发布验收。

## 已落地修改

- Firefox 请求发送前接管同源、带 `download` 属性的无后缀链接，保留原始签名 URL、凭据和网页文件名；普通导航不因下载日志读取失败被取消，丢回执继续按原请求 ID 查询。
- HTTP 206 校验声明终点和 Content-Length，读取范围不越过响应声明边界；合法短范围继续请求后续字节，矛盾元数据不能进入可靠检查点。失败和整包转交先记录停止状态，再释放活动区间。
- HLS 首个有效行要求完整 `#EXTM3U`，隐式字节偏移只能延续相邻同资源；部分分片与完整分片分别记录偏移。
- DASH 使用项目已有 roxmltree 解析 XML，处理命名空间、注释、CDATA、签名 URL 和真实父节点继承，删除被替代的字符串 XML 扫描。
- 已完成任务在发布标记不可读或被缓存清理移除时继续使用持久化成品路径，不误指向其他任务的同名文件。
- 2026-11-15 在 `ddcb2c6e` 基线上补入 FTP EPSV/IPv6 与受限 PASV 回退；修复 PASV 跳过坏字段、接受多余字段和端口饱和计算。协商失败不覆盖原文件，REST 仍紧邻 RETR。
- 同日贯通 DASH SegmentList 显式字节范围的解析、初始化段、视频、音轨、字幕、直播去重、时长累计和断点身份。通过现有 HTTP 凭据和重定向路径，严格校验范围响应，以 64 KiB 缓冲落盘，校验并同步成功后才记检查点。
- 分片身份变化时先持久失效旧检查点，再写新数据；修复失败传输恰好写满旧长度后，重启可能误用旧身份的问题，并移除不再调用的旧检查点查询方法。
- 在 `5206bb81` 基线上接入 DASH SegmentBase.indexRange / SIDX v0、v1：继承和覆盖初始化范围，仅展开选定视频、音轨及启用的字幕。索引结果进入同一范围下载、断点、预览与封装路径；索引请求显式携带任务控制文件，逐块读取时检查暂停、取消和限速。坏字幕索引保留既有可选附件语义，不能使完整音视频任务失败。

原版函数、调用方、行为约束与 HLS 生产路径见 [逐模块对应表](idm-source-audit.md#逐模块实施2026-11-14)。没有修改 UI、迁移持久数据、接入恢复 C 引擎或 IDM 私有 DLL。

## 实际执行结果

下表为上一批浏览器/HTTP/HLS/XML 改造的验证快照；FTP/DASH 后续修改的结果单列在下一节，不能将旧构建文件当作本次产物。

| 检查 | 结果 |
| --- | --- |
| Core、client、protocol 完整 Rust 库回归 | 723 通过，14 忽略，0 失败；分别为 664、48、11 项通过 |
| 删除无生产调用的包装函数后，下载工作流子集回归 | 103 通过，1 忽略，0 失败；属于上述全套的子集，不重复累计 |
| 扩展 WXT 类型生成、TypeScript 检查和 Vitest | 54 文件、606 测试通过 |
| Core Release 构建 | 成功，无编译警告 |
| 最终 Core `--self-test` | 退出码 0，输出 `HLSDownloaderEngine 7.0.6 self-test ok` |
| Chromium、Firefox 及 Firefox 源码 ZIP | 构建和打包成功，Firefox 未签名 |
| 四份修改的 Python 验收脚本 | AST/语法编译通过，未执行产品运行流程 |
| 项目和参考输入身份 | 核验 333 份项目代码/配置与 21 份参考文件；Firefox 源码包中 74 份对应文件与工作区一致 |
| `git diff --check` | 通过 |

完整 Rust 回归后仅删除旧包装函数并让测试直接调用生产实现，随后重新运行受影响的下载工作流子集并重建最终 Release。原始日志、输入快照和开发产物的本地记录目录为 `artifacts/idm-port-20261114/`，不上传到源码仓库。

本地开发 Core 文件为 15,083,008 字节，SHA256 为 `ee5287a6e6432fdec8f9d75201c9798beed7565866f564a6c10ad37db94d79c6`。该文件构建于下述 CI 借用清理之前，仅是当时的 Core EXE，不能作为最新源码的完整安装包或整个产品占用的度量。

首次同步提交 `de151ad7` 的 GitHub 检查中，Core 测试、客户端测试、构建及依赖边界检查通过；Clippy 拒绝了 DASH XML 改造后遗留的三处 `&&str` 多余借用。现已按诊断移除这三处借用，本地 `cargo fmt --all -- --check` 通过。同步后的自动测试、构建和 lint 结果以 [v7 CI](https://github.com/ciaooo55/hls-downloader/actions/workflows/ci.yml) 对应源码提交为准。

## 2026-11-15 FTP / DASH 后续验证

修改基线为 `ddcb2c6e`。原版静态复核再次检查 `0x00577f50` 的 PASV/REST 状态分派、`0x00574f60` 的六字段 PASV 回复和失败分支、`0x00573fa0` 的 REST 回复分支，以及 `0x1003fa40` 的 fMP4 box 边界循环。EPSV 和 DASH XML 的新增协议处理分别核对 RFC 2428 与 ISO MPD schema，不冒充原版已有同名实现。

| 实际执行 | 结果与边界 |
| --- | --- |
| FTP/SFTP 定向库回归 | 22 通过，含 IPv4/IPv6、EPSV 成功、500/501/502/504 回退、临时/权限/畸形回复、PASV 坏字段、保留原文件、续传及停止 |
| Core、client、protocol 最终完整库回归（单线程） | 730 通过、15 忽略、0 失败；Core 671、client 48、protocol 11 项通过。包含范围下载拒绝整包/错误边界/编码/长度/截断/超长响应，以及写满旧长度后失败、重启和范围回切的检查点回归 |
| 真实 FFmpeg 指定忽略用例 | 单独显式运行 `real_ffmpeg_ranged_fmp4_preserves_decoded_frames`，1 通过；合法 2 秒源视频，范围下载字节一致，封装后 20 帧解码哈希一致 |
| Clippy（Core 全 targets，项目 CI 同样的结构性例外） | `-D warnings` 通过，无新增警告 |
| Core 开发构建及 `--self-test` | 构建成功，退出码 0，输出 `HLSDownloaderEngine 7.0.6 self-test ok`；使用内存数据库，不启动桌面、托盘或浏览器 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |

最终日志为 `artifacts/idm-protocol-20261115/rust-regression-final.log`、`clippy-final.log`、`core-build-final.log`、`core-self-test.log`，真实媒体日志为同目录 `real-media.log`。常规完整库回归仍将真实 FFmpeg 用例列为忽略，因此单独列出显式执行结果，不重复累加。没有操作个人桌面/浏览器，没有运行 IDM。浏览器插件源码本次未改动，未重复运行未受影响的 606 项扩展测试。

## 2026-11-15 SegmentBase / SIDX 后续验证

只读核对原 `0x0055a250 → 0x005d0250 → 0x005d0180 → 0x005cfef0 → 0x005cfd80` 的实际调用链，确认索引表、box 长度、偏移锚点和嵌套引用拒绝路径。没有运行 IDM 或接入恢复 C。具体原版行为与 Rust 适配差异见 [源码对应表](idm-source-audit.md#逐模块实施2026-11-14)。

本次支持单 Period、带 indexRange 的 SegmentBase，SIDX v0/v1、32/64 位 box、父级继承、初始化范围、媒体范围与时长。限制为整个索引请求最多 3,000,000 字节；拒绝未知版本/flags、零 timescale、零长度或时长、截断/溢出、size=0、无 indexRange、外置 RepresentationIndex 和嵌套索引。前者是 HLS 请求限制，原版对应常量仅为缓冲预算，不能说两者完全相同。

真实媒体通过 `download_dash_selected` 正式入口下载：FFmpeg 生成两秒 MPEG-4 视频和 AAC 音轨及真实 global SIDX，经本地 HTTP 范围服务、选轨、初始化段、分片、检查点与最终封装。逐字节核对已下载范围，重启后仅重新读取清单和索引，未重下初始化段与媒体。未选视频和禁用字幕不请求；启用的损坏字幕索引不阻断音视频任务。

首次真实音频断言发现最后一帧相差 256 个样本。完整原文件独立重封装同样将 AAC 终端填充从 1024 样本裁至声明的 768 样本，和 HLS 成品逐帧一致；不是范围下载丢数据。最终回归同时验证下载字节，以及与独立整文件重封装相比的全部音频样本哈希、时间基和时间戳，没有忽略末帧或放宽数据断言。结果为 20 帧视频、95 帧音频完全匹配，仅代表该双轨夹具。

实际执行的两个真实 FFmpeg 用例均通过（原 SegmentList 视频用例和新增 SegmentBase 双轨用例）。原始日志、首次失败、独立重封装对照及最终结果保留在 `artifacts/idm-sidx-20261115/`，包括 `real-media-initial.log`、`aac-padding-comparison.json` 和 `real-media-final.log`。这些用例在常规库回归中仍明确忽略，必须配置 `HLS_FFMPEG` 后显式运行，不能把忽略算作通过。

| 实际执行 | 结果与边界 |
| --- | --- |
| Core、client、protocol 完整库回归（单线程） | 735 通过、16 忽略、0 失败；Core 676、client 48、protocol 11 项通过。日志 `rust-regression-final.log` |
| 最终 DASH 全部用例（含两个真实 FFmpeg 用例） | 33 通过、0 忽略、0 失败；包含完整库回归后按 Clippy 建议进行的等价固定数组分组调整。日志 `dash-tests-final.log`，不重复累计至上一行 |
| Clippy（Core 全 targets，与 CI 相同的结构性例外） | `-D warnings` 通过；日志 `clippy-final.log` |
| 最新 Core 开发构建及 `--self-test` | 构建成功，退出码 0，输出 `HLSDownloaderEngine 7.0.6 self-test ok`；日志 `core-build-final.log`、`core-self-test.log`。内存自检，不启动桌面、托盘或浏览器，不代表整套安装包验收 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |

本轮生成的测试音视频、HTTP 夹具及空临时目录已清理，原始失败日志与解码对照保留；没有测试常驻进程。开发 Core 留在项目配置的 Cargo target 内，身份见同证据目录的 `run-summary.json`，不覆盖已有 Release，也不将缓存或本地证据提交为产品源码。

## 2026-11-16 FTP/FTPS 停止与长度边界

修改基线为 `8b77a98c`。只读复核 `FTPConnection` 析构及控制/数据 socket 清理链，依据和没有核实的原版调用关系见 [源码对应表](idm-source-audit.md#ftpftps-通道生命周期2026-11-16)。没有执行 IDM，也没有接入恢复 C。

旧代码只在数据读取期间观察任务停止，隔离回归实际复现了等待欢迎消息时写入 pause、两秒内仍不返回。生产路径现将控制检查置于底层 TCP 读写：100 ms 超时只用于检查停止，在内部续等至原有 30 s 单次 I/O 上限；Schannel 接收到的是正常数据或真实终止错误。热路径不逐块读控制文件，最多每 100 ms 读取一次；数据继续在任务调用线程下载并沿用限速上下文，移除原读取线程、跨线程结果通道及超时后分离线程。

SIZE 已知时在写入前拒绝超界响应，短响应不再等最终回执才失败，SIZE=0 也验证 EOF。未知长度保留流式下载；同步文件且收到 226/250 后才清除身份状态。保留 SIZE+MDTM 续传身份、REST 紧邻 RETR、被动 peer 限制及 FTPS 凭据句柄复用。

| 实际执行 | 结果与边界 |
| --- | --- |
| FTP/SFTP 定向库回归 | 26 通过，0 失败；覆盖控制阶段、AUTH/TLS 握手、数据读取及最终回执的 pause/cancel，预停止不连网、不改原文件；保留现有身份续传、EPSV/PASV 与 SFTP 回归 |
| FTP 数据边界夹具 | 11 个实际 TCP 传输场景随上述定向回归执行：准确长度、跨轮询间隔的慢控制回复和慢数据、226/250、超长/跨缓冲超长、短响应/短响应无完成回复、空文件/空文件超长、未知长度和 426；失败不写超界、不移除已知身份，不重复累计为额外测试数 |
| Core、client、protocol 完整库回归（单线程） | 739 通过、16 忽略、0 失败；Core 680、client 48、protocol 11 项通过；日志 `rust-regression-final.log` |
| Clippy（Core 全 targets，与 CI 相同的结构性例外） | `-D warnings` 通过；日志 `clippy-final.log` |
| Core 开发构建及 `--self-test` | 构建成功，退出码 0，输出 `HLSDownloaderEngine 7.0.6 self-test ok`；日志 `core-build-final.log`、`core-self-test.log`。内存自检，不创建桌面、托盘或生产 IPC |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |

完整库回归后补入慢响应夹具，并重跑受影响的 FTP/SFTP 定向回归；生产代码保持不变。原始失败、诊断及最终日志位于 `artifacts/idm-ftp-lifecycle-20261116/`。本轮没有五轮修改前后或原版性能数据，不能由少一个线程推断具体 CPU/内存/吞吐提升；新增完成时单次 `sync_all` 保证成功返回前同步数据，其耗时尚未量化。

本轮未更改浏览器插件、UI 或数据库，没有操作个人桌面/浏览器。DNS 解析和仍在进行的 TCP connect 尚不能由底层读写轮询中断；FTP 多连接/主动 PORT 仍未实现，不把这次修复描述成整个 FTP 已与 IDM 等效。

编译与测试进程已结束。自动审批以 `blocked by policy` 拒绝删除本轮测试临时目录 `.tool-cache/test-tmp/idm-ftp-lifecycle-20261116/`，其中保留早期失败的 `data-pause` 夹具；该忽略目录未提交，不将其记录为已清理。原始验证日志继续保留在上述 artifacts 目录。

## 2026-11-16 分块 POST 与 HLS 流式传输

修改基线 `205ea099`。先更新既有架构计划第六节，再只读核查原插件 `fc` 的调用方、`0x005f37c0 → 0x00567020 → recv` 和容量记录函数；没有执行 IDM、操作个人桌面/浏览器或引入恢复 C。具体源码约束与未核清链路见 [源码对应表](idm-source-audit.md#分块-post-与-hls-有界读取2026-11-16)。

插件补齐多 raw 块的精确 POST 正文；任一块缺失或总长超过 128 KiB 全部拒绝，保留原 MIME 类型限制、requestId/frame、签名地址和凭据。旧版新增两项反例实际失败；现从注册的浏览器事件到后台 offer 消息验证原始正文与 Authorization，而非只测辅助函数。

普通 HLS 分片、初始化段、字节范围段和辅助分片改为有界读取及暂存输出；AES-128 复用已有 AES 算法，跨块 CBC 状态和末块 padding 校验在流中完成。网络错误可重试，本地写盘/解密错误不再请求；正文、长度、解密、flush/sync 和原子替换成功后才返回完成，失败保留原文件。原有整段包装不再编译进生产。清单/密钥仍受原大小限制，读取时逐块执行限速；控制轮询沿用 50 ms 热路径预算。

初次回归两项续传夹具在首段正文尚未送出时就暂停，却假定该段已经完成；新路径显式携带控制文件，按暂停中断该请求。现将暂停屏障移到第二段请求，先确保首段已持久发布，再验证第二段中断、首段不重下、VOD/live 恢复字节完全一致。另新增正文未结束前确有暂存输出、传输中暂停/取消保留旧文件的四个普通/Range 场景；没有通过关闭取消检查来迁就旧测试。

| 实际执行 | 结果与边界 |
| --- | --- |
| 插件完整类型生成、类型检查与 Vitest | 54 文件、609 通过；`plugin-full-final.log` |
| Chromium / Firefox 生产构建 | 成功；展开后分别约 794.09 / 795.45 kB；不是已签名商店包；`plugin-build.log` |
| HLS 定向回归 | 最后去除无用响应 sidecar 后，31 通过、1 基准用例忽略；`hls-final-sidecar.log` |
| AES / 哈希定向回归 | 13 通过；包含独立 .NET AES/CBC/PKCS7 向量、1～33 字节输入分块、截断/坏 padding；`crypto-targeted.log` |
| Core、client、protocol 完整库回归（单线程） | 741 通过、17 忽略、0 失败；分别 682、48、11 通过。最后只改 HLS Job 的空 output/progress 字段以移除无用响应 sidecar，随后重跑上列 HLS 定向、Clippy 及 Core 构建。新增忽略项是需显式运行的五轮传输基准；`rust-regression-final.log` |
| 最终 Clippy（Core 全 targets，与 CI 相同的结构性例外） | `-D warnings` 通过；`clippy-sidecar-final.log` |
| 最终 Core 开发构建及自检 | 构建成功，`--self-test` 退出 0，输出 `HLSDownloaderEngine 7.0.6 self-test ok`；`core-build-sidecar-final.log`、`core-self-test.log` |
| 隔离 Core 真实 HLS 媒体 | 明文 TS、AES-128 TS、fMP4、single-file fMP4 Range 四项通过；每项 30 帧视频、142 帧音频的完整 framemd5 行（时间戳、大小和哈希）与 FFmpeg 独立读取原清单的结果相同，双轨严格解码通过；`real-media-range.json` |

真实媒体验收由 `scripts/validation/verify_hls_streaming.py` 生成合法三秒 H.264/AAC，经独立数据目录、loopback TCP 和禁弹窗 Core 下载。TS/m4s 使用 chunked 正文且每块 1003 字节，覆盖 AES 非块对齐读取；single-file 场景严格要求 206、四个预期 Range 及精确 Content-Range/长度。全部 GET 要求原始签名 query、Authorization 和 Cookie；无拒绝请求、额外重下、`.part` 或无用 `.response.json` 残留。真实夹具成功后已清除，报告保留。该范围不含浏览器、桌面、原版 IDM 或安装包。

最终本地开发 Core 为 `.tool-cache/build-cache/cargo-target/debug/hls-downloader-engine.exe`，29,220,864 字节，SHA256 为 `02b02897bc0c1fe7d615fa9a39e0e05c8da26f6c7888edeed83f3514dba69dc7`。这是开发构建，不代表新发布安装包或发布体积。原始日志、初次失败、基准和输入身份位于 `artifacts/idm-streaming-20261116/`；忽略项不算通过。

### HLS 资源与持久化成本

用同一机器、同一 Cargo 开发测试配置和 32 MiB 普通 TCP HLS 实体各运行五次，采样时无本批构建或其他测试。每轮新进程，逐字节验证成品；CPU/峰值内存包含服务器夹具和成品校验，传输时间包含成功返回所需的写盘。测试不启动浏览器、UI 或 IDM。原始结果在 `before/benchmark.json` 和最终去除 sidecar 后的 `after-final/benchmark.json`，各含受测 EXE 哈希。前后顺序采样，未交错 A/B，且新旧持久化合同不同，不能当作隔离网络吞吐比较。

| 五轮中位数 | 修改前 | 修改后 |
| --- | ---: | ---: |
| 峰值工作集 | 78,647,296 B（约 75.00 MiB） | 12,582,912 B（12.00 MiB） |
| 峰值提交内存（Windows PeakPagefileUsage） | 103,649,280 B | 3,375,104 B |
| 32 MiB 完成时间 | 0.493900 s | 1.621705 s |
| 完成吞吐 | 64.790 MiB/s | 19.732 MiB/s |
| 全进程 CPU 时间 | 0.062500 s | 0.015625 s |

工作集约降 84%，但完成吞吐回退约 69.5%，且低于计划的 20 MiB/s 本地下载门槛，**本场景性能未通过**。存储阶段另做五轮诊断：32 MiB 本地写入、`fsync`、原子替换的中位数分别约 0.0121、0.6492、0.0013 s，记录在 `io-phases.json`。新路径增加成功返回前的数据同步；旧路径只将缓存写交给操作系统，不保证检查点对应数据已同步。同步是已经测出的成本，但此诊断不是最终下载的逐阶段跟踪，不能将全部回退精确归因于它。先前同为 1 MiB 写缓冲的 `after/` 吞吐中位数为 37.002 MiB/s，存在明显批间波动，不能选择较快一批代表最终结果；初始 64 KiB 结果保存在 `after-initial/`。不移除可靠写入保证来换数值，也不据此宣称网络提速。

CPU 受 Windows 时钟粒度影响，不据此宣传降低百分比。上述单分片数据不能推广为多连接、真实 HTTPS、整机占用、启动或完整安装包表现，也没有达到 IDM 的运行对照证据。浏览器身份 curl 分支仍先整段落临时文件，本批只避免其后整段内存复制，该分支的网络过程限速/背压仍未完成。

### 本批临时资源

没有本批常驻 Core、FFmpeg、编译或测试进程。成功的真实媒体及基准夹具已清除，验证日志保留。两份早期失败夹具的递归清理在执行前被自动审批以 `blocked by policy` 拒绝，仍在 `.tool-cache/test-tmp/idm-streaming-20261116/hls-auth-live-resume-10904-4440` 和 `hls-auth-vod-resume-10904-4445`，未改用其他工具绕过。只读快照见 `failed-fixture-state.json`，实际清理状态见 `cleanup.json`；不称临时目录已全部清空。

## 尚未完成的验收与差异

没有操作个人桌面或浏览器，没有运行原版 IDM。常规回归中的忽略项包含交互式 UAC、Sandbox 传输和真实 FFmpeg 等条件场景，不能算作通过；此前显式运行的两个 DASH FFmpeg 用例和本批四种 HLS 真实媒体结果分别列在对应小节。

当前源码在独立环境的真实浏览器/Host/Core 接管、安装启动与升级、所有媒体变体和音画同步、其他完整性能场景及原版 IDM 同条件对照仍未执行。已实际验证此前两秒 DASH 视频/双轨及本批三秒 HLS 四种双轨夹具，不能推广为所有媒体或所有音画同步场景已验收。本批 HLS 单分片已有五轮前后采样且发现回退，不能宣称性能回退低于 5% 或达到 IDM。Firefox 永久安装仍需 Mozilla 签名。

FTP 多连接/主动 PORT、DASH 多 Period、无 indexRange 的 SegmentBase 及外置索引尚未实现。嵌套索引沿用已确认的原版拒绝行为；畸形数据校验不作为待实现功能。SegmentList 显式字节范围及带 indexRange 的非嵌套 SegmentBase/sidx 现已实现。现有 FFmpeg 封装、DOM 浮窗、持久回执及额外协议功能按计划保留。

## 源码与发布文件的边界

`main` 保存当前维护的产品代码、测试、配置和必要文档；Git 提交历史、版本标签与已有 Release 保留。正式安装包和配套浏览器扩展以 Release 附件为准，不把本轮开发包覆盖到已发布的 v7.0.6。

`.tool-cache/`、`artifacts/` 和依赖输出是本地忽略目录；下载文件、任务恢复数据、用户自有备选 UI 和加密凭据备份不能作为编译垃圾删除。清理记录保留实际结果，不将被策略拒绝的删除记录为成功。

2026-11-15 整理结果：GitHub 只保留 `main` 这一条维护源码分支；10 个已发布 Release、94 个附件及 1 个未发布草稿保持原样。根目录 `site-cache/` 纳入项目忽略规则。旧 Rust target、Compose 构建目录和扩展输出此前已删除，后续编译按项目配置重建缓存。用户随后执行清理脚本，已复核 31 个指定旧目标均不存在，包括 `.tool-cache/idm-port` 和 `.tool-cache/test-tmp/idm-port`；之前的策略拒绝记录不再代表当前残留。依赖缓存、用户数据和原始验证证据保留。详细本地记录为 `artifacts/workspace-cleanup-20261115/`。
