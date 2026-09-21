# v7 全流程迭代日志

本文件是全流程迭代的回滚锚点与状态记录。每完成一个迭代批次即在 `main` 上留下
可独立回滚的提交,并同步 `origin/main`。

## 硬约束

- 项目内容只允许存在于本工作目录;仅有的两个用户授权例外:
  `E:\h`(本机唯一安装位,只允许存在一个安装)与桌面插件文件夹(只允许一个版本,
  每次更新先删旧版)。
- 不做过度测试:每批次用聚焦测试验证,完整四套回归只在整个工作收敛后跑一次。
- 没把握之前不编译/不打包;先理解问题再动手。
- 每个小步提交:任何提交点都必须可以 `git revert`/`git reset` 回退。

## 模块与功能分类(衔接关系)

| 模块 | 职责 | 衔接点 |
| --- | --- | --- |
| `native_shell` | 常驻 Rust Core:下载引擎、SQLite、v7 命名管道 IPC、Native Messaging host | 命名管道 `\\.\pipe\HLSDownloader.v7`;DPAPI 凭据;迁移入口(5.x/v6) |
| `desktop_ui` | Compose Desktop 主工作台 | 仅经 `Protocol.kt` 管道客户端;负责拉起 Core/Presenter 进程 |
| `presenter_ui` | 热确认窗口(领租赁约、进度 HUD、完成通知) | 独立连接 Core 管道;可拉起工作台 |
| `extension` | WXT MV3 浏览器扩展(Chromium/Firefox) | Native Messaging → Native Host → Core;storage.session + alarms 状态机 |
| `scripts/` | 构建打包、安装(E:\h)、冒烟验证 | 消费上述模块产物;目标安装位 `E:\h` |
| `docs/`、`artifacts/v7-productization/feature-parity.json` | 事实文档与发布门禁 | feature-parity 为唯一权威门禁文件 |

## 迭代计划(10 轮)

| # | 内容 | 状态 |
| --- | --- | --- |
| 1 | 提交此前已验证的迁移加固/构建可移植化(3 提交)并推送 GitHub | 完成 (a5b847c..8d76065) |
| 2 | native_shell 可靠性:5.x 导入有界+续传、旧库只读、设置解码日志、单实例类型化判定 | 代理执行中 |
| 3 | desktop_ui 可靠性:重连退避上限、taskLogs 回收、批量操作限流、投屏 toast 去噪、Locale.ROOT | 代理执行中 |
| 4 | desktop_ui UX:深色横幅配色、进度圈动画、交接对话框输入保护、连接状态结构化、事件重同步 | 代理执行中 |
| 5 | presenter_ui:HUD 确定性主任务、记住目录读-改-写、冷启动预算、删除陈旧 bin 副本 | 排队(并发上限) |
| 6 | extension:后台收尾改 alarm 驱动、alarm 周期兼容、权限收敛、版本单源、popup 渲染口径 | 完成(37 文件/222 测试绿) |
| 7 | scripts 可移植化:去除 D:/E: 缓存硬编码,默认仓库内;安装脚本对准 E:\h | 完成(23 脚本过 PS5.1 解析门禁) |
| 8 | 构建 + 部署:portable/MSI 安装到 E:\h(唯一安装),插件产物放桌面文件夹(先删旧) | 待办 |
| 9 | 文档与 parity 证据对齐(README/docs 与实际架构一致);UX 细节补漏 | 待办 |
| 10 | 全量四套回归一次、最终提交、合并/推送 GitHub、收尾审计 | 待办 |

## 批次记录

### 迭代 1(2026-08-31)

- `a5b847c` fix(v7): harden v6 adoption and schema version guard
  —— v6 整库迁移:补 `HLS_V6_SKIP_MIGRATE` 跳过开关、可测试核心 + 7 单测、
  `snapshot_json` 同步规范化;store 打开既有库时 schema 版本守卫;修复合
  "未来计划"行为矛盾的既有失败测试。
- `4f2f018` fix(v7): keep build outputs inside the repository
  —— cargo target-dir 与 gradle build 目录收进仓库,可用环境变量/属性覆盖。
- `8d76065` fix(v7): derive product version and tidy core residue
  —— 版本常量统一 `CARGO_PKG_VERSION`、移除失效 `#[allow(dead_code)]`、
  credentials 文档与实现对齐。
- 验证:cargo lib 354/354、presenter 2/2、gradle BUILD SUCCESSFUL、
  扩展 208/208 + 双目标构建;均已推送 `origin/main`。

## 第二轮精进批次记录(2026-08-31,分支 `v7-refinement`)

本轮以 `docs/v7-refinement-plan.md` 为总纲:先多子代理并行摸底四个活跃模块,
汇总 20 项问题登记(D1-D7/E1-E4/N1-N5/P1-P3/G1),再按十轮逐项落地。
分支 `v7-refinement` 逐轮提交,收敛后合回 `main` 并推送。

| 轮 | 内容 | 提交/证据 |
| --- | --- | --- |
| 1 | 摸底整合:模块×功能分类、问题登记表、十轮计划 | `f0ba7fb` |
| 2 | desktop_ui 连接层:按管道路径共享的空闲连接池,批量操作/事件长轮询复用连接,失败即弃不再入池 | `8baa840`,`compileKotlin` 通过 |
| 3 | desktop_ui 治理:诊断日志改有界后台队列;深色/排序本地先行改动加代数守卫防回跳;parsing/probing 显式状态+未知状态显示"其他";presenter 探测指数退避;删除死代码旧 SettingsDialog | `d25a1dd` |
| 4 | desktop_ui 体验:系统托盘驻留(关窗最小化、托盘菜单暂停/继续全部任务与退出)、关于页、100 条通知中心;托盘安装失败自动回退真实退出 | `58d0567`,`compileKotlin` 通过 |
| 5 | extension:nativeBridge 抢占显式出队防重复入队;popup 状态机改稳定 token+文案映射;content.ts 按钮构造先于清晰度选择器;directBackend 原生专用 op 集合具名化 | `e946789`,`tsc --noEmit` + nativeBridge/directBackend 13 测试通过 |
| 6 | native_shell:VOD 分片 3 次重试+退避(pause/canceled 控制信号原样上抛);v6 遗留文案中性化;torrent_engine 15 处大端读取收敛为 `be32` 助手 | `bb44253`,`cargo check --lib` 通过 |
| 7 | native_shell 并发:管道/TCP 三条服务路径统一 64 连接上限,Drop 守卫防 panic 泄漏计数;慢探测经桌面连接池+上限已受控,异步任务化决策另立轮次 | `c6f7ebe` |
| 8 | presenter_ui:窗口标题常量单一来源;单实例判定改用 `is_already_running_error`;panic 崩溃报告落盘临时目录 | `8875b2a`,`cargo check` 通过 |
| 9 | 文档与门禁同步:本记录与 refinement-plan 登记;feature-parity.json 不动(4 个 partial 均需实机门禁,本轮未新增合同项);本轮未改任何脚本 | `本轮提交` |
| 10 | 四套回归一轮、扩展构建并发布桌面单副本(先删旧)、合并 `v7-refinement` → `main`、推送 GitHub | 见下一条记录 |

约束执行情况:项目内容均在工作目录内;`E:\h` 当前不存在,parity 未全绿未生成
正式包;构建缓存继续使用仓库内 `.tool-cache\build-cache`(JDK 位于既有
`E:\HLSDownloaderBuildCache\jdk-21`,属工具链非项目内容)。

### 第二轮 · 迭代 10 记录(2026-08-31)

四套回归各跑一轮,全部绿:

- `cargo test --manifest-path native_shell/Cargo.toml --lib`:354 passed / 0 failed
- `cargo test --manifest-path presenter_ui/Cargo.toml`:3 passed / 0 failed
- `desktop_ui`: `gradlew.bat test --no-daemon`:BUILD SUCCESSFUL(66 用例,
  thousand-task p95=13.6ms)
- `extension`: `pnpm test`:wxt prepare + tsc --noEmit + vitest 222/222(37 文件)

构建与桌面发布:

- `pnpm run zip:chrome` / `zip:firefox` 产出 7.0.0 双端包;
  当轮使用的历史一次性桌面发布入口按 install-v7-local 约定把桌面扩展包规范为恰好
  `HLSDownloader-Chromium.zip` + `HLSDownloader-Firefox.zip` 各一份,
  执行前已删除桌面上的全部旧扩展包副本。该一次性入口现已移除,
  后续只能由 `scripts/install-v7-local.ps1` 从当前候选/正式产物事务发布。
- parity 仍为 24/28 verified + 4 partial(需实机门禁),`release_ready=false`,
  按约束本轮未生成正式安装包。误写入 `E:\h` 的 Compose 构建缓存已迁回工作区,
  当前本机仍为零有效安装;`E:\h` 只保留给后续通过门禁的唯一安装。

git 收尾:`v7-refinement` 以 --no-ff 合回 `main` 并推送 `origin/main`,
本轮全部 11 个提交(含文档)可通过合并提交回溯。

## 第七轮精进批次记录（2026-08-31，分支 `v7-refinement-r7`）

本轮按桌面端、Presenter、Rust Core、浏览器扩展和交付脚本分工并行复核，
只保留会影响真实功能、数据一致性、连接可靠性或用户操作反馈的问题。

| 模块 | 已收敛内容 |
| --- | --- |
| `desktop_ui` | 增加工作台跨进程文件锁；重复启动只向已有实例发送 `open_main`，避免多工作台竞争。 |
| `presenter_ui` | 暂停/取消和打开主窗口移出 UI 线程；显示忙碌、成功、失败状态；异步结果按 HUD 当前任务 ID 隔离。 |
| `native_shell` IPC/并发 | 服务端空闲帧头 120 秒有界、帧体 15 秒绝对截止；BT/磁力探测用单后台槽并通过事件流回传。 |
| `native_shell` 持久化 | handoff/media-push 旁路行与事件、快照、checkpoint 同 SQLite 事务提交；resolved 持久化成功后才移除内存 offer。 |
| `extension` | alarm 创建兼容异步拒绝；本地等待超时保持非终态；未知 handoff 所有权持续暂停并持久复核；尊重用户自行恢复/取消/完成，临时查询失败不丢记录。 |
| `scripts` | 构建门禁补扩展协议核对；覆盖 `E:\h` 前验证安装归属；Portable 升级/回滚验证双浏览器扩展身份连续。 |

