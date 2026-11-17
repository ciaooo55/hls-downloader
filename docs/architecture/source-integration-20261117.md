# 2026-11-17 外部第五轮源码与第六轮界面整合

## 输入与基线

- 用户提供 `HLSDownloader-7.0.7-第五轮-源码.zip`，SHA-256：`bf26a4e1f98a0a6f14da2c8b6fb5b1dd1b615693a5b652027f34a9919dc92ee0`。
- 用户提供 `final6.zip`，SHA-256：`3186f728ea3b6c20e8b7e0a61d3612a30cacf8cd51413af7eba6923cadcc6d94`。实际内容是 12 张 UI 对比图，没有另一套源码。
- 当前 `main` 基线提交 `e0a2007a8f144cbc213243576732637fbe2803b6`，另有此前 7.0.7 未提交修复。逐文件核对原导出清单，当前工作区与交给对方的 569 文件快照一致；对方改动共涉及 75 个文件，无删除文件。
- 输入解压与原始差异保存在忽略目录 `artifacts/source-integration-20261117/`，不运行压缩包内脚本，不把其验证声明当成本次结果。

## 实施顺序与完成标准

1. **全量差异审阅。** 核对全部变化文件及 12 张 UI 图，重点跟踪生产调用方、状态/凭据/资源生命周期。保留此前 curl 流式传输、溢出修复、POST 原文、原响应保全和持久回执合同。
2. **实际合入现有模块。** 将原生 Slint 的工作台、设置、详情、资源工具、投屏/播放器、小窗口和统一主题，以及插件弹窗、网页浮层、错误文案改动合入现有路径。保留原操作、额外功能、持久数据和现有构建入口。
3. **Core 改造。** 合入进程内控制/进度、周期持久化、进度信箱、检查点缓存、curl 并发限制、HTTP 检查点节流、FTP/FTPS 多连接和主动模式、DASH 多 Period/整文件 SegmentBase、HLS 批量同步与可选重封装。对审阅发现的控制竞态、注册释放、持久序号、平台差异等问题先补回归再修正。
4. **清理未接入内容。** 新增但没有调用的媒体尺寸探测模块需核对用途；不得为显示文件大小自动消耗一次性 URL 或扩大凭据范围。没有安全实际入口的孤立代码不进入生产。
5. **Windows 代码验收。** 先运行控制/持久化和引擎定向回归，再执行受影响的完整 Rust 库、原生 UI 编译/测试、Core/Host 构建、插件类型检查/测试及双浏览器构建。真实媒体使用项目已有隔离无 UI 夹具。必要修复后只重跑受影响检查。
6. **整合交付。** 记录每组采用情况、后续修正、实际通过和未验证项；构建产物绑定当前源码。对方截图保留为参考，不能代替本机 Windows 实测。继续遵守不操作个人桌面和浏览器的要求；不为合入新增分支或 worktree，不将旧日志或旧源码清单移入产品根目录。

## 当前核查发现（合入前）

- UI 图片显示统一浅/深色主题与小尺寸布局；源码包包含对应 Slint/CSS/Rust 修改，仍需 Windows 编译核对全部属性与回调。
- 内存控制的嵌套注册对进度目录增加了多次引用，但最后才释放一次，存在缓存无法释放的风险；对账读文件未与命令写入串行，旧文件读值可能覆盖刚收到的暂停/取消。整合时补确定性回归。
- 进度快照冲刷同时使用预留的未来事件序号写任务行，需要核对与后台写入的序号保护是否冲突。
- `mediaSizeProbe.ts` 在该包中未见入口变更，需确认是否为未接入试稿。

## 合入结果与整合修正

75 项外部变化中，74 项进入当前生产源码、测试和文档。原工作区已有修复完整保留。唯一未采用的 `extension/lib/mediaSizeProbe.ts` 没有任何调用方，并且 HEAD 失败后发 GET 的策略可能消耗一次性地址；没有为接入该孤立模块扩大网络请求。

