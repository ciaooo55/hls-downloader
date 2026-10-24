# 2026-10-23 全量对抗测试与续传修复

结论：本轮已识别的三个 HTTP 续传问题已修复，最终冻结二进制的回归、沙盒交互和安装生命周期验证通过。范围是现有验收矩阵加五轮续传/日志故障注入，不是五遍完整 GUI 矩阵，也不代表所有网站和电脑均无缺陷。

源码基线为 `main@00cd089e7f643dab3ec1847ce8a69a8c5864b614`，产品修改仅涉及 `native_shell/src/http_engine.rs` 和 `native_shell/src/download_worker.rs`。桌面交付为基线加 `source.patch` 的未签名候选包；构建时尚未提交的状态记录于包内来源文件，本轮收尾将相同产品源码与验收记录一并提交到本地 `main`。未推送或发布。没有在宿主安装或执行产品；产品运行和 Rust 测试在 Windows Sandbox 内进行。

## 修复的根因

1. **截短缓存可能被发布为损坏成品。** 检查点声称完整，但实际临时文件较短；旧代码在核对文件前先预分配，补出的零字节被算作已完成。现于预分配前读取实际文件长度，任一已完成范围超过文件实际长度即弃用检查点并重新下载。修复前的错误 SHA 和请求记录保留在 `resume-faults.json`；修复后各轮成品 SHA 均匹配源数据。
2. **已知大小、缺少可靠资源标识时，正确断点被丢弃。** 原来主地址已知大小便跳过探测，不能取得强 ETag/Last-Modified 验证检查点。现在缺少可靠标识时先探测，弱 ETag 可被探测得到的强 ETag 替换；验证匹配后仅下载缺失范围。不能验证身份时仍不会盲目拼接旧内容。
3. **失效断点遇到不支持 Range 的服务器可能持续失败。** 原来仅凭临时文件存在就认为不是从零开始。现在判断是否真正保留了有效范围；全部范围已弃用且续传偏移为零时，允许安全回退整包单流下载。有效断点遇到不兼容响应仍拒绝拼接。

新增三个有实际字节断言的回归用例。协调器的三个测试服务器原来只处理一个请求，资源探测后连接耗尽；已改为连续处理并正确响应 Range，保留原有成品发布、扫描拦截和链接刷新断言。生产路径没有增加依赖或新配置。

## 验证结果与证据

| 范围 | 实际结果 | 证据 |
| --- | --- | --- |
| 最终 Core 库测试 | 623 通过、0 失败、4 默认忽略 | `results/rust-hls_native_shell-100fa787ddcb0e38.log` |
| Updater/Engine/集成目标 | 7/2/8/1/1 项分别通过；NativeHost 目标无单元用例 | `results/rust-*.log` |
| 桌面测试 | 130 通过、0 失败 | `results/desktop-unit.json` |
| 插件测试 | 51 个文件、393 项通过；源码未改，原交付 ZIP 复用 | `results/extension-test.log` |
| 最终 Firefox 响应接管 | 39 项通过，含用户提供的 GitHub EXE、无浏览器下载项/创建事件、取消/离线回退 | `results/final-firefox-takeover.json` |
| 工作台功能与删除 | 详情四页、32 MiB 成品 SHA、暂停续传、批量、抓取、队列、删除/取消/清理范围均通过 | `results/final-workbench-*.json` |
| 磁力链接 | 取消元数据加载、失效输入、选择文件、真实 peer 传输及仅发布所选文件通过 | `results/final-workbench-magnet.json` |
| 边下边播和缓存清理 | 27 项通过；HTTP、TS/fMP4/AES HLS、外挂音轨和 DASH；150% 播放控件另验 12 项 | `results/final-download-playback-cleanup.json`、`results/player-overlay-150.json` |
| 五轮最终续传/日志故障注入 | 每轮 5 项全部通过；成品 SHA、复用前缀、截短、忽略 Range、数据库及接口最多 500 条日志 | `results/resume-faults-verified-1.json` 至 `5.json` |
| 浏览器交互与网页字幕 | Firefox/Edge 原生确认操作、下载 SHA、插件配置入口；Shadow DOM 采集及模拟提供方通过 | `results/actions-*.json`、`results/subtitle-shadow.json` |
| 最终 MSI | 自选带空格目录、全部组件 SHA、Windows Downloads 默认路径、公开 MDN 视频下载、UI 连接、卸载注销通过 | `results/installer.json` |
| 最终 MSI 升级与失败回滚 | 旧版本运行时升级、保留任务/设置/断点、恢复后 SHA、失败回滚均通过 | `results/final-msi-Upgrade.json`、`results/final-msi-FailureRollback.json` |
| FTPS | 显式/隐式 TLS、REST 续传及最终 SHA 通过 | `results/final-local-ftps.json` |
| 基线广覆盖矩阵 | 15 组通过：媒体识别、接管、字幕预取、HTTP 边界/停滞/限速、扩展名、播放/清理、千任务、FTP/SFTP/BT/队列等 | `matrix.json`、`results/baseline-matrix/` |