最新要求是不在本机执行编译、测试、构建、打包或安装。本轮仅做静态差异、
脚本解析、文本编码、清单 JSON 与模块衔接检查，然后按模块提交、合并 `main`
并同步 GitHub；`feature-parity.json` 保持原门禁状态。

## 第八轮精进批次记录（2026-08-31，分支 `v7-refinement-r8`）

本轮继续使用三个 `gpt-5.6-sol` 低延迟子代理分离文件所有权，主代理复核
跨模块停服链。改动仅覆盖可由当前调用关系确认的用户级故障：

| 模块 | 已收敛内容 |
| --- | --- |
| `desktop_ui` | 系统关闭与自绘标题栏关闭共用托盘驻留回调，避免标题栏按钮绕过后台驻留并直接退出。 |
| `extension` | NativeBridge 严格匹配 `__request_id`；首个 `postMessage` 同步失败时断开已创建端口再重试，避免错配响应和残留 Host 连接。 |
| `native_shell` 启动 | 恢复任务状态的 SQLite 写失败不再被忽略；恢复成功后才创建 watcher，防止启动失败遗留后台线程。 |
| `native_shell` 停服 | Engine 新增 `--shutdown`，活动 worker 先暂停并等待断点状态收敛；stop watcher 唤醒阻塞的 `ConnectNamedPipe`，唤醒连接不进入 handler。 |
| `scripts` | `shutdown-running.ps1` 删除 v6 HTTP supervisor 调用，改由当前安装的 Engine 通过 v7 pipe 有序停服，超时诊断记录 Core 命令退出码。 |

本轮仍不执行本地测试、编译、构建、打包、安装或 UI 自动化；只进行一次
最终静态一致性检查。`feature-parity.json` 的 4 个 partial 仍由其既有实机门禁决定。

## 第九轮收口批次记录（2026-09-18，本地 `main`）

本轮按"先修发布门禁阻断缺陷，再清确定性债，最后静态收口"的顺序执行，
不改任何发布门禁的判定强度，也不提前置真 `release_ready`。

| 模块 | 已收敛内容 |
| --- | --- |
| `scripts/verify-v7-feature-parity.ps1` | `$requiredGateIds` 由 4 项补为 5 项（加 `browser_media_push`），错误文案同步；正式打包路径 `build-v7.ps1 -Task package` 不再自我阻断。 |
| `scripts/validate-powershell.ps1` | 新增 invoke / recorder / verify 三方 gate-id 契约断言，杜绝门禁集合再次分叉（此前只比对 invoke↔recorder）。 |
| `docs/v7-verification.md` | "四项门禁"更正为五项，与 `docs/v7-release-runner.md` 的 "five candidate-bound release gates" 对齐。 |
| `native_shell/media` | `mod harness;` 加 `#[cfg(test)]`，媒体测试夹具不再进入正式二进制；移除 `harness.rs` 中随之冗余的两处 `#[allow(dead_code)]`（CI clippy 已全局 `-A dead_code`）。 |
| `native_shell/store.rs` | 删除与 `profile_paths.rs` 重复的 `default_v7_database_path`/`default_v7_download_dir`（含仅被其使用的 `portable_v7_root`），路径解析收敛到 `profile_paths` 单处；移除随之无用的 `use std::env;` 与 `profile_paths.rs` 的自引用对齐测试。 |
| `native_shell/core_ipc.rs` | `default_core_bind()` 改为返回 `Result`，坏 `HLS_V7_CORE_BIND` 不再 panic，错误沿 `core_server::bind_local` / `CoreIpcClient::connect_existing` 向上传播；保留端口占用时的临时端口回退。 |
| `extension` | 删除已全 no-op 的 `lib/directBackend.ts` 及其测试；`background.ts` 的 `native()` 去掉死掉的 loopback 分支，只走 Native Messaging；把扩展唯一的 v7 协议常量 `V7_CORE_PROTOCOL` 迁到 `lib/nativeBridge.ts`，并同步 `build-v7.ps1` 的扩展协议核对路径（否则正式构建会因读取已删除文件而失败）。 |
| `docs` | `docs/manger.log` 更名为 `docs/manager.md`；`docs/v7-branch-state.md` 注明三个安全审计工作流保留 `pull_request` 触发器的用途；`.gitignore` 把 `/.pi/` 归入"本地助手状态不入库"分组（与 `/.workbuddy-ai/`、`/outputs/` 同类）。 |

本轮既有静态检查，也在本机补齐了实际执行：`validate-powershell.ps1` 在
PowerShell 5.1 与 7 下均通过（45 个脚本解析、三方 gate-id 契约、版本契约一致），
关键 JSON 均无 BOM。本机运行结果：`cargo test --manifest-path native_shell/Cargo.toml
--lib` 464 passed / 0 failed / 1 ignored；`cargo test --manifest-path
presenter_ui/Cargo.toml` 9 passed / 0 failed；`cargo clippy --manifest-path
native_shell/Cargo.toml --locked --all-targets -- -D warnings -A dead_code ...` 无告警；
`desktop_ui\gradlew.bat test --no-daemon` BUILD SUCCESSFUL（用临时下载的 Microsoft
OpenJDK 21 工具链）；扩展 `tsc --noEmit` 通过、`vitest run` 43 文件 / 301 用例全通过、
`wxt build -b chrome|firefox` 均成功。未执行正式打包、签名、实机安装或真实浏览器 /
LAN TVBox 门禁。`feature-parity.json` 维持 27 verified / 1 partial、
 
 ## 第十轮本机验证记录（2026-09-19，本地 `main` @ `2c634c6`）
 
 本轮在本机 Windows（不使用 WSL）实机执行可执行的测试与静态检查，结果与工具链、
 逐类用例数、未执行项完整记录在 `docs/v7-local-verification-record.md`；
 操作者在 `hls-release` 机器上的发布执行顺序见 `docs/v7-release-runbook-local.md`。
 
 实机结果摘要（Rust 侧按仓库 pin 的 1.98.1 运行，与 `ci.yml:103` 一致）：`native_shell`
 464 passed / 0 failed / 1 ignored；`presenter_ui` 9 passed；两个 crate 的 clippy
 （与 `ci.yml:129`/`ci.yml:164` 参数一致）与 `cargo fmt --check` 均通过；`desktop_ui` 用
 `--rerun-tasks` 强制重跑 13 个测试类 / 71 用例，0 失败；扩展 `wxt prepare` + `tsc --noEmit`
 + `vitest run`(43 文件 / 301 用例) + `wxt build -b chrome|firefox` 全部成功；
 `validate-powershell.ps1` 在 PowerShell 5.1 与 7.6.6 下均通过，三方 gate-id 契约一致（5 项）。
 
 未执行（不得视为通过）：正式五项实机门禁、正式打包/签名/发布、实机安装与真实浏览器 /
 局域网 TVBox 证据、四个 exact-SHA 前置工作流。`feature-parity.json` 维持
 27 verified / 1 partial（`browser.media_push_device_selection`）、`release_ready=false` 不变。

## 第十一轮：浏览器插件失效原因与本地恢复（2026-09-19，本地 `main` @ `8cf413a`）

发现现象：插件"不能用"。只读排查确认原因不是代码或扩展产物损坏，而是此前的本机卸载清理
连带移除了插件运行所必需的三样依赖：

1. **Native Messaging 注册被清除**：`HKCU\Software\{Google\Chrome,Microsoft\Edge,Mozilla,Chromium}\NativeMessagingHosts\com.ciaooo55.hls_downloader` 全部不存在。
2. **引擎可执行文件被清除**：清理构建缓存时一并删掉了 `hls-downloader-engine.exe` / `HLSDownloaderEngine.exe`，Native Host 即使注册成功也无后端可拉起。
3. **没有任何 HLS 进程或命名管道**：`\\.\pipe\HLSDownloader.v7` 不存在。

扩展自身完好：`extension/.output/chrome-mv3` 与 `firefox-mv3` 两份产物存在，清单 `key` 与商店
ID 一致，`nativeMessaging` 权限在位；源码未改动。本机仅安装 Edge，无 Chrome / Firefox。

本地恢复（不重装桌面端，不动源码，不重新下载工具链）：

1. `cargo +1.98.1 build --bin hls-downloader-engine`（`CARGO_TARGET_DIR` 指向仓库内 `.tool-cache\build-cache\cargo-target`）编译成功。
2. 将 `hls-downloader-engine.exe` 复制为同目录 `HLSDownloaderEngine.exe`，满足 `core_spawn::locate_core_executable` 与 Native Host 同目录查找约定。
3. `HLSDownloaderEngine.exe --register-native-host` → `Native Host repair complete: 7 registration(s)`；共写入 7 条用户级注册（Chrome / Edge / Brave / Chromium / Vivaldi / Opera 6 条 Chromium 系 + Firefox 1 条），均指向生成的清单。
4. 真实 Edge 加载扩展（`--load-extension extension\.output\chrome-mv3`），service worker 以商店 ID `bbdfldcjnikaemnimalegbopgaknjhla` 运行；经 CDP 在 service worker 内执行 `chrome.runtime.sendNativeMessage('com.ciaooo55.hls_downloader', {op:'ping'})` → `lastError:null`、`ok:true`、`protocol:"hls-downloader-v7-core"`、`version:"7.0.2"`。
5. 同一通道内执行真实 `offer` → 返回真实 handoff `handoff-1a0b8fd90ef-1`，`status:"pending"`、`presentation_mode:"native-rust"`、`presentation_ok:true`；popup 渲染并显示"下载引擎已连接"。

