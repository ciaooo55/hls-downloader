# 发布后源码验证记录

日期：2026-11-15。正式发布版本仍为 [v7.0.6](https://github.com/ciaooo55/hls-downloader/releases/tag/v7.0.6)，受测发布源码为 `1c1dc06b7dc63e9c728ae75e5dcff9772781cd7d`。本文记录在基线 `7636ad417afec41cce81cf078af0a40598db1fff` 上完成的后续源码修改，不能替代新成品的完整发布验收。

## 已落地修改

- Firefox 请求发送前接管同源、带 `download` 属性的无后缀链接，保留原始签名 URL、凭据和网页文件名；普通导航不因下载日志读取失败被取消，丢回执继续按原请求 ID 查询。
- HTTP 206 校验声明终点和 Content-Length，读取范围不越过响应声明边界；合法短范围继续请求后续字节，矛盾元数据不能进入可靠检查点。失败和整包转交先记录停止状态，再释放活动区间。
- HLS 首个有效行要求完整 `#EXTM3U`，隐式字节偏移只能延续相邻同资源；部分分片与完整分片分别记录偏移。
- DASH 使用项目已有 roxmltree 解析 XML，处理命名空间、注释、CDATA、签名 URL 和真实父节点继承，删除被替代的字符串 XML 扫描。
- 已完成任务在发布标记不可读或被缓存清理移除时继续使用持久化成品路径，不误指向其他任务的同名文件。

原版函数、调用方、行为约束与 HLS 生产路径见 [逐模块对应表](idm-source-audit.md#逐模块实施2026-11-14)。没有修改 UI、迁移持久数据、接入恢复 C 引擎或 IDM 私有 DLL。

## 实际执行结果

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

## 尚未完成的验收与差异

没有操作个人桌面或浏览器，没有运行原版 IDM。忽略的 14 项包含交互式 UAC、Sandbox 传输和真实 FFmpeg 等条件场景，不能算作通过。

本轮尚未执行当前源码在独立环境的真实浏览器/Host/Core 接管、安装启动与升级、合法媒体成品解码和音画同步、五轮修改前后性能及原版 IDM 同条件对照。性能脚本已补齐五轮采样和中位数输出，但不能据此宣称性能回退低于 5% 或达到 IDM。Firefox 永久安装仍需 Mozilla 签名。

FTP 多连接/主动 PORT、DASH 多 Period/SegmentList 字节范围尚未实现。现有 FFmpeg 封装、DOM 浮窗、持久回执及额外协议功能按计划保留。

## 源码与发布文件的边界

`main` 保存当前维护的产品代码、测试、配置和必要文档；Git 提交历史、版本标签与已有 Release 保留。正式安装包和配套浏览器扩展以 Release 附件为准，不把本轮开发包覆盖到已发布的 v7.0.6。

`.tool-cache/`、`artifacts/` 和依赖输出是本地忽略目录；下载文件、任务恢复数据、用户自有备选 UI 和加密凭据备份不能作为编译垃圾删除。清理记录保留实际结果，不将被策略拒绝的删除记录为成功。

2026-11-15 整理结果：GitHub 只保留 `main` 这一条维护源码分支；10 个已发布 Release、94 个附件及 1 个未发布草稿保持原样。根目录 `site-cache/` 纳入项目忽略规则。已删除可重建的 Rust target、Compose 构建目录、扩展 `.output` 和 `.wxt`，释放约 40 GiB；依赖缓存和原始验证证据保留。`.tool-cache/idm-port` 与 `.tool-cache/test-tmp/idm-port` 的删除再次在执行前被自动审批以 `blocked by policy` 拒绝，仍保留约 15 MB 的旧 Core 副本和两份共 28 字节的夹具，未绕过限制。详细本地记录为 `artifacts/workspace-cleanup-20261115/`。
