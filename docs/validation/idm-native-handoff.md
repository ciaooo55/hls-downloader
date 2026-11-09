# IDM 原生接管链静态核查（2026-11-04）

本轮按用户授权查看 `A:\Ubuntu\逆向\新建文件夹`，延续浏览器插件参考工作，重点核对原生接收、裁决回传和用户取消的关系。只读取参考程序，没有启动、修改或注册 IDM，没有修改项目生产代码。

## 结论与边界

- 已从插件的 type 13 接管消息追到 `IDMNetMon.dll` 的请求对象、COM 调用、原始主程序的接口实现，再连接到返回插件的 type 1 裁决；不再仅依赖插件或协议字符串推断。
- 一条已核对的 COM 路径将桌面方法的 `S_OK`（HRESULT 为 0）转换为裁决 `3`，其他返回值转换为 `1`。插件收到原 type 13 请求的裁决 `3` 才取消浏览器网络请求。
- 主程序接口存在异步投递分支：投递主窗口消息后即可返回 `S_OK`，无须等待用户操作确认窗。因此裁决 `3` 可以表示桌面已接收并派发，不能解释为用户已点击开始或文件已下载成功。
- 这支持项目保留“收到可呈现的持久接收回执后由桌面负责，用户取消结束本次下载”的合同。项目的持久记录和按原业务 ID 恢复查询是自身实现；本轮没有证明 IDM 也提供同样的恢复保证。
- 主程序另有同步 `SendMessageA` 分支，返回值参与 HRESULT 决策；尚未完整连接其所有 UI 按钮、模式标志和最终回包。不能把上述异步分支推广为所有设置下的行为，也不能宣称已完整还原 IDM 本体。

以上属于原始二进制静态控制流证据，没有进行 IDM 本体运行验证；异步投递也不等于已经持久落盘。

## 样本身份

| 组件 | 实际读取文件 | SHA-256 |
| --- | --- | --- |
| 主程序 | `破解idm\app\IDMan.exe.before_unlock_patch` | `3b5dc9ce53eb31933effa3fd973bec068a78e0f8c1d15a919b081f0c4d9325ba` |
| 消息宿主 | `_cracked\portable\IDMMsgHost.exe` | `dba25a49adb88f675db26d2dc7a0fa9d4a5db2326858cb9d2515f6f34b8e0b65` |
| 网络监控模块 | `_cracked\portable\IDMNetMon.dll` | `79cf944e6b42435c8d90a756efc74bd1ee3bffc9040f2784cdafa32c0847cd03` |

表中路径相对于授权参考目录。主程序使用名为 `before_unlock_patch` 的补丁前备份，没有把 portable 目录中另一个哈希的主程序混用为同一样本，也未与官方安装包逐字节校验。目录中的重建项目 `idm-ext-re`、`dlbridge` 不作为原版行为的证明。现有文件并非经运行确认的同版本安装组合，跨组件关联依据是协议、接口 GUID 和实际函数匹配。

全部地址为 PE 首选映像基址下的静态 VA：主程序和宿主 `0x00400000`，网络模块 `0x10000000`。

## 关键证据链

### 接管入口与回执

1. 扩展 `src/idmmzcc3-6.43.1/deobfuscated/background.js` 的 `Y/sc` 打包并发送 type 13，保存 Promise 的完成函数。`Fa` 对 type 1 回执取原消息记录，原类型为 13 且裁决为 3 时调用 `X(..., true)`；`X` 完成 `{cancel:true}`。
2. 网络模块的接收分派在 `0x10035165` 以 type 减 1 查 byte table `0x10035578`，再查 jump table `0x10035524`。原始表字节给出 **type 13 → `0x10035285` → `0x10037420`**。
3. `0x10037420` 解析 `%u:%u:%lld:%u:%u:%u:%u:%lld`，按请求种类建立对象。HTTP 对象的主虚表为 `0x1006adb0`，FTP 对象为 `0x1006ad44`；虚表 `+0x40` 处理候选，`+0x48` 均为 `0x10021300`，负责完成裁决。
4. `0x1003770e` 将尚未完成的对象交给待处理集合。COM 工作线程通过 `PostThreadMessageW(0xb1d7, requestId, verdict)` 报告结果；消息循环 `0x10013be0` 按 requestId 查找对象，调用 `0x1003d620`，再调用对象的 `+0x48`。
5. `0x10021300 → 0x100147c0 → 0x1003be90 → 0x1003af30` 的插件分支生成 type 1 回包，携带原请求序号及裁决。`0x10034b50` 使用 `MSG#%u#%u#%u#%u` 序列化。`0x100147c0` 还有驱动来源的分支，不能把其驱动裁决字节与插件字段混为一谈。