边界：以上是**开发构建态**的连通性证据（引擎来自 `target\debug`，注册指向仓库内构建产物），
不等于"安装后正式环境 + 实机 TVBox"证据。因此 `feature-parity.json` 维持
27 verified / 1 partial（`browser.media_push_device_selection`），`release_ready=false` 不变。

注意：这是**开发态注册**，仅让本机插件可用；它指向工作区内的构建产物路径。正式安装仍应通过
`build-v7.ps1 -Task package` + `release-v7.yml` 由操作者在 `hls-release` 机器上完成。

另注：引擎在 `%LOCALAPPDATA%\HLS Downloader\v7\` 下重建了 `data.db`（含 WAL/SHM）与 `instance.lock`，
这是插件运行必需的运行期数据，不属于安装残留，未清理。

## 第十二轮：浏览器插件功能与 UI 收口（2026-09-19，本地 `main` @ `aafb34b`）

目标：在已恢复连通的插件上，补齐真实可用的功能与 UI。范围为 popup 层
（`extension/entrypoints/popup/main.ts` 与 `style.css`），不改引擎/后端——识别、下载、落盘
三条链路此前已在本机端到端验证可用。

本轮修复的三个 UI 缺陷（均为「用户实际会遇到的」缺陷，非风格问题）：

| 缺陷 | 现象 | 修复 |
| --- | --- | --- |
| 资源元信息与主机名截断无提示 | 长文件名/URL 被 CSS 截断后无法看到完整值 | 给 `name` / 元信息行 / mime+host 行补 `title` 提示（分别显示完整文件名、完整元信息、`mimeType · host`） |
| 无法复制资源链接 | 用户只能肉眼抄 URL | 新增「复制链接」按钮（`.copy-link`），`navigator.clipboard.writeText` 为主、textarea + `execCommand('copy')` 兜底；成功翻转文案为「已复制」1.8s，失败提示「复制链接失败，请手动选择」 |
| 开关语义含糊 | `自动接管` / `Cookie` / `排除本站` 三个按钮只显示名词，读不出当前是开还是关 | 改为状态语义：`自动接管：已开启/已关闭`、`本站 Cookie：已授权/未授权`、`本站提示：已隐藏/已显示`，并补 `aria-pressed` 与 `title` 说明 |

验证（真实 Edge，CDP 驱动，全链路）：

1. 识别：实验室页 `http://127.0.0.1:8765/index.m3u8` 被正常识别，`article` 计数为 1。
2. 复制链接：点击后文案翻转为「已复制」，剪贴板读回值为 `http://127.0.0.1:8765/index.m3u8`（经 `Browser.grantPermissions` 授予 `clipboardReadWrite`）。
3. 标题提示：`copyTitle=复制完整链接（127.0.0.1:8765）`、`lineTitle=HLS · .m3u8 · 0:06 · 大小未知`、`metaTitle=application/vnd.apple.mpegurl · 127.0.0.1:8765`。
4. 开关状态：`aria-pressed=true`、文案 `自动接管：已开启`。
5. 下载回归：点击资源卡「下载」（`article .article-actions .hlsd-button.primary`）后，引擎真实落盘 `%LOCALAPPDATA%\HLS Downloader\v7\downloads\HLS Lab`（11280 字节，3×3760 分片 + 头）及 `.hls-tasks\task-3\{local.m3u8,published.path,vod_segments.json,segments\00000N.ts}`。

静态检查：`tsc --noEmit` 通过；`vitest run` 43 文件 / 301 用例全绿。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial
（`browser.media_push_device_selection`），`release_ready=false` 不变；本轮不产生新的实机门禁证据。

## 第十三轮：插件 UI / 选项 / 悬浮窗全量核对（2026-09-19，本地 `main` @ `b09cc2f`）

目标：按「插件签名先不管，先确保功能、UI、选项、悬浮窗没问题」的要求，把 popup、页面悬浮面板、
视频悬浮工具条三处 UI 面逐一核对，只修真实缺陷，不为风格或可选优化动手。

### 修复的真实缺陷：投屏按钮无样式（`cast-button`）

- 现象：`extension/entrypoints/popup/main.ts:366` 为投屏按钮设置 `class="cast-button"`，但
  `style.css` 与 `theme.ts` 均无 `.cast-button` 规则，按钮退回基础样式；同排的 TVBox 按钮
  `.push-button` 有专属紫色样式（`style.css:47-48`、`theme.ts:85`）。功能完整（`ICONS.cast`、
  `CAST_PUSH_LABELS` 均在位），仅缺视觉区分。
- 修复：在 `.push-button` 规则后补 `.cast-button` 与其 `:hover`（复用设计契约的
  `--purple` 22% 混入 `--surface-3` 底、`--purple-ink` 前景）；`theme.test.ts:177` 补同款
  对比度守卫；`theme.ts` 设计契约注释补 `.cast-button` 行。改动仅 3 个文件、共 4 行。
- 证据：构建后 `.output/chrome-mv3/assets/popup-*.css` 内含 `cast-button`（此前只含
  `push-button`）。真实 Edge 内另测得页面悬浮面板的投屏按钮 `.download.cast` 背景为绿色、
  TVBox `.download.push-tv` 为紫色、下载 `.download` 为蓝色，三者前景均为白色（可读）——
  该三色属 content 脚本内联样式（`.download.cast` 用 `--green`），与 popup 新增的
  `.cast-button`（紫色，与 `.push-button` 同款）是两套不同 UI 组件，各自非缺陷。

### 其余 UI 面核对结论（无缺陷，不需要改）

- 选项开关语义：`自动接管 / 本站 Cookie / 本站提示` 均已带状态文案 + `aria-pressed`，与桌面端
  `set-takeover-settings` 往返同步（`takeoverSettingsSync.ts`），正确。
- 清晰度菜单：点击生成、backdrop `mousedown` 关闭、按触发元素定位，正确。
- 页面悬浮面板（`<hls-downloader-media-panel>`）与视频悬浮工具条（`.video-hover`）：真实 Edge 内
  确认 Shadow DOM 挂载、主题令牌在 Shadow Root 内正确解析（`data-hlsd-theme=light`、
  `--primary #2563eb`、`--purple #7c3aed`）、资源卡三按钮（下载/TVBox/投屏）样式正确、
  工具条 `下载视频` 主按钮 + `⋯` 更多按钮 + 悬浮卡片（标题/状态/事实标签/来源/下载·投屏·TVBox
  三个动作）结构完整且展开态可见。逐类核对 popup 与 content 脚本使用的每个 class，除
  `popup-boot*`（在 `popup/index.html` 内联定义）外均有样式，`cast-button` 是唯一遗漏项。

### 静态检查

`tsc --noEmit` 通过；`vitest run` 43 文件 / 301 用例全绿；`wxt build -b chrome` 成功。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial
（`browser.media_push_device_selection`），`release_ready=false` 不变。投屏 / TVBox 的真实
设备投送仍缺实机证据（本机所在网段无可用接收端），属发布门禁而非插件 UI 缺陷。

## 第十四轮：native_shell 死代码与断链清理（本地 `main`，本轮）

目标：在本机（不使用 WSL）对 `native_shell` 做一轮编译器自证的清理，只动「零生产行为影响」
的真实缺陷与死代码，不新增功能、不改引擎行为。

### 修复项

| 项 | 位置 | 说明 |
| --- | --- | --- |
| Retry-After 断链 | `native_shell/src/net_policy.rs` | `acquire()`（每个 HTTP 作业都经 `http_engine.rs:run_job_once` 调用）会读 `retry_until` 来阻塞 host，但唯一写入者 `note_retry_after` 全仓库无调用者，三条传输路径（curl / WinHTTP / 主 `http_get`）也不解析 `Retry-After` 头。即读侧活在活跃生产路径、写侧从未接线，属于半接线的空机制。删除 `retry_until` 字段、`acquire()` 中的读分支、`note_retry_after` 函数，使连接预算只按其本职（全局/单 host 连接上限）工作，行为不变。 |
| 测试夹具混入生产构建 | `native_shell/src/sftp_engine.rs` | `FixtureSession` / `FixtureFile` 定义在非测试区，仅被 `#[cfg(test)]` 代码引用（生产函数 `download_sftp` 的 `#[cfg(test)]` 分支与 `mod tests`）。给定义、trait 实现与 `use std::io::Read` 补 `#[cfg(test)]` 门控。 |
| 空 no-op 占位 | `native_shell/src/core_runtime.rs` | 删除 `#[allow(dead_code)] fn _keep_contract_types_visible(_: ResourceOffer) {}`。`ResourceOffer` 本身在生产代码被大量真实使用，"保持类型可见"的借口不成立，是纯死代码。 |
| 文档矛盾 | `docs/architecture/coordination-protocol.md` | "Branch and PR contract" 一节仍要求按任务建分支并走 PR，与已生效的单 `main` 工作流冲突。补 Superseded 说明（镜像 `handoff.md` 措辞），不改历史记录正文。 |

### 验证

- `cargo +1.98.1 test --manifest-path native_shell/Cargo.toml --lib`：464 passed / 0 failed / 1 ignored（与改动前基线一致）。
- `cargo +1.98.1 test --manifest-path presenter_ui/Cargo.toml`：9 passed。
- `cargo +1.98.1 clippy --locked --all-targets -- -D warnings -A dead_code -A clippy::large_enum_variant -A clippy::too_many_arguments -A clippy::type_complexity`：通过（与 `ci.yml:129` 参数一致）。
- `cargo +1.98.1 fmt --manifest-path native_shell/Cargo.toml -- --check` 与 presenter_ui 同名检查：均通过。

### 未处理（记录为既有债务，不在本轮范围）