- **UI 与插件：**采用 Graphite + Cobalt 主题及各 Slint 窗口、弹窗、网页媒体浮层样式；继续使用现有原生 UI 和 Core。插件桥接区分“未安装”与“浏览器禁止访问”，避免把权限错误误报为缺少宿主。
- **任务控制与进度：**修正嵌套注册引用未成对释放；对账文件读取与控制写入串行，避免旧值覆盖暂停/取消；进度发布与退出冲刷串行，避免共用临时文件冲突和旧快照覆盖。
- **持久化：**修正冲刷任务行时错误使用未来预留事件序号的问题，周期 writer 不再被未来序号阻断。嵌套释放和序号问题均先用回归复现失败，再修改实现。测试不再泄漏整个 Core 来模拟崩溃。
- **检查点与播放：**Range cache 的缺失读取、更新和清除串行，防止旧读取覆盖新发布状态；诊断计数只在测试编译。播放优先区间的文件/缓存更新同样串行。
- **FTP：**采用多连接、主动模式及回退，修正 `resume=false` 仍复用分段完成表的问题；新增同身份旧检查点加错误文件的真实 FTP 回归，要求重新下载并与完整夹具相等。
- **DASH：**采用多 Period 与整文件 SegmentBase；修正复用相同初始化段时遗漏 Period discontinuity，增加独立/共享初始化段两种真实音视频场景。
- **HLS：**采用分片批量同步和可选 TS 无损封装。把封装设置贯通 Presenter、IPC、Core 和 worker；显式保留所有视频/音轨，新增三轨解码哈希比较，防止有效 MP4 丢失第二条音轨。
- **文档：**外部测试记录标为历史输入，移除无同条件证据的“已优于 IDM”等结论；修正将完成分片时检查时间阈值误写为严格每秒落盘的描述。
- **验收修正：**MP2 测试音轨在 TS/MP4 中触发 FFmpeg 不同默认解码器，压缩音频数据哈希一致、默认解码舍入不同；统一使用 MP2 解码器后比较所有轨道。媒体代理取消测试接受 Windows 主动断开时的 `ConnectionReset`，仍拒绝超时和残留数据。隔离 Core 验收显式传入 `--no-tray`，避免测试进程创建桌面托盘；既有独立 IPC 和设置继续隔离注册、数据、通知和窗口。

## 验证结果

原始日志位于 `artifacts/source-integration-20261117/`，外部 Linux 验证和截图不作为本次 Windows 验收证据。

| 检查 | 本次结果 | 证据 |
| --- | --- | --- |
| 插件全量测试及类型检查 | 54 个测试文件、613 项通过 | `extension-tests.log` |
| 桥接错误分类修正后的复验 | 类型检查、30 项 nativeBridge 回归通过 | `extension-final.log` |
| 最终 Chromium / Firefox 构建 | 两种目标均成功 | `extension-final.log` |
| 第一轮 Windows Core | 741 项通过、17 项按原配置跳过；后续修正另行验证 | `core-tests.log` |
| Windows Slint / Presenter | 编译成功，37 项逻辑测试通过；未显示窗口 | `ui-tests.log` |
| 最终 Windows Core / client / protocol 库 | 742 / 48 / 11 项通过；原配置共 18 项跳过 | `core-final.log` |
| 显式 curl 传输回归 | 5 项通过（上列跳过项目中的 5 项） | `curl-explicit.log` |
| 显式真实 FFmpeg 回归 | DASH 两项、封装替换/暂停取消两项，共 4 项通过（上列跳过项目中的 4 项） | `ffmpeg-explicit.log` |
| 实际引擎 HLS 全链路 | 普通 HTTP、每资源首次 403 后 curl 回退各 4 场景，共 8 场景通过；TS、AES-128 TS、fMP4、Range fMP4，每场景 30 帧视频和 142 帧音频的时间戳/大小/解码哈希与独立 FFmpeg 参考一致，实际请求保留签名 query、Cookie、Authorization | `real-hls.json`、`real-hls-curl.json` |
| 最终 Clippy | Core、Native Clients、UI、client、protocol 全 targets 以 `-D warnings` 通过，沿用项目的 `large_enum_variant`、`too_many_arguments`、`type_complexity` 例外 | `clippy.log` |
| 最后清理复验 | 4 项 Range cache 测试及周期 writer 测试通过；关闭 SQLite 后没有新增该测试数据库残留 | `cleanup-regression.log` |
| 最终 Windows 开发构建 | Core、Launcher、CLI、Native Host、Updater、Slint Presenter 构建成功，无编译警告；不是正式安装包发布 | `native-build-final.log` |