type 6 进入的 `0x10036a20` 解析窗口几何参数，不是上述接管候选入口。主程序其他位置的字段编号 13 也不能直接当成消息 type 13。

### 桌面接收与裁决值 3

网络模块 `0x10018b80` 创建工作线程 `0x10019010`。线程调用：

- CLSID：`ac746233-e9d3-49cd-862f-068f7b7ccca4`，位于网络模块 `0x10064330`。
- IID：`94d09862-1875-4fc9-b434-91cf25c840a1`，位于网络模块 `0x10064340`。
- COM 接口虚表 `+0x10`，调用点 `0x100190d2`。

调用返回后，`0x100190db` 检查 HRESULT；`sete` 加 `eax*2+1` 明确计算出 `HRESULT == 0 ? 3 : 1`，然后通过 `0xb1d7` 回到请求集合。COM 创建失败在该线程内有单独退出路径，不能宣称所有启动失败都会由此立即返回 `1`。

原始主程序中相同 IID 位于 `0x006d9658`，`QueryInterface` 的 `0x00425e51` 比较它并返回本对象。构造函数 `0x00425e00` 设置虚表 `0x006d96cc`；虚表 `+0x10` 指向 `0x0042cf20`。该方法整理参数后在 `0x0042d56c` 调用 `0x0042a180`，并在 `0x0042d657` 将其结果作为 HRESULT 返回。

### 主窗口异步与同步分支

`0x0042a180` 整理下载参数后存在以下可确认分支：

| 地址 | 实际行为 | 含义 |
| --- | --- | --- |
| `0x0042c592` | 将局部结果初始化为 1 | 异步路径没有等待 UI 返回值 |
| `0x0042cba7` | 检查请求标志 `0x01000000` | 决定此处分支使用同步或异步投递；未给该标志臆造产品名称 |
| `0x0042cbcf` | `PostMessageA(hwnd, 0x2b3e, 0, request)` | 异步交给主窗口处理 |
| `0x0042cc97` | 另一条路径也调用相同 `PostMessageA` | 同样存在无需等待 UI 的路径 |
| `0x0042cbb8`、`0x0042cd2f` | `SendMessageA`，将返回值写入局部结果 | 同步路径可受主窗口处理结果影响 |
| `0x0042cdd6` | 局部结果非 0 进入成功清理路径，否则返回 `0x80004001` | 成功与拒收有区分 |
| `0x0042a8cd` | `xor eax, eax` | 成功清理路径返回 `S_OK` |

主程序消息映射表 `0x006e14d0` 将 `0x2b3e` 映射到 `0x0049b3c0`。处理器进一步调用下载处理/UI 路径。异步分支的 COM 成功返回与稍后的 UI 选择没有同步依赖；尚未据此断言完整 UI 路径或每个 Cancel 按钮的协议映射。

## 通信组件补充

- 宿主的 `0x004028cc` 使用 `\\.\pipe\IDMNetworkMonitor.` 前缀。网络模块 `0x10013d88` 使用 `\\.\pipe\IDMNetworkMonitor.%u`，`0x10013625` / `0x1001364b` 调用 `CreateNamedPipeW` / `ConnectNamedPipe`。
- 网络模块 `0x100344ea` 引用 `plugin.v3.internetdownloadmanager.com` 子协议，提供与插件相符的 WebSocket 通信线索。
- 主程序 `0x0060f210` 的 session 管道路径调用 `CreateFileA`，这一段也是客户端，不能因为它在主程序里就称为管道服务端。
- 宿主包含对端进程查询 API，但本轮没有完整证明其每一条身份校验条件。

## 项目处理与验证

本轮只补充分析文档，不改变已有交接行为。当前 `desktopAcceptedHandoff` 根据可呈现的接收回执转移责任；恢复查询收到用户已取消/拒绝的终态时，不另行恢复浏览器副本。交接失败、无法呈现和未查清接收结果继续走各自保全/恢复流程。