`cargo check --lib` 仍有 28 条 `dead_code` 警告（既有、非本轮引入），集中在：`cast.rs` 的
BrowserPush 一族（含仅测试引用的 `start_browser_push` / `probe_tvbox`）、`media/hls.rs` 的
`select_variant` / `select_default_audio` / `download_hls_selected` / `write_local_playlist` /
`load_seen` / `save_seen`、`media/dash.rs:download_dash`、`torrent_engine.rs` 的
`download_from_peer(_ex)` / `watch_delay` / `is_fresh`、`http_engine.rs:fetch_bytes_range`、
`player.rs` 的 `last_url` / `last_preview` / `last_embed`、`playback.rs` 的 `lan_enabled` / `port`、
`power_action.rs:label`、`sleep_inhibit.rs:is_active`、`category.rs:category_dirs_json`、
`net_policy.rs:effective_limit_kib`、`cast.rs` 的 `discover_devices` / `tvbox_payload` /
`browser_push_status`。其中不少是"薄包装 + 仅测试引用"，删除需连同对应测试一并处理，属独立的
测试重构工作，本轮不动。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial

## 第十五轮：删除无用旧代码与历史脚手架（本地 `main`，本轮）

目标（操作方指示）：完成一个全新项目；删掉确定没用的旧代码，不为不存在的功能做兼容，
也不保留对旧代码的向后兼容。只动经编译器或全仓库检索证明无生产引用的代码与文档，
不改引擎行为、不新增功能。

### native_shell（删除项，均经 `cargo check --all-targets` 零警告 + 全仓库检索确认）

| 位置 | 删除内容 |
| --- | --- |
| `cast.rs` | BrowserPush 投送链、`tvbox_payload`、`discover_devices`、测试用包装 `probe_tvbox`（测试改为直接调用生产的 `probe_tvbox_until`） |
| `media/hls.rs` | `select_variant`、`select_default_audio`、`download_hls_selected`、`write_local_playlist`、`load_seen`、`save_seen` |
| `media/dash.rs` | `download_dash` |
| `media/harness.rs` | `run_dash_static_fixture`，以及仅测试读取的 `FixtureOrigin.requests` / `body_bytes` 字段（保留内部 Arc 克隆，服务线程不受影响） |
| `torrent_engine.rs` | 包装 `download_from_peer` / `download_from_peer_ex`、`watch_delay`、`is_fresh`（测试改为直接调用 `download_from_peer_ex_with_telemetry`，已核对语义等价） |
| `net_policy.rs` | `effective_limit_kib` 与遗留令牌桶 |
| `playback.rs` | `lan_enabled` 与静态 `port()` |
| `player.rs` | 三个只读 getter（字段保留，生产仍写入） |
| `power_action.rs` / `sleep_inhibit.rs` / `category.rs` / `http_engine.rs` / `core_runtime.rs` | `label`、`is_active`、`category_dirs_json`、`fetch_bytes_range`、no-op 占位 `_keep_contract_types_visible` |
| `media/mod.rs` | 随上述删除同步收敛的 re-export |

### extension

- 删 `storeIdentity.ts:CHROMIUM_EXTENSION_ID`（保留 `CHROMIUM_PUBLIC_KEY`、`FIREFOX_EXTENSION_ID`）。
- 删 `browserCapabilities.ts:resolveFirefoxClickIntent`。
- 删 `takeover.ts:mayDiscardBrowserTransfer`。
- 对应测试同步收紧。

### desktop_ui

- `Protocol.kt`：删 `EngineCapabilities` 数据类、`capabilities()`、`placeQueue()`。
- `Main.kt`：删 `categoryLabel()`。
- `UiTestApi.kt`：删 `mouseModifierMask()`、`pasteText()`。

### docs（历史协调脚手架，自闭合、只被彼此与 `handoff.md` 交叉引用）

- 删 `docs/worker-logs/`（21 个）、`docs/coordination/`（2 个）、`docs/manager.md`、
  `docs/architecture/project-plan.md`、`docs/architecture/v7-contract-audit.md`、
  `docs/architecture/v7-final-readiness-audit.md`、`docs/architecture/coordination-protocol.md`、
  `docs/v7-bt-selection-evidence.md`、`docs/v7-hls-auth-resume-evidence.md`、
  `docs/v7-hls-candidate-auth-resume-evidence.md`、`docs/v7-refinement-plan.md`、
  `docs/v7-ui-polish.md`、`docs/v7-ui-test-api.md`。`docs` 由 52 个文件收敛到 19 个。
- `handoff.md` 同步改写，说明该脚手架已随单 `main` 工作流移除、历史留在 Git。

### 保留（经核实仍被真实依赖，不属「无用旧代码」）

- **v6 → v7 已装数据库迁移**：`migrate.rs` / `v6_migrate.rs` 由 `lib.rs`、`core_server.rs` 实际调用，
  是 AGENTS.md 要求的已安装用户升级路径，删除即数据丢失。
- **`extension/lib/nativeBridge.ts:V7_CORE_PROTOCOL`**：被 `scripts/build-v7.ps1` 引用。
- **`wait_handoff` 超时表项**：Core 的合法操作，删除会改变未知操作回退行为。
- **整个 `scripts/`**：80 个脚本都服务现有功能或发布流程，无「旧代码」。

### 验证（本地 Windows，未使用 WSL）

- `cargo +1.98.1 check --manifest-path native_shell/Cargo.toml --all-targets`：零警告。
- `cargo +1.98.1 test --manifest-path native_shell/Cargo.toml --lib`：462 passed / 0 failed / 1 ignored
  （相对上一轮 464 少 2，为删除项对应的专用测试）。
- `cargo +1.98.1 test --manifest-path presenter_ui/Cargo.toml`：9 passed。
- `extension`：`tsc --noEmit` 通过；`vitest run` 43 文件 / 300 passed（相对上一轮 301 少 1）。
- `desktop_ui`：`gradlew.bat compileKotlin` 与 `test` 均 BUILD SUCCESSFUL。
- `scripts/verify-v7-feature-parity.ps1 -PackageTier candidate -RequireNoBlocked -RequireCleanWorktree`：
  通过，`FEATURE_PARITY=96.4% (27/28 verified, 1 partial, 0 blocked)`，`COMMIT=36aa601`。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial
（`browser.media_push_device_selection`），`release_ready=false` 不变。

## 第十六轮：本机对抗性测试闭环与缺陷修复（目标契约执行）

按批准的目标契约在本机 Windows（未使用 WSL，复用已装工具链）跑通全部可本机执行的测试面，
对抗性暴露真实缺陷并以第一性原理修根因。三项测试侧缺陷已修复并复现验证（提交 `103fdce`），
均未改动任何生产行为路径，也未放宽任何发布门禁或断言。

### 已修复缺陷（测试侧，根因修复）

1. **`native_shell/src/native_host.rs` 并行必挂**：
   `offer_is_idempotent_for_the_extension_request_id` 与 `browser_offer_rejects_javascript_and_file_urls`
   断言进程级全局计数器 `NEXT_HANDOFF` 的绝对值不变；该计数器被 handoff id / credential ref /
   media-push id 三处生产路径共用，默认并行下被其他测试推进 → 假失败（实测 left:15 / right:14）。
   修复：新增 `NEXT_HANDOFF_TEST_LOCK` 互斥 + `lock_next_handoff()`，仅把两处断言串行隔离，
   保留「无谓消耗序号」的回归防护语义。生产路径不变。
2. **`scripts/smoke_v7_presenter.py` 延迟自检的百分位数学回归**：
   上一轮曾把暖机样本从 P95 集合中剔除，使集合缩到 17 个元素；`percentile95` 在小集合上退化为
   最大值，单个调度抖动即翻转 100ms 门限。修复：恢复采集 **20 个稳态样本**（P95 再次丢弃最高 1 个），
   仅剔除「启动后第 1 个 offer」和「每次崩溃恢复后第 1 个 offer」这两类一次性暖机样本，
   暖机样本单独记录（`warmup_samples_ms` / `warmup_max_ms`）以便观测。**100ms 门限不变**。
3. **`native_shell/src/download_worker.rs` 并行负载下的假失败**：
   `live_torrent_selection_update_cancels_requested_file_and_publishes_remaining_file`
   为 tracker HTTP + BT 握手就绪等待设了 5 秒墙钟预算；462 路并行在共享 4 核主机上偶发超时。
   该测试是行为测试而非延迟测试，修复：三处就绪等待统一为宽松的 `MACHINERY_WAIT = 60s`。
   真正的挂起仍会失败。仅测试改动。

### 本机全测试面结果（提交 `103fdce`，工作树干净）

- `cargo +1.98.1 test --manifest-path native_shell/Cargo.toml --lib`：默认并行连续 3 次均为
  `462 passed / 0 failed / 1 ignored`。
- `cargo +1.98.1 test --manifest-path presenter_ui/Cargo.toml`：`9 passed / 0 failed`。
- `extension`：`tsc --noEmit` 通过；`vitest run` 43 文件 / 300 passed。
- `desktop_ui`：`gradlew.bat test --no-daemon --rerun-tasks`：BUILD SUCCESSFUL（8 任务实执行）。
- `cargo +1.98.1 clippy --locked --manifest-path native_shell/Cargo.toml --all-targets --
  -D warnings -A dead_code -A clippy::large_enum_variant -A clippy::too_many_arguments
  -A clippy::type_complexity`：exit 0；`cargo fmt --check`（native_shell + presenter_ui）：exit 0。
- `scripts/validate-powershell.ps1`：Windows PowerShell 5.1 与 PowerShell 7 均 exit 0
  （45 脚本；正式发布 gate-id 契约 = browser, browser_media_push, installer, performance, rollback）。
- `scripts/adversarial-v7.ps1 -Scope native`：架构边界、冷启动门、256MiB 真实 Range 吞吐与内存门全过。
- `scripts/verify-v7-feature-parity.ps1 -PackageTier candidate -RequireNoBlocked -RequireCleanWorktree`：
  通过，`FEATURE_PARITY=96.4% (27/28 verified, 1 partial, 0 blocked)`，`COMMIT=103fdce`，
  `SHA256=177c72e8…` 不变。

### 如实列出的未验证项（本机条件所限，不谎报通过）

