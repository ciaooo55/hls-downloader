# 发布后源码验证记录

日期：2026-11-15。正式发布版本仍为 [v7.0.6](https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.6)，受测发布源码为 `1c1dc06b7dc63e9c728ae75e5dcff9772781cd7d`。本文记录在基线 `7636ad417afec41cce81cf078af0a40598db1fff` 上完成的后续源码修改，不能替代新成品的完整发布验收。

## 已落地修改

- Firefox 请求发送前接管同源、带 `download` 属性的无后缀链接，保留原始签名 URL、凭据和网页文件名；普通导航不因下载日志读取失败被取消，丢回执继续按原请求 ID 查询。
- HTTP 206 校验声明终点和 Content-Length，读取范围不越过响应声明边界；合法短范围继续请求后续字节，矛盾元数据不能进入可靠检查点。失败和整包转交先记录停止状态，再释放活动区间。
- HLS 首个有效行要求完整 `#EXTM3U`，隐式字节偏移只能延续相邻同资源；部分分片与完整分片分别记录偏移。
- DASH 使用项目已有 roxmltree 解析 XML，处理命名空间、注释、CDATA、签名 URL 和真实父节点继承，删除被替代的字符串 XML 扫描。
- 已完成任务在发布标记不可读或被缓存清理移除时继续使用持久化成品路径，不误指向其他任务的同名文件。
- 2026-11-15 在 `ddcb2c6e` 基线上补入 FTP EPSV/IPv6 与受限 PASV 回退；修复 PASV 跳过坏字段、接受多余字段和端口饱和计算。协商失败不覆盖原文件，REST 仍紧邻 RETR。
- 同日贯通 DASH SegmentList 显式字节范围的解析、初始化段、视频、音轨、字幕、直播去重、时长累计和断点身份。通过现有 HTTP 凭据和重定向路径，严格校验范围响应，以 64 KiB 缓冲落盘，校验并同步成功后才记检查点。
- 分片身份变化时先持久失效旧检查点，再写新数据；修复失败传输恰好写满旧长度后，重启可能误用旧身份的问题，并移除不再调用的旧检查点查询方法。

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

## 尚未完成的验收与差异

没有操作个人桌面或浏览器，没有运行原版 IDM。忽略的 14 项包含交互式 UAC、Sandbox 传输和真实 FFmpeg 等条件场景，不能算作通过。

本轮尚未执行当前源码在独立环境的真实浏览器/Host/Core 接管、安装启动与升级、所有媒体变体和音画同步、五轮修改前后性能及原版 IDM 同条件对照。已新增并实际运行真实 FFmpeg 回归：生成 2 秒 fMP4，按初始化段与两个媒体范围下载，通过生产封装路径输出，20 帧解码哈希与源文件一致；该用例为视频，不代表独立音轨同步已验收。性能脚本已有五轮采样和中位数输出，但不能据此宣称性能回退低于 5% 或达到 IDM。Firefox 永久安装仍需 Mozilla 签名。

FTP 多连接/主动 PORT、DASH 多 Period 和 SegmentBase/sidx 索引展开尚未实现。SegmentList 显式字节范围现已实现。现有 FFmpeg 封装、DOM 浮窗、持久回执及额外协议功能按计划保留。

## 源码与发布文件的边界

`main` 保存当前维护的产品代码、测试、配置和必要文档；Git 提交历史、版本标签与已有 Release 保留。正式安装包和配套浏览器扩展以 Release 附件为准，不把本轮开发包覆盖到已发布的 v7.0.6。

`.tool-cache/`、`artifacts/` 和依赖输出是本地忽略目录；下载文件、任务恢复数据、用户自有备选 UI 和加密凭据备份不能作为编译垃圾删除。清理记录保留实际结果，不将被策略拒绝的删除记录为成功。

2026-11-15 整理结果：GitHub 只保留 `main` 这一条维护源码分支；10 个已发布 Release、94 个附件及 1 个未发布草稿保持原样。根目录 `site-cache/` 纳入项目忽略规则。旧 Rust target、Compose 构建目录和扩展输出此前已删除，后续编译按项目配置重建缓存。用户随后执行清理脚本，已复核 31 个指定旧目标均不存在，包括 `.tool-cache/idm-port` 和 `.tool-cache/test-tmp/idm-port`；之前的策略拒绝记录不再代表当前残留。依赖缓存、用户数据和原始验证证据保留。详细本地记录为 `artifacts/workspace-cleanup-20261115/`。