此前实际通过的扩展 520 项回归、类型检查、Chrome/Firefox 构建和 43 项 Rust 定向测试见 [浏览器参考验证记录](browser-reference-completion.md)。本轮未修改生产代码，没有重复执行这些测试，也没有做 IDM 性能或运行时对照。不能宣称低于 IDM 的占用、快于 IDM，或所有功能行为完全相同。

分析产物位于工作区 `.tool-cache/browser-reference-20261104/idm-native/`，包括原始字节反汇编、COM 接收方法、下载派发方法、主窗口处理器及 `rea-evidence.json`。Capstone 线性反汇编可能把跳表解码为指令；结论只采用已确认入口、有效调用和直接读取的跳表数据。

REA 现有引擎未配置 Ghidra/IDA 等反编译器，未重复失败或安装大型分析工具；使用已有 pefile/Capstone 继续静态追踪。REA 导入表、资源和导出表证据已导出，共 4 条记录：

- Host imports：`ev_2c23bcb85c3318ec8a3ab478cb5d27eae637b11c96a375457bd91aab8ecf9c5c`
- Main PE resources：`ev_70cf7ab4d27fcebce3527202005ff8cfa607589f40c3fe5c8ed1c96f524e751e`
- NetMon exports：`ev_ad8524c6216c97d69eaef79df67ef470af248188801eb376e8f49cbf416802f2`
- NetMon imports：`ev_1fed0d573f53469bbbe26877a41dcdd2f225cd4632a75e13f60ac03b772df4a1`

证据包的工具级 `unknowns=0` 只描述这 4 次结构化读取，不代表本轮行为问题均已解决。REA 原生会话已关闭。本轮没有启动 IDM、浏览器、测试服务或后台工作进程。

## 重新归档的参考目录与请求前接管（2026-11-05）

项目交付目标为 `A:\Ubuntu\测试\hls-downloader`。`A:\Ubuntu\逆向` 仅供只读参考，旧 `新建文件夹`、`idm643` 路径已不在当前目录结构中；前文路径记录的是当时检查位置。本轮检查了新目录的 README、原版扩展、恢复的下载引擎 C、Ghidra 输出以及重建 C# 的候选处理入口，没有运行参考目录的程序或验证脚本。

当前输入需区分三类：

- `1_浏览器插件源码/idmmzcc3_6.43.1_deobfuscated/`：解包、反混淆的插件代码。
- `2_项目主体源码/ghidra_decompiled/`：原生函数的反编译伪代码，不是原厂源码。
- `2_项目主体源码/download_engine/`、`ipc_and_host/`、`media_parsers/` 和 `rebuilt_app/src/`：恢复或重建实现；必须与原始函数、协议消费方核对后再借鉴，不能单凭目录说明当作原版行为。

本轮发现具体证据冲突：`download_engine/IDMan_DownloadEngine_recovered.c` 的 `idm_split_largest_active_segment` 注释将动态最大段对半切分归到 `0x00582990`；相同地址的 `ghidra_decompiled/idman/0x00582990.c` 展示的是逐行读取、查找和解析 `Content-Range` 等响应处理，没有该段扫描和对半分配实现。因此本轮没有将这份重建调度算法移植进项目，也没有据此声称还原了 IDM 的动态切分。

`rebuilt_app/src/NetMonBus.cs` 的 type 13 分支先 `sendReply(verdict)`，再 `_engine.EnqueueAndStart(spec)`；这一重建行为不足以证明桌面已经持久保存请求，不能替代本项目的持久接收回执。

可采用的插件机制来自原版 `background.js`：`zb` 注册带 `requestHeaders`、`blocking` 的 `onBeforeSendHeaders`，`Y` 打包本次头部和请求信息，`sc` 保存等待裁决的 Promise，`X` 兑现 `{cancel:true}`。结合 [Firefox onBeforeSendHeaders 官方合同](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/webRequest/onBeforeSendHeaders)，项目新增 Firefox 明确附件 GET 的请求前交接。该事件发生在 HTTP 数据发送前，允许取得此次凭据并等待桌面裁决；其他扩展随后修改头部的情况不在该事件的保证范围内。

项目继续使用 Native Messaging、自己的持久业务 ID 和原 ID 恢复查询，没有移植 IDM 的私有 MSG# 协议。普通 [Chromium MV3 的 webRequestBlocking 限制](https://developer.chrome.com/docs/extensions/reference/api/webRequest)继续适用，不把 Firefox 的阻塞方式推广到普通 Chrome/Edge 安装。具体边界、回归结果和浏览器检查见[项目验证记录](browser-reference-completion.md)。