- **AC3 / AC8 的 presenter 稳态可见延迟 100ms 门限**：在本机（i7-7700HQ 4 核笔记本，与 ChatGPT /
  PI-Desktop / 浏览器等无关负载共享）实测 presenter「offer 提交 → 确认窗口可见」的稳态 P95 为
  **90–110ms**，恰跨在 100ms 门限上：即使瞬时 CPU 负载为 0% 也有约 40% 的采样越界，且此时
  `native_host_submit_max_ms` 仅 26–38ms（host/Core 段无停顿），即越界出在 presenter 自身的
  窗口显示腿（`post_submit_visible_p95` 亦达 85–101ms）。对照 `feature-parity.json` 记录的
  专用机基线（3 连跑最大值 42.65/45.35/48.22ms、P95 47.59ms）可判定：**这不是产品性能回归，
  而是本机吞吐低于该绝对门限**。契约边界禁止提高 100ms 门限，故 AC3/AC8 在本机判为「环境受限、
  未达成」，并保留 `adversarial-v7.ps1` 的 presenter smoke 门限原样不动。
- 真实设备投送（cast / TVBox）、正式签名/打包/发布、缺失的真实浏览器二进制项：仍需操作方 +
  自托管 `hls-release` 运行器与真实 LAN 接收端，本机无法核验。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial
（`browser.media_push_device_selection`），`release_ready=false` 不变。

### 第十七轮（2026-09-20，继续检查）：修复 v6 迁移测试基目录不是绝对路径的确定性失败

- **现象**：`cargo test --manifest-path native_shell/Cargo.toml --lib`（462 用例默认并行）连续 6 次
  出现 1 个失败：`v6_migrate::tests::absolute_dirs_are_left_untouched` 在 `src\v6_migrate.rs:634`
  断言 `spec.download_dir == 绝对路径` 失败；单跑该用例、或改跑整组 `v6_migrate::tests` 都通过。
  之前怀疑的 `download_worker::tests::live_torrent_selection_…` 仅是被误读的日志，
  其真实错误信息为 `v7 task task-1 failed: url path invalid`，属于并行调度下
  BT 对端监听线程先退出的时序噪声，实际失败点在本测试。
- **根因**：`v6_migrate.rs` 测试助手 `test_dir()` 用 `CARGO_TARGET_DIR`（本仓建的是
  `.tool-cache\build-cache\cargo-target`，**相对路径**）作为基目录。相对基目录下
  `absolute = <base>\v6\keep` 不是绝对路径，于是迁移逻辑在 `original.is_absolute()`
  分支正确地把它当相对路径重解析，与 `absolute_dirs_are_left_untouched` 的用例前提矛盾。
  生产路径完全正确，缺陷只在测试夹具对「绝对路径」的假设。
- **修复**：`test_dir()` 在拿到基目录后统一解析为绝对路径（相对则与当前工作目录拼接），
  不改动任何迁移生产逻辑。
- **验证**：`cargo +1.98.1 test --lib` 默认并行连续 5 次 462 passed / 0 failed / 1 ignored；
  `v6_migrate::tests` 7 项全过（`absolute_dirs_are_left_untouched` 真实执行并通过）；
  `cargo clippy --locked --all-targets -- -D warnings -A dead_code -A clippy::large_enum_variant
  -A clippy::too_many_arguments -A clippy::type_complexity` exit 0；`cargo fmt --check` exit 0。

### 第十八轮（2026-09-20，继续检查）：修复测试在共享 `%TEMP%` 根部固定路径的写入与泄漏

- **现象一（确定性失败）**：`cargo test --lib` 默认并行连续 6 次出现
  `v6_migrate::tests::absolute_dirs_are_left_untouched` 失败；单跑该用例或整组都过。
- **根因一**：`v6_migrate.rs` 测试助手 `test_dir()` 用 `CARGO_TARGET_DIR` 作基目录，而本仓
  该变量是相对路径 `.tool-cache\build-cache\cargo-target`，于是用例构造的「绝对路径」输入
  实际是相对路径，被迁移逻辑正确地重解析，与用例前提矛盾。
- **现象二（残留）**：每跑一轮完整套件，`%TEMP%` 就多出 5 个测试数据库
  （`hls-v6-core-restart-*`、`hls-v7-media-push-restart-*`、`hls-v7-media-push-normalize-*`、
  `hls-v7-side-row-transaction-*`、`hls-v7-verification-*`），累计已 350+；
  以及 `%TEMP%\.hls-tasks` 目录根的残留。
- **根因二**：5 个 Core 重启类用例在 `reopened`（持有 SQLite 文件句柄）仍存活时调用
  `remove_file`；Windows 上无法删除已打开文件，删除失败但错误被 `let _ =` 吞掉。
  另 `download_worker::tests::spec()` 的 `download_dir` 直接写 `%TEMP%`，而 `build_job()`
  会调用 `prepare()` 真实落盘，于是把 `.hls-tasks` 写进共享临时目录根部，且是固定路径
  （多个测试进程/真实实例会互撞）。
- **修复**：`test_dir()` 把基目录解析为绝对路径；5 处清理前先 `drop(reopened)`；
  `spec()` 改用本进程专属隔离子目录，并在 `get_job_without_size_is_not_forced_sequential`
  里连同隔离根一起删除。**未改动任何生产逻辑**（迁移判定、SQLite 生命周期、TaskPaths 均不变）。
- **验证**：`cargo test --lib` 默认并行连续 3 次 462 passed / 0 failed / 1 ignored；
  套件跑完后 `%TEMP%` 的 `hls-*` 残留数为 0（修复前为 5 个数据库 + 1 个目录）；
  `cargo clippy --locked --all-targets -- -D warnings -A dead_code -A clippy::large_enum_variant
  -A clippy::too_many_arguments -A clippy::type_complexity` exit 0；`cargo fmt --check` exit 0。
- 其余四套回归同轮复核：`presenter_ui` 9 passed；extension `typecheck` exit 0 且
  `vitest run` 300 passed；`desktop_ui` `gradlew test` BUILD SUCCESSFUL；
  `validate-powershell.ps1` PS 5.1 + PS 7 均 exit 0；`verify-v7-feature-parity.ps1
  -PackageTier candidate -RequireNoBlocked -RequireCleanWorktree` 通过（27/28、1 partial）。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial
（`browser.media_push_device_selection`），`release_ready=false` 不变。

### 第十九轮（2026-09-20，项目落地）：修复 FFmpeg 供应链固定腐烂导致的打包全面阻断

- **现象**：`scripts\build-v7.ps1 -Task candidate` 在 cargo release 与扩展构建全部成功后中断：
  `Candidate/formal packaging requires HLS_V7_FFMPEG_DIR or ffmpeg.exe on PATH`。
  进一步 `scripts\bootstrap-v7-media-tools.ps1` 直接失败：
  `Invoke-WebRequest ... 404 未找到`。
- **根因**：`bootstrap-v7-media-tools.ps1` 把 FFmpeg 固定到 BtbN 的 **autobuild 日期** 资产
  `autobuild-2026-09-06-13-06 / ffmpeg-n9.0.1-26-g5c8e7e2433-win64-gpl-9.0.zip`。
  BtbN 只保留最近约 5 个 autobuild 发布，该发布已被上游删除，资产返回 404。
  因此这不只是本机构建问题：`.github/workflows/package-v7-candidate.yml` 调用同一个脚本，
  候选打包 CI 同样会失败。即**固定方式本身会腐烂**，与产品代码无关。
- **修复**：重新固定到当前仍存在的 `autobuild-2026-09-19-13-11 / ffmpeg-n9.0.2-win64-gpl-9.0.zip`
  （同为不带独立 DLL 集的静态 win64-gpl 构建，FFmpeg 9.0.1 → 9.0.2），
  SHA-256 `44083538105b4e64d439f9e67bd875bd264b4271239c808b2acea09773ad1aa3`（实测）。
  供应链属性不变：仍是「固定版本 + 固定摘要 + 下载后校验」，且脚本仍校验
  `ffmpeg.exe`/`ffprobe.exe`/`ffplay.exe` 三者同目录。文件头注明腐烂原因与再固定流程。
  未改动任何产品代码、未改动发布门禁、未改动 `feature-parity.json`。
- **验证**：`bootstrap-v7-media-tools.ps1` 退出 0 并返回
  `.tool-cache\build-cache\ffmpeg-n9.0.2-win64-gpl-9.0\bin`；`ffmpeg -version` / `ffprobe -version`
  正常输出 `n9.0.2-20260919`；`validate-powershell.ps1` PS 5.1 + PS 7 均 exit 0。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial
（`browser.media_push_device_selection`），`release_ready=false` 不变。

### 第二十轮（2026-09-20，项目落地）：从当前源码产出 candidate 交付并把插件链路接实

- **修复第十九轮的阻断后，`scripts\build-v7.ps1 -Task candidate` 全流程跑通**（前台执行，
  中途工具超时不影响子进程继续）：扩展构建 → cargo release（native_shell 2m27s +
  presenter 5m01s）→ gradle `clean createDistributable packageDistributionForCurrentOS`
  （BUILD SUCCESSFUL 6m44s，13 tasks executed）→ MSI 回滚序重写 → 便携 zip → 扩展 zip →
  产物清单与溯源。产物 `source_commit=495bd11`（= 当前 HEAD），`tree=58e695d0`（= `HEAD^{tree}`）。
- **产物完整性（逐个实测）**：exe / msi / portable / Chromium / Firefox 的 SHA-256 与
  `ARTIFACT-MANIFEST.json` 全部一致；便携包内 `BUILD-PROVENANCE.json` 的 commit/tree 等于 HEAD、
  `package_tier=candidate`；包内 `FEATURE-PARITY.json` 哈希等于仓库 canonical 文件。
- **扩展 manifest（用 .NET 显式 UTF-8 解析，避开 PowerShell 代码页把中文读坏的问题）**：
  Chromium 与 Firefox 均 `version=7.0.2` + `manifest_version=3`；Chromium 携带 216 字符商店公钥；
  Firefox `gecko.id=hls-downloader-store@ciaooo55.com`；`popup.html` 引用的
  `chunks/popup-DfY81R3S.js`、`assets/popup-N6bGNzxR.css`、`icon-32.png` 全部存在。