## 验证边界

- 未启动个人桌面窗口或浏览器；Slint 编译及逻辑测试不能替代真实 DPI、字体、阴影、遮挡和点击验收。
- 不把本次自测描述为达到 IDM：没有同环境原版对照或修改前后五次性能中位数结果。
- FTP 回归使用本地协议夹具；真实 FTPS 服务、复杂公网 NAT 仍需独立环境验证。
- DASH 动态直播、跨 Period 的整文件轨道和编码族变化仍受现有实现限制。
- 本次只整合与验证，不提交、推送或发布。

## 本轮资源与清理

输入解压、差异、运行日志和验收脚本保留在忽略目录 `artifacts/source-integration-20261117/`，用于复核；原始用户 ZIP 未修改。真实 HLS 两次验收的成功夹具由脚本清除。

显式清理本轮 5 个遗留测试夹具的命令在执行前被自动审批以 `blocked by policy` 拒绝，未改用其他方式删除。它们仍位于 `.tool-cache/test-tmp/source-integration-20261117/`：

- `hls-task-control-nest-7452-1794911887828547600/`（修复前的嵌套注册反例）
- `hls-ts-remux-18552-1794914111664342900/`（不同解码器导致哈希不同的诊断夹具）
- `hls-core-progress-writer-18552-1794914058102773700.db`
- `hls-core-progress-writer-5940-1794912241160634500.db`
- `hls-core-progress-writer-6820-1794914351150949500.db`

后三项来自周期 writer 测试的未关闭 SQLite 连接；测试已补 `drop(blocker)`，避免后续再产生同类残留。准确清理状态见本轮 `cleanup.json`，不宣称测试临时目录已经清空。

## 逐文件采用表

| 文件 | 处理 |
| --- | --- |
| `DESIGN.md` | 已合入；必要修正见上文及工作区差异 |
| `docs/architecture/idm-alignment-review.md` | 已合入；必要修正见上文及工作区差异 |
| `docs/architecture/v7-architecture.md` | 已合入；必要修正见上文及工作区差异 |
| `docs/releases/v7.0.7.md` | 已合入；必要修正见上文及工作区差异 |
| `docs/validation/idm-port-validation.md` | 已合入；必要修正见上文及工作区差异 |
| `extension/entrypoints/background.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/entrypoints/content.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/entrypoints/popup/index.html` | 已合入；必要修正见上文及工作区差异 |
| `extension/entrypoints/popup/main.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/entrypoints/popup/style.css` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/format.test.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/format.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/mediaOverlay.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/mediaSizeProbe.ts` | 未采用：无调用入口，额外探测请求有一次性地址副作用 |
| `extension/lib/nativeBridge.test.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/nativeBridge.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/productCopy.test.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/productCopy.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/subtitleContent.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/theme.test.ts` | 已合入；必要修正见上文及工作区差异 |
| `extension/lib/theme.ts` | 已合入；必要修正见上文及工作区差异 |
| `native_clients/src/native_host.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/av_scan.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/bin/hls_downloader_engine.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/connection_parts.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/core_runtime.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/core_service.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/curl_slots.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/download_worker.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/download_worker/paths.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/download_worker/settings.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/download_worker/tests.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/ftp_engine.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/http_engine.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/lib.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/media/dash.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/media/hls.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/media/merge.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/media/preview.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/native_host_registration.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/output_path.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/playback.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/progress_facts.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/progress_relay.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/progress_writer.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/range_cache.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/sftp_engine.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/sleep_inhibit.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/store.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/task_control.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/torrent_engine.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_shell/src/updater.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/cast_tools.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/hot_main.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/resource_tools.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/service_settings.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/settings_editor.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/system_tools.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/table_layout.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/src/workbench.rs` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/cast_tools.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/details.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/dialog_components.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/hot.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/media.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/queues.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/resource_tools.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/service_settings.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/settings.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/site_rules.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/system.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/task_editor.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/theme.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/workbench.slint` | 已合入；必要修正见上文及工作区差异 |
| `native_ui/ui/workbench_components.slint` | 已合入；必要修正见上文及工作区差异 |
