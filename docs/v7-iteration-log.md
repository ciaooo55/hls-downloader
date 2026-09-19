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