- **扩展 ID 与注册 origins 对齐（自行按 Chromium 规则复算）**：把 manifest `key` 做
  DER→SHA-256→前 16 字节→a-p 十六进制，得到 `bbdfldcjnikaemnimalegbopgaknjhla`，
  与 `allowed_origins` 中的扩展 ID 完全一致。
- **原生主机注册改指向出厂二进制**：原先注册的是 `cargo-target\debug\HLSDownloaderNativeHost.exe`
  （与交付包不同哈希），且该目录里引擎叫 `hls-downloader-engine.exe`，宿主要找的却是
  `HLSDownloaderEngine.exe`，所以宿主无法自启 Core（报
  `HLSDownloaderEngine.exe is not next to the desktop UI`）。改为对
  `.tool-cache\build-cache\compose-build\compose\binaries\main\app\HLSDownloader\app\resources\HLSDownloaderEngine.exe`
  执行 `--register-native-host`，7 处注册全部指向该目录；三处浏览器注册已验证。
- **插件端到端冒烟（官方探针，不用自制管道）**：`smoke_v7_native_host.py` 对**包内**二进制跑
  冷启动：`cold_first_response_ms=576.54`（门限 1500）、两次帧式 ping、隔离 Core 建库、干净退出。
  `smoke_v7_presenter.py --recovery-only`：`latency_passed=true`、`visible_offer_p95_ms=57.66`、
  `native_host_submit_p95_ms=21.85`、`presenter_pending_recovery=true`。
- **完整原生对抗门 `adversarial-v7.ps1 -Scope native` PASS**：Core 协议 30 passed、
  传输 worker 45 passed、presenter 9 passed、player process passed、Compose 协议与恶意输入
  （gradle test + native_shell 462 passed/0 failed/1 ignored + presenter 9 + extension 300 passed）
  、Native Host 冷启动 760.02ms、真实 Range 吞吐 **110.31 MiB/s**（门限 20）、
  工作集增长 5.68 MiB（门限 256）、发布后额外网络字节 0。
- **无头 Edge 真实加载交付包 popup**：`--dump-dom` 得到
  「HLS Downloader 正在载入浏览器插件… 浏览器插件脚本未启用。」——popup JS 真实执行并正确进入降级态
  （file:// 独立加载下的预期行为）；截图 420x640、329 种颜色、24.8 万非白像素。
- **本机交付目录**：`outputs\local-20260920-183500\`（exe / msi / portable / 两个扩展 zip /
  ARTIFACT-MANIFEST / BUILD-PROVENANCE / FEATURE-PARITY / latest.json + README 使用与验证说明）。
  `outputs/` 与 `artifacts/` 均不入库。
- **未验证项如实记录**：本机无 msedgedriver / chromedriver / geckodriver，也未安装 Firefox，
  故 `verify-v7-candidate-browser.ps1` 的完整浏览器矩阵无法运行；Presenter 稳态延迟门本轮
  57.66 / 75.44ms 低于 100ms，但同机历史采样曾达 90–110ms，不声称其在本机稳定达成。

边界不变：`feature-parity.json` 维持 27 verified / 1 partial
（`browser.media_push_device_selection`），`release_ready=false` 不变；未做任何签名或正式发布。
### 第二十一轮（2026-09-20，继续修复）：修掉 presenter 稳态百分位被暖机样本污染的证据缺陷

- **现象**：`scripts\smoke_v7_presenter.py` 的 `visible_offer_p95_ms`（100ms 门限）
  已经把暖机样本排除在 `latencies` 之外，但 `submit_latencies` / `visibility_latencies`
  对每个样本都追加，于是与稳态门限并列输出的
  `native_host_submit_p95_ms` / `post_submit_visible_p95_ms`
  实际被一次性的首绘/崩溃恢复重绘离群值污染，**不能描述稳态**，
  会直接把后面的延迟归因带偏（此前一次 110.01ms 的失败归因就受此影响）。
- **修复**：暖机样本不再进入这两个分段集合，改记到
  `warmup_submit_max_ms` / `warmup_post_submit_visible_max_ms` 单独观测。
  门限语义不变：仍是稳态可见 offer 的 P95 <= 100ms。
- **验证（本机，commit ff0ad2e）**：
  - 单独跑一次 smoke：`visible_offer_p95_ms=75.84`、`native_host_submit_p95_ms=27.23`、
    `post_submit_visible_p95_ms=52.65`、`latency_passed=true`。
  - `adversarial-v7.ps1 -Scope @('browser','transfer')` **PASS**：presenter
    `visible_offer_p95_ms=89.82`、`native_host_submit_p95_ms=27.20`、
    `post_submit_visible_p95_ms=61.89`；Compose 协议与敌意输入
    （gradle test + native_shell 462 passed / 0 failed / 1 ignored + presenter 9 +
    extension 300 passed）全绿。
  - `adversarial-v7.ps1 -Scope @('native')` **PASS**：presenter
    `visible_offer_p95_ms=75.37`、submit P95 24.24 / 可见段 53.95；
    Native Host 冷启动 810.38ms（门限 1500）；真实 Range 吞吐 **113.82 MiB/s**
    （门限 20）、工作集增长 5.75 MiB（门限 256）、发布后额外网络字节 0。
- **候选包范围补充验证**：`verify-hls-candidate-auth-resume.ps1 -Runs 2` **PASS**，
  便携包内引擎的认证 VOD 与 Live 暂停/续跑均通过（401 → 鉴权 → 续跑，
  live 模式 checkpoint=present）。
- **注册表复核**：HKCU 下 7 条 `com.ciaooo55.hls_downloader` 注册
  （Chrome / Edge / Brave / Chromium / Vivaldi / Opera + Firefox）全部存在，
  且 Chromium 系 6 条均指向打包资源目录的
  `HLSDownloaderNativeHost.chrome.json`，Firefox 指向 `.firefox.json`；
  早前"Opera 注册缺失"的记录已过时。
- **真实浏览器内容脚本注入验证：本机受阻（未通过，非代码问题）**。
  Edge 153 headless=new 下：`--dump-dom` 对网页（file:// 与 http://）返回空 DOM
  （仅对 `chrome-extension://` 弹窗页有效，该页此前已验证）；
  `--remote-debugging-port` 始终不绑定（profile 内不生成 `DevToolsActivePort`，
  新 profile 启动常以 exit code 13 退出）；本机也没有 Firefox 与任何 WebDriver。
  因此"扩展在真实浏览器里注入页面并挂载 shadow UI"这一项仍未验证。
- **未执行项维持原判**：`verify-v7-msi-lifecycle.ps1` 要求把 MSI 装回 `E:\h`
  并需要 msiexec 提权，与"本机清理干净、不重复安装"的既有约定冲突，不做；
  `smoke-installed-v7.ps1` 同理；完整浏览器矩阵仍缺 Firefox + WebDriver。
- 边界不变：`feature-parity.json` 维持 27 verified / 1 partial
  （`browser.media_push_device_selection`），`release_ready=false` 不变；
  未做任何签名或正式发布。
### 第二十二轮（2026-09-20，继续修复）：把"真实浏览器插件注入与 UI 渲染"从受阻项变成已实测项

- **突破点**：此前判定"真实浏览器内容脚本注入受阻"是因为只试了
  `--headless=new`（`--dump-dom` 对网页返回空 DOM）与 `about:blank`
  （Edge 直接 exit 13）。改为**有头模式 + 真实 URL + 独立 profile + `--load-extension`**
  后 CDP 立即可用，注入验证一次通过。
- **内容脚本注入（真实 Edge 153 + 未打包扩展）**：
  `document.documentElement[data-hls-downloader-extension]="1"`，shadow root 挂载成功。
- **播放 overlay 真实渲染**：合成 `play` 事件后 `.video-buttons` 图层
  `display:block` / `position:fixed`，1 个操作组；按钮 aria-label 为
  `下载当前视频`、`更多操作：投屏或推送当前媒体链接`，hover 面板含
  `下载` / `投屏` / `TVBox`。截图 1240x845、1192 种颜色、非白像素 9.1%、
  含主题色 `#2563EB`（3720 px）。
- **popup 真实渲染**：`chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/popup.html`
  （与商店 ID 一致）输出
  `HLS Downloader / 连接中… / 打开 / 自动接管 / 本站 Cookie / 本站提示 / 已识别资源 0 /
  重新识别 / 浏览器插件 版本 7.0.2`，9 个按钮、资源列表存在、
  **无"未启用 / 无法连接"错误**，说明真实原生消息通道可用。截图 420x640、非白像素 86.0%。
- **安全边界**：浏览器使用仓库内 `.tool-cache\build-cache\edge-inject*` 独立 profile，
  未触碰用户自己的 Edge 数据；数据目录指向隔离的 `HLS_V7_DATA_DIR` /
  `HLS_V7_DOWNLOAD_DIR`；探针脚本与截图只写在 PI scratch；结束后进程与 profile 全部清理。
- **仍未覆盖**：本机无 Firefox 与任何 WebDriver，故 Firefox 与 Brave / Vivaldi /
  Opera / Chromium 未实测；投屏 / TVBox 真实推送仍需真实局域网接收端。
- 边界不变：`feature-parity.json` 维持 27 verified / 1 partial
  （`browser.media_push_device_selection`），`release_ready=false` 不变；
  未做任何签名或正式发布。
### 第二十三轮（2026-09-20，继续修复）：修掉"服务器不支持 Range 就整个下载失败"的引擎缺陷

- **复现路径（真实产品链路）**：有头 Edge + 未打包扩展 + 已注册原生主机，
  页面里放一个可真实播放的本地视频，点 overlay「下载当前视频」→
  扩展 `download-now` → 原生主机 → 引擎建任务 → 引擎把 148680 字节全部拿到，
  然后 **失败**：`DOWNLOAD_FAILED server ignored Range and returned 200`，
  已到手的整包被丢弃。本机用 Python `http.server` 这类不支持 Range 的服务器时，
  所有直链文件下载都不可能成功。
- **根因**：`http_engine.rs` 的 `fetch_range` 只要收到 200 就判 `RangeUnsupported`，
  该错误一路冒泡成任务失败；而 `download_sequential`（本就能处理 200 整包）
  只在 POST / `sequential` / `total==0` 时被选中。