汇总判定见 `summary.json`。基线矩阵的 Engine SHA 与最终修复版不同，不将其冒充最终二进制全量复跑。最终二进制补跑范围、哈希及执行时间在 `results/final-summary.json` 和 `results/verified-summary.json` 中分别记录。

两个修改文件的 `rustfmt --check` 和 `git diff --check` 实际通过。全仓 `cargo fmt --all --check` 有未修改文件的既有格式差异（core_runtime/core_service/media-dash/native_host/subtitle_translate），未扩大本轮修改范围，不能记为全仓格式检查通过。

Core 默认忽略的四项是扫描超时子进程夹具、真实 FFmpeg 暂停取消、伪装 HLS 音频解码、交互式 UAC。两项 FFmpeg 用例已在本轮较早阶段显式运行通过，见 `results/ffmpeg-*.log`；最终冻结版的相应媒体源码未改变。扫描超时行为由调用它的测试覆盖，夹具本身不单独运行；UAC 安全桌面未自动点击。

## 安装体积与性能范围

最终 MSI 为 203427840 字节（约 194.00 MiB），安装目录 469486939 字节（约 447.74 MiB）、222 文件。主要大文件为 FFmpeg 165155840 字节、libmpv 119757824 字节和 Java modules 54030811 字节；完整列表在 `results/installer.json`。本轮没有新增播放引擎或依赖。

8 逻辑 CPU 的沙盒、UI 可见连接后约 5 秒空闲采样：整机 CPU 约 0.039%，全部产品进程 RSS 合计约 231.03 MiB。它是短采样，不能当作长期峰值或其他电脑保证。

同一 640×360、25 fps 测试视频在边下边播的约 3 秒采样：关闭虚拟 GPU 时整机 CPU 约 51.76%，启用虚拟 GPU 时约 0.77%，后者 Core 加播放器 RSS 约 149.68 MiB。对照支持本次高 CPU 来自软件渲染的判断，不证明所有格式、驱动、分辨率性能正常；保留两个完整报告：`results/final-download-playback-cleanup.json` 和 `results/download-playback-vgpu.json`。两组播放/清理检查均全部通过。

基线千任务 IPC p95 两次为 2.150/2.564 ms，Core RSS 约 35 MiB。没有与 IDM/AB 做同机同负载基准，不作性能相等声明。

## 已解决的验收环境问题

- 首次最终故障注入在创建任务后才写入断点；CreateTask 会立即调度，导致测试写入与下载竞争。已通过公开队列配置先禁用隔离队列，写好断点后再启用，断言写入前无请求，五轮重新通过。原失败保留于 `results/fixture-race-failure.json`，不把它隐藏或算作通过。
- 原工作台 480 秒总超时、边下边播参数不匹配、测试工作目录缺相对源码、Firefox 映射路径错误及 TLS 测试依赖缺失均已修正后重跑。TLS 依赖仅放隔离测试目录，未改变用户全局 Python 环境。
- 安装步骤首次停在故障注入前置条件，尚未安装；修正夹具后用完全相同 SHA 的 MSI 新建沙盒重跑通过。`results/final-summary.json` 中旧失败不改写，汇总明确由 verified 结果取代。
- 早期界面中断及遮挡的历史结论保留于 `initial-review.md` 和 `supplement-interrupted.json`；其“未完成”状态属于当时，不是本轮最终结论。

## 交付身份与未覆盖范围