## 本地最新参考增量复核（2026-11-07）

继续只读查看同一参考目录。`RECONSTRUCTION.md` 本次读取时修改时间为 2026-11-07 15:10，内容已到 §105；原版插件 manifest 仍为 **6.43.1**，主体仍以 **IDM 6.43 build 16** 为目标。这是本地还原材料的更新，未联网获取新版 IDM，也未证明它对应厂商最新版本。

- 新目录 `ghidra_decompiled/idman_all/` 提供主程序完整函数输出，`main_callgraph_map.md` 明确指出：MFC 通过虚表调用 `InitInstance`，按文本直接调用构造的可达函数数量不能代表真实运行范围。`idman_main/` 增补启动流程及许可校验重建；§97–§105 的新工作主要在许可记录与验签，对 HLS 下载功能没有直接应用价值，本轮未移植。
- `ts2mp4/` 增补共享读取器、缓冲交接、分帧、音视频时间戳和 MP4 写出等恢复代码。§76–§77 撤回了早期“环形/缓存/容器”的部分命名，将相关函数重新确认为 deque、原子标志、一次通知和线程 join。抽查 `idman_all/0063/0x00636210.c`：先加锁检查 `+0x28`，未置位时置位，解锁后通知；已置位时直接解锁退出。这支持“一次结束、及时释放”的通用思路，不证明 HLS 应复刻其二进制布局或自建 TS→MP4 编码链。
- 前轮的分段归属冲突**仍存在**：最新完整输出 `idman_all/0058/0x00582990.c` 依然是逐行解析 `Content-Length` / `Content-Range` 的路径，恢复 C 和报告 §27 仍将该地址称作“动态最大段对半切分”。本轮没有采用这个未经对应原函数证明的调度归属。
- 本轮将资源回收思路用于 HLS 自身已复现的缺陷：MAIN-world 清单克隆探测在 SPA 换页或 `pagehide` 时立即取消自己的 reader，避免等旧响应下一块数据才退出；取消只作用于插件克隆，原响应仍可完整读取，片段导航保留当前探测。实现使用现有 Streams 和导航事件，没有引入额外请求、依赖或 IDM 私有协议。浏览器分支取消与背压边界参见 [MDN Streams tee](https://developer.mozilla.org/en-US/docs/Web/API/ReadableStream/tee)。

参考文件、抽查伪代码、HLS 修改与两种构建的 SHA-256 位于 `artifacts/idm-stream-lifecycle-20261107/source-build-sha256.json`。定向失败复现、修复后回归与真实 Firefox 场景结果见[浏览器验证记录](browser-reference-completion.md#旧页面克隆探测及时释放2026-11-07)。本轮未运行参考目录的可执行程序或其验证脚本，不继承其宣称的编译、字节校验和端到端通过状态，也不宣称完整复制 IDM 或达到相同体积、速度和内存占用。

## 下载线程与媒体后处理增量复核（2026-11-08）

本轮重新读取 `A:\Ubuntu\逆向` 的新增材料。调查期间 `RECONSTRUCTION.md` 从 §158 继续更新到 §160（02:08 版，798375 字节），新增内容涵盖 HTTP 工作线程、请求头、错误分发、连接状态、Schannel 缓冲及媒体后处理。原版插件仍为 6.43.1，主体目标仍是 IDM 6.43 build 16；这是本地还原进展，不是厂商新版本源码。文档存在两套 §106–§124，以下按主题和函数地址定位。

| 抽查主题 | 原始函数与可确认行为 | 采用范围或限制 |
| --- | --- | --- |
| 媒体就地重封装（第一套 §118） | `idman_all/005d/0x005d1f70.c` 将原文件移到 `.tmp`，分别打开源和目的文件后改写 | 借鉴源与输出分离；该函数没有展示完整的逐项错误处理，不能据此宣称 IDM 已保证失败回滚或成功后原子发布 |
| 请求头构造（§141） | `idman_all/005a/0x005ae310.c` 包含 64 位 Range，以及 `Accept-Encoding: identity` 和 `gzip, deflate` 两条分支 | 不按恢复文件的概括把所有请求强制为 identity；HLS 既有 Range 身份校验和压缩处理继续保留 |
| TLS 接收容量（§159–§160） | `idman_all/005f/0x005f1ad0.c` 查询流大小并以三项相加再加 128 分配；`0x005f37c0.c` 按剩余容量接收 | `stream_sizes_recovered.c` 把 `+0x33A8` 标为 MaxMessage、`+0x33AC` 标为 Trailer，但原函数日志的参数顺序是 Header、Trailer、MaxMessage，后两项相反；不复制该字段命名，也不将其容量推论当作完整内存安全证明 |

新增 `speed_limiter_recovered.c` 主要描述默认值、界面和注册表保存，尚不能作为真正传输限速算法的证据。恢复 C 的探针编译或字面量校验不足以证明它与原程序功能等价；本轮未运行参考工程、验证脚本或其可执行文件。

本轮实际修改 HLS 的 `native_shell/src/media/merge.rs`：HLS/DASH 的 FFmpeg 合流及多轨封装先写同目录独占创建的暂存文件，保留输出后缀，子进程成功退出并同步文件后再次检查暂停/取消，再调用项目已有的原子替换。失败、暂停、取消时清理本次暂存，保留旧结果、输入轨道和诊断日志。无附加轨道的复制复用既有安全拼接路径，避免输入输出路径别名导致原文件截断。未新增依赖、自制媒体封装器或 IDM 私有协议。

这是 HLS 自身的输出完整性修复，不是对 IDM 失败恢复保证的复刻。机器断电或进程被外部强杀仍可能留下未提交的暂存文件。实际失败复现、定向回归和边界见[合流验证记录](browser-reference-completion.md#媒体合流成功后替换2026-11-08)，证据存于 `artifacts/idm-media-publish-20261108/`。本轮未修改浏览器凭据、接管裁决或桌面 UI。

## 全量材料与重建完整性复核（2026-11-08）

本轮快照到 `RECONSTRUCTION.md` §186（15:25 版，926592 字节）。对四个完整目录的 23988 份 Ghidra 输出做清点、指标扫描与哈希，复核原版插件/通信/下载/媒体关键路径，并独立检查恢复 C 与最终交付插件。完整结论见 [IDM 源码与完整性复核](idm-source-audit.md)，本轮不修改 HLS 生产代码。

主程序 20704 份输出数量属实，但有 1 份反编译超时占位，浏览器 DLL 另有 3 份；文件数不能证明全部语义恢复。119 份恢复 C 中有实际函数，也有探针和状态示意。MSVC x86 独立对象编译为 116 成功、3 失败，没有完成整体链接；Zig 在 C 分析前发生缓存 AccessDenied，未用这一环境错误否定其原有编译结果。两个 C# 重建目标构建成功，但不链接这 119 份 C，也未运行或注册。

此前 `0x00582990` 的动态切分归属冲突仍存在；本轮还直接读取原 EXE 字节，确认音频恢复 C 将两个不同字段错误合并为同一字段的两次累加。`4_自研下载器` 的实际 MV3 解码器又在 4 个协议纯函数用例中全部解析错误，其 Python 测试没有覆盖该 JS。故不能依据“编译通过”“模拟客户端通过”认定已经完成原版等价下载器和真实浏览器接管。

新增 TLS 命名恢复已在 §161 修正 Trailer/MaxMessage 顺序；不沿用前轮对旧文件的错误命名作为新文件缺陷。浮窗创建与几何链可追到 `IDMBRBTN.DLL` 的导出调用，但当前材料缺少该 DLL 的完整绘制/输入实现。

证据保存在 `artifacts/idm-full-audit-20261108/`。本轮没有执行原版/重建程序、参考安装或注册脚本、全部 109 个验证脚本，也没有运行 HLS 回归或做性能对照；保留 HLS 已有完整凭据、持久责任转移、续传身份校验及媒体原子输出实现。

## 新增参考更正（2026-11-09）

最新 `IDMan_DownloadEngine_recovered.c` 已撤回 `0x00582990` 的动态切分归属，并指向新增 `chunks_list_recovered.c`。因此，上文“归属冲突仍存在”仅描述 11 月 8 日的历史快照，不再作为最新材料的缺陷。新模块仍存在 ZIP 优先分支简化、块布局冲突、模拟文件打开及未实际合并文件等问题；新增链接与 TS 脚本也未完成真实下载和 MP4 成品验收。具体原函数、脚本范围及 HLS 采用边界见 [增量复核](idm-source-audit.md#新增调度解析器与链接测试复核2026-11-09)。