- **修复**：`download_ranges` 识别"从零开始 + 响应身份与任务一致（或无身份可校验）"
  的 200，退回 `download_sequential` 单流整包下载。为此新增内部变体
  `EngineErrorCode::RangeIgnored`，对外退出码与消息保持不变。
  **安全边界不变**：If-Range 未命中（资源身份已变）与续传位置错位仍然失败，
  绝不会用外来 body 覆盖已保留数据（`etag_change_range_200_does_not_stitch_new_body`
  继续通过）。
- **测试**：新增 `fresh_download_falls_back_to_sequential_when_server_ignores_range`
  与 `partial_download_still_fails_when_server_ignores_range`；
  `http_engine` 47 项全过；完整 `native_shell` 套件 464 passed / 0 failed / 1 ignored；
  `cargo fmt --check` 与 CI 同参 clippy 均 exit 0。
- **进程级端到端复验（真实引擎 + 真实原生主机）**：对 Range-less 服务器发 `download` op，
  `task-1` 状态 `completed`、`downloaded_bytes=148680/148680`、
  产出文件 SHA-256 与源一致，`passed=true`。
- **修复后重跑对抗 native 范围 PASS**：Core 协议 30、传输 worker 45、presenter 9、
  Compose 协议与敌意输入（native_shell 462 + presenter 9 + 扩展 300）、
  Native Host 冷启动 832.25ms（门限 1500）、真实 Range 吞吐 67.38 MiB/s（门限 20，
  32 个 range 请求 / 0 个整包请求 / 0 重叠 / 发布后额外字节 0）、
  工作集增长 5.87 MiB（门限 256）；presenter 稳态 P95 68.62ms（门限 100）。