- Engine SHA256：`4a3753309aba0f051278c7950133c196fb4d08ff8cc0fe67f8ffc855206a8221`
- MSI SHA256：`4c7a1a7b4b9152f93383f9c7f47c3d77c7c231f7f913edc63c4037305702caa9`
- 源码补丁 SHA256：`dbd243da2ca767a78583ebdcf950ed96740c862840b1d34a506a3abb79158e94`

`BUILD-PROVENANCE.json`、`source.patch` 与相对上述基线的实际产品源码差异一致，候选包如实记录构建时的 `git_worktree_clean:false` 和 `release_ready:false`。本地提交不改变已验收 MSI 的字节或来源记录；插件及审核源码 ZIP 与原交付哈希一致，未签名。

没有调用收费字幕云端、没有上传用户音频、没有实体投屏/TVBox 接收器验收。Firefox 的无下载项结论不能直接推广到 Chromium 全部接管路径；也未覆盖所有第三方网站、登录状态或 DRM 媒体。文件长度验证无法证明外部篡改或等长磁盘损坏不存在，需源端校验和才能验证该类损坏。

## 收尾状态

桌面只保留 `C:\Users\lee\Desktop\HLSDownloader\最新-20261023` 一份交付，包含已验收的未签名 MSI、Firefox/Chromium 插件、Firefox 审核源码及来源/哈希。最终沙盒已结束。仍为一个 `main` 分支、一个 worktree；本轮产品修复和验收证据作为同一项本地提交保存，未推送。

前次直接删除命令被自动审批拒绝（`blocked by policy`，未提供原因）。用户继续授权后，执行了带完整成品哈希、旧目录内容和 Git 跟踪检查的限定路径脚本，已清理 `.tool-cache`、插件依赖、Gradle/构建输出等约 17 GiB 生成物及两个旧桌面交付目录。具体范围、旧文件哈希及完成状态见 `cleanup.json`。

一次性收尾脚本执行成功后已移除。另有两个原有目录 `native_shell/downloads`、`native_shell/site-cache` 各仅含一个 `.hls-tasks/task-1/control` 文件（`pause` 5 字节和 `run` 3 字节），补充删除再次被自动审批拒绝，仅返回 `blocked by policy`，未操作这两个目录。`outputs/private-user-backup` 内约 4.4 MiB 原有任务/设置/注册表卸载备份保留，用户下载、其他项目及注册表均未操作。

## 2026-10-24 续查：源码已改，主动验证待确认

`93e5cd4` 已正常推送到 GitHub `main`，未改写历史。以上验收数据和桌面包仍对应该次交付，不代表下述改动已验收。

- 播放器原来未设置 `hwdec`，沿用 mpv 默认的软件解码。现设置 `hwdec=auto-safe`，只尝试 mpv 安全列表中的硬件解码方式，不支持时仍由 mpv 回退软件解码。没有更换渲染器、减少画质或引入依赖。此改动不等于修复禁用 GPU 时 WARP 软件渲染的高 CPU；需分别测量解码和渲染路径。依据为[打包时固定的 mpv 版本文档](https://github.com/mpv-player/mpv/blob/7b8915bc1d/DOCS/man/options.rst)。
- 公开下载验收对 Chromium 原来只检查 `exists=false`，可能把尚未生成成品的活动/暂停下载项误判为已清除。现要求确认前浏览器下载暂停，记录真实下载项和创建事件，接管完成后匹配下载项为空。Firefox 仍要求从未创建浏览器下载项。新断言尚未运行。
- Chromium 当前使用 MV3，在 `onCreated` 后暂停交接；[Chrome 官方文档](https://developer.chrome.com/docs/extensions/reference/api/webRequest)明确普通 MV3 扩展不能使用 `webRequestBlocking`。该路径不能宣称与 Firefox 一样在浏览器创建下载项前阻断。

本轮仅进行了源码、调用关系、官方资料和 Git 差异的静态检查。尚未恢复依赖、编译、运行测试或沙盒验收，未替换桌面安装包，也未发布新的 GitHub 交付包。已向用户请求本轮主动验证确认。

两个旧控制文件的内容和目录范围已再次核实，限定删除仍被自动审批拒绝（`blocked by policy`，无具体原因），文件保留；未采用其他方式绕过。
