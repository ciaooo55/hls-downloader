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

## 尚未完成的验收与差异

没有操作个人桌面或浏览器，没有运行原版 IDM。常规回归中的忽略项包含交互式 UAC、Sandbox 传输和真实 FFmpeg 等条件场景，不能算作通过；本次显式运行的两个 FFmpeg 用例结果单列于上节。

本轮尚未执行当前源码在独立环境的真实浏览器/Host/Core 接管、安装启动与升级、所有媒体变体和音画同步、五轮修改前后性能及原版 IDM 同条件对照。已实际验证上节的两秒视频及双轨夹具，但不能推广为所有媒体或所有音画同步场景已验收。性能脚本已有五轮采样和中位数输出，但不能据此宣称性能回退低于 5% 或达到 IDM。Firefox 永久安装仍需 Mozilla 签名。

FTP 多连接/主动 PORT、DASH 多 Period、无 indexRange 的 SegmentBase 及外置索引尚未实现。嵌套索引沿用已确认的原版拒绝行为；畸形数据校验不作为待实现功能。SegmentList 显式字节范围及带 indexRange 的非嵌套 SegmentBase/sidx 现已实现。现有 FFmpeg 封装、DOM 浮窗、持久回执及额外协议功能按计划保留。

## 源码与发布文件的边界

`main` 保存当前维护的产品代码、测试、配置和必要文档；Git 提交历史、版本标签与已有 Release 保留。正式安装包和配套浏览器扩展以 Release 附件为准，不把本轮开发包覆盖到已发布的 v7.0.6。

`.tool-cache/`、`artifacts/` 和依赖输出是本地忽略目录；下载文件、任务恢复数据、用户自有备选 UI 和加密凭据备份不能作为编译垃圾删除。清理记录保留实际结果，不将被策略拒绝的删除记录为成功。

2026-11-15 整理结果：GitHub 只保留 `main` 这一条维护源码分支；10 个已发布 Release、94 个附件及 1 个未发布草稿保持原样。根目录 `site-cache/` 纳入项目忽略规则。旧 Rust target、Compose 构建目录和扩展输出此前已删除，后续编译按项目配置重建缓存。用户随后执行清理脚本，已复核 31 个指定旧目标均不存在，包括 `.tool-cache/idm-port` 和 `.tool-cache/test-tmp/idm-port`；之前的策略拒绝记录不再代表当前残留。依赖缓存、用户数据和原始验证证据保留。详细本地记录为 `artifacts/workspace-cleanup-20261115/`。