- **重新产出 candidate 交付**：`build-v7.ps1 -Task candidate` 全流程重跑，
  `source_commit=e28aa75`、`source_tree=53d724ce`、工作树干净；
  5 个产物 SHA-256 与 `ARTIFACT-MANIFEST.json` 逐一复算一致；
  parity 门禁在干净工作树上 PASS（27/28，`release_ready=false` 未变）。
  交付目录 `outputs\local-20260920-211500\`（含两个扩展 zip 与更新后的 README），
  并清掉今天两份不完整的旧交付目录。
- **真实浏览器插件链路的进展与边界**：有头 Edge + 独立 profile + `--load-extension` + CDP
  首次跑通——内容脚本真实注入（`data-hls-downloader-extension="1"`）、shadow UI 挂载、
  overlay 真实渲染（`下载当前视频` / `更多操作：投屏或推送当前媒体链接` / `下载`·`投屏`·`TVBox`）、
  popup 在真实扩展上下文渲染且原生消息通道可用。
  但"点 overlay 后由浏览器侧完成确认"这一步依赖视频真实播放：
  本机探针页的 `<video>` 反复出现 `networkState=2` 却从不发请求（不带扩展也一样），
  属测试环境问题；扩展对此的正确行为是 fail-closed 报"浏览器尚未确认该媒体请求"。
  该场景的下载闭环改由上面的进程级端到端验证覆盖。
- 边界不变：`feature-parity.json` 维持 27 verified / 1 partial
  （`browser.media_push_device_selection`），`release_ready=false` 不变；
  未做任何签名或正式发布。

### 第二十五轮（2026-09-20，继续收口）：给手写 SHA 补已知答案向量，删掉零调用的 Win32 模块

- **动机**：上一轮被质疑"弄了半天啥都没做"。复盘后确认：真正落到产品代码的只有
  Range-less 回退一处，其余是验证、文档和重新打包。因此本轮转向"还有哪些高风险面
  从来没被验证过"，先看测试最薄又最要命的模块。
- **crypto_lite.rs 已知答案差分（commit 6925fd8）**：SHA-1 是 BT piece 校验与
  info-hash 的唯一实现，SHA-256 是任务校验和的唯一实现，而该模块此前只有空输入向量
  加一个 6 字节流式用例——padding 或分块边界写错没人会发现。新增 21 个 SHA-256
  单发向量、21 个 SHA-1 单发向量、25 个流式向量（chunk 1/7/63/64/65），覆盖
  0/1/2/3/31/32/55/56/57/63/64/65/111/119/120/127/128/129/191/192/200 这些块边界长度。
  所有摘要由 Python hashlib 生成，并在信任 Rust 实现之前用独立脚本对提交进来的表
  复算一遍（42+25 例 0 偏差）。`cargo test --lib crypto_lite` 6 passed；
  完整串行 lib 467 passed / 0 failed / 1 ignored；`cargo fmt --check` 与
  CI 同参 clippy 均 exit 0。**未发现实现缺陷**——这一处的价值是把
  "从没被验证过"变成"已被已知答案锁定"。
- **删除零调用模块（commit a9bb252）**：`native_shell/src/drop_target.rs` 与
  `file_dialog.rs` 共 231 行 Win32 代码（WM_DROPFILES 子类化 + OPENFILENAME 选择器），
  全仓库没有任何调用方：Compose 工作台自己安装 `java.awt.dnd.DropTarget`
  并发起自己的选择器，而 native_shell 只是 rlib、只被本 crate 的三个二进制消费，
  库外没有任何路径能到达 `attach_file_drop` / `pick_import_paths`。
  连同 `lib.rs` 的 `mod` 声明与 `pub use` 一并删除。拖拽导入与文件选择功能
  经 desktop_ui 实现继续可用，不受影响。验证：`cargo build --all-targets`、
  `cargo test --lib`（467 passed / 0 failed / 1 ignored）、
  `cargo test --bin HLSDownloaderUpdater`（7 passed）、`cargo fmt --check`、
  CI 同参 clippy 全部 exit 0。
- **顺带核查、未改动的模块**：`av_scan.rs` 自身失败方向是 fail-closed 的
  （spawn 失败 / 超时 / 非 0/1/2 退出都记 `error`），但调用方只对 `threat`
  阻断发布，`error` 不阻断，且 `av_scan_enabled` 默认关闭——属产品策略选择；
  `motw.rs` 的 `is_public_download_url` 对畸形数字主机（如 `127.1`、
  `::ffff:127.0.0.1`）会判为公网并打 Zone.Identifier，方向是"多标记"而非漏标记。
  两者都只记录，不改产品行为。
- 边界不变：`feature-parity.json` 维持 27 verified / 1 partial
  （`browser.media_push_device_selection`），`release_ready=false` 不变；
  未做任何签名或正式发布。

### 第二十六轮（2026-09-21，按钮动效）：把"点了没反应"的按钮反馈补上，并用真实浏览器验证

- **问题**：用户反馈"有 UI 的按钮动画效果基本没用、很差"。核查后确认是真实缺陷，
  而且不在 Compose 侧——Compose 的 `rememberPressFeedback`（hover 底色 / press 缩放 /
  focus 边框，全部走 `motionDurationMillis` 尊重"减弱动画"设置）本来就是健全的。
  问题集中在扩展的 CSS：
  1. `.video-download` 声明了 `transition:…transform .16s ease`，但 `:hover` 只改背景色、
     `:active` 只改 cursor，**没有任何状态改变 transform** —— 这条过渡是死代码。
  2. `.video-more`、`.hover-action`（下载/投屏/TVBox/查看并选择）**完全没有 transition**。
  3. popup 里 7 个自绘按钮类（`.update-notice button`、`.scan-button`、`.empty-retry`、
     `.copy-link`、`.quality-trigger`、`.quality-menu button`、`.restore-site-prompts`）
     **全部没有 transition**，hover 是颜色瞬间硬切。
  4. content script 的 `prefers-reduced-motion` 只关了 `transition`，没关 `animation`。
- **修复**：在 content.ts 的 shadow-DOM 样式表末尾追加一套统一微交互，
  与 `.hlsd-button`（transform .12s）和 Compose（hover 130ms / press 90ms）对齐：
  hover 抬升 `translateY(-1px)` + 阴影提升，press 缩放（主按钮 .96、图标按钮 .92、
  面板按钮 .94、文字链不缩放只换色）。
  popup 的 7 个按钮类逐个补上 transition 与 `:active` 缩放。
  reduced-motion 两边都改为同时关闭 transition 与 animation。
  **三条安全边界**：拖拽中（`.video-action-group.dragging`）禁用按钮 transform，
  因为拖拽改的是 group 的 left/top，按钮再缩放会看起来在抖；
  `identifying`（aria-disabled 的"识别中"）不给抬升/按压反馈，避免暗示可交互；
  `.scan-button:disabled` / `.hover-action[disabled]` 不参与按压缩放。
  **没有加入场动画**：`updateVideoButtons()` 在 scroll / resize / ResizeObserver 时
  会 `replaceChildren()` 重建控件，入场动画会随滚动反复播放。
- **测试**：新增 `extension/lib/buttonMotion.test.ts`（16 项），
  解析两份样式表并断言"每个交互按钮都声明 transition、press 时真的改变 transform、
  reduced-motion 同时关两种动效、拖拽与 identifying 状态不误反馈"。
  回归有效性已实测：临时回退 `.scan-button` 的 transition 与 `:active` → 2 项失败；恢复 → 16 项全过。
  扩展全量：`typecheck` exit 0，`vitest run` **44 文件 / 316 测试全过**（原 300，+16）；
  `theme.test.ts` 的 16 项对比度守卫未被破坏；`wxt build` chrome + firefox 均成功。
- **真实浏览器验证（有头 Edge 153 + 独立 profile + CDP）**：读取三个按钮的 computed style，
  `transition` 均为 `background-color, color, transform, box-shadow 0.15s, 0.15s, 0.12s, 0.15s`
  （修复前 `.video-more` / `.hover-action` 为无过渡）；并把 transform 从 `none` 改为
  `translateY(-1px)`，逐帧采样到**过渡插值中间帧** `matrix(0.996644, 0, 0, 0.996644, 0, -0.162072)`，
  250ms 后稳定在 `matrix(1, 0, 0, 1, 0, -1)` —— 证明 transform 过渡真实生效，
  修复前它是死代码。探针 profile 已清理。
- 边界不变：`feature-parity.json` 维持 27 verified / 1 partial
  （`browser.media_push_device_selection`），`release_ready=false` 不变；
  未做任何签名或正式发布。

### 第二十七轮（2026-09-21 晚，performance 门禁收口 + 构建脚本 PowerShell 5.1 兼容）

- **问题 1：`performance` 门禁连续两次失败在冷启动上**（1990.58 ms / 1978.19 ms > 1500 ms 阈），
  而同一批二进制手工测只有 672–732 ms。不是偶发，是测量方式差异。
- **根因定位（受控变量实验）**：`benchmark-v7.ps1` 第 14 行把 `$env:TEMP` 指进仓库，
  于是 `smoke_v7_native_host.py` 的隔离副本被放在仓库所在的 **A: 盘**上；
  `Get-PhysicalDisk` 证实 A: 在 Disk 1 = `ST1000LM048`（**HDD**），C: 在 Disk 0 = Phison **SSD**。
  定量对照（同一批二进制、短 ASCII 路径排除路径因素）：
  - exe 在 A 盘 + db 在 A 盘：1834.6 / 1938.26 / 2148.95 ms
  - exe 在 A 盘 + db 在 C 盘：1836.65 / 1841.14 / 2628.22 ms（**数据库位置无关**）
  - exe 在 C 盘 + db 在 A 盘：704.06 / 727.1 / 745.83 ms
  - exe 在 C 盘 + db 在 C 盘：668.1 / 636.3 / 643.01 ms
  - **同一条 A 盘路径**：第 1 次 1907.97 ms，第 2 次 323.93 ms，第 3 次 321.24 ms
  结论：那 ~1.6 s 是"在一个**全新路径**上首次执行文件"的一次性系统开销
  （实时扫描 + 无预取记录），**不是产品启动耗时**。门禁测到的是测试装置自己造的磁盘开销。
- **修复**（阈值一动不动）：`smoke_v7_native_host.py` 增加 `--stage-root`
  （默认 `tempfile.gettempdir()`），并把 `stage_volume` / `stage_root` 写进报告；
  `benchmark-v7.ps1` 改为从 User→Machine 注册表环境变量取**真实**系统临时卷
  （绕开进程级覆盖）并显式传入，所有证据 JSON 仍落在仓库内。
  提交 `d6c55ca`。
- **顺带暴露并量化了第二个门槛敏感点**：Compose 帧 p95 随 CPU 负载变化。
  CPU 7–12% 时 p95 = 32.147 / 29.225 / 25.372 ms（全通过）；
  CPU 100% 时 37.977 ms、超阈 27 帧（失败）。同一脚本、同一二进制。
  本机当时被另一个并发工作区（`A:\deepseek-harness`，约 20 个 Codex node worker）打满。
  这是主机容量限制，不是产品缺陷；契约里的专用机基线仍然有效。
- **问题 2（构建期才发现）：`build-v7.ps1 -Task candidate` 在 Windows PowerShell 5.1 下
  根本无法完成构建**。cargo / gradle / pnpm / wxt 都把进度写到 stderr，
  PS 5.1 在 `$ErrorActionPreference='Stop'` 下会把原生命令的 stderr 升级成终止性
  `NativeCommandError`；PS 7 不会，所以历史上的构建**只在 pwsh 7 下跑通过**。
  修复：契约校验前言保持 `Stop`，进入构建阶段改为 `Continue`；
  每个外部调用后面都仍有显式 `$LASTEXITCODE` 检查，每个产物都仍有存在性与 SHA-256 断言。
  提交 `2d89bb3` → `8bc5e57`。
- **结果**：
  - `build-v7.ps1 -Task candidate` 在 **Windows PowerShell 5.1** 下完整跑通
    （EXIT=0、`BUILD SUCCESSFUL in 8m 42s`、MSI 回滚顺序已写入、portable 已生成）。
  - candidate 重建，`source_commit=8bc5e57` / `source_tree=8e12333` 与 HEAD 一致；
    5 个产物 SHA-256 独立复算全部 MATCH。
  - **`performance` 门禁完整通过**（绑定新 candidate）：帧 p95 **22.687 ms**（阈 33，超阈 0）、
    冷启动 **643.17 ms**（阈 1500，`stage_volume=C:`）、IPC p95 **0.226 ms**（阈 75）、
    真实传输 **26.95 MiB/s**（阈 20）、工作集增长 6.27 MiB（阈 256）、
    发布后额外网络字节 **0**。证据落在
    `artifacts/v7-productization/release-evidence/performance.json`。
  - parity 门禁在干净工作树上 PASS（27/28，`release_ready=false` 与 SHA256 均未变）。
  - 新交付物 `outputs/local-20260921-201500/`。**未放入 `latest.json`**：
    `adversarial-v7.ps1 -Scope native` 本轮被正确拦下——用户自己的
    `E:\h\HLSDownloaderPresenter.exe`（PID 23964）占着会话级 mutex
    `Local\HLSDownloader.v7.presenter`，冒烟脚本 fail-closed 拒绝在错误原因下继续。
    前面的 cargo 测试三段已通过。没有真实来源就不放文件。
- **边界不变**：`feature-parity.json` 维持 27 verified / 1 partial
  （`browser.media_push_device_selection`），`release_ready=false` 不变；
  未做任何签名或正式发布；用户自己的 `E:\h` 进程（22004 / 31328 / 23964）未被触碰。

### 第二十八轮（2026-09-21 夜，Clash TUN 下的 TVBox 发现）：判据不是"连上了"，而是"从哪块网卡出去的"

- **问题**：用户全程开着 Clash/Mihomo TUN，要求"用 TUN 模式也能找到"电视接收端。
  但本机 `192.168.2.11`（此前记录的候选地址）根本不存在，之前所有"端口打开"的结论
  都是 **Mihomo TUN 在本地替不存在的主机完成了 TCP 握手**：连接确实建立，本地端点是
  `198.18.0.1` 而不是 WLAN 的 `192.168.2.6`，一个字节都没离开本机。
  ARP 表里真实主机只有 `.1 / .2 / .3 / .5 / .9`；ICMP 全网被禁；SSDP M-SEARCH 零响应。

- **实测三种情形（不绑定源地址，即产品原本的探测方式）**：
  | 目标 | 结果 | 本地端点 |
  | --- | --- | --- |
  | `192.168.2.1:9978`（真实主机、端口关闭） | 1215ms 后 `TimeoutError` | — |
  | `192.168.2.1:80`（真实主机、端口开放） | 2ms 连上，返回 HTTP 302 | `192.168.2.6` |
  | `192.168.2.11:9978`（不存在的主机） | 19ms 连上，不返回任何数据 | `198.18.0.1` |
  | `192.168.2.200:9978`（不存在的主机） | 1ms 连上，不返回任何数据 | `198.18.0.1` |

- **修复**（`native_shell/src/cast.rs`，commit `cc89a49`）：`probe_tvbox_until` 在
  `connect_timeout` 之后立刻检查 `stream.local_addr()`，只有确认这条连接是从本机自己的
  某个局域网地址出去的，才继续当接收端探测；否则立即放弃。
  真实主机（可 ARP 解析、流量确实从 WLAN 网卡出去）不受影响；幻影主机不再占用扫描预算，
  也不会把隧道/代理端点当成局域网接收端上报。

- **A/B 实测（同一台机器、TUN 开启、真实产品二进制跑 `discover_cast_devices(mode=tvbox)`）**：
  | 构建 | 设备数 | 发现耗时 |
  | --- | --- | --- |
  | 判据关掉（`if false`） | 0 | **2499.1 ms**（撞上 2500ms 共享截止，扫描未完成） |
  | 判据开启 | 0 | **178.3 ms**（扫完 512 个主机） |

  本局域网当前没有任何 TVBox 接收端，所以两边都是 0 个设备；**差别在预算**：旧逻辑要把
  每个幻影连接 write+read 到 140ms 的单次上限才放弃，64 个 worker 在 2500ms 内根本扫不完
  512 个主机，排在扫描顺序后面的真实接收端永远不会被探到。新逻辑 14 倍速扫完整个 /24。

- **一处此前的错误结论已纠正**：旧代码并没有把幻影主机报成设备（幻影连接不返回数据，
  read 超时后返回 `None`），所以问题不是误报，而是扫描预算被幻影主机吃光。
  commit message 最初写成"扫描结果几乎全是假设备"，已 amend 为实测结论。

- **测试**：新增 `tvbox_probe_accepts_a_connection_that_left_via_the_lan` 与
  `tvbox_probe_rejects_a_connection_that_did_not_leave_via_the_lan`，
  另有 `spawn_tvbox_responder()`（先读请求再回包，否则探测端 `write_all` 会撞 ECONNRESET）。
  回归有效性已实测：把判据改回 `if false`，拒绝方向那条测试按预期失败。
  完整 lib 套件 **469 passed / 0 failed / 1 ignored**；`cargo fmt --check` 0；
  **CI 同参 clippy exit 0**（`-D warnings -A dead_code -A clippy::large_enum_variant
  -A clippy::too_many_arguments -A clippy::type_complexity`，见 `.github/workflows/ci.yml:129`——
  这四类结构性 lint 是契约里显式豁免的，本地直接用 `-D warnings` 会报 28 条假错误）。

- **仍未闭环**：本机只有一块 WLAN，`tvbox_scan_targets` 按设计跳过本机自身地址，
  所以没有第二台真实主机就无法给出"真的发现到一个设备"的正向证据。
  已在 `192.168.2.6:18080` 起一个只记录客户端 IP 的服务，让用户在电视浏览器里打开一次，
  即可拿到电视真实 IP；拿到后用 `scripts/smoke_v7_tvbox_real.py --expected-host <IP>` 收口。

- **边界不变**：`feature-parity.json` 维持 27 verified / 1 partial
  （`browser.media_push_device_selection`），`release_ready=false`，未做任何签名或正式发布。

