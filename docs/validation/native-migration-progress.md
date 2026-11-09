# 原生化实施记录

实施范围以 [原生化计划](../architecture/native-migration-plan.md) 为准。

## 基线

- 起点：`main`，`412f1f98`，包含用户已存在的未提交修改；没有重置、暂存或提交。
- 完整源文件摘要：`artifacts/native-migration/baseline/source.json`。
- 基线源码清单 SHA-256：`bb846c917ce2d4d4cb0b0fe7ec2bb4b71de3cba729a7ee584d63a84be05ec7e7`。
- 既有候选包、既有测试记录与本次验证分开，不将历史结果计为新架构通过。

## 阶段状态

| 阶段 | 状态 | 说明 |
|---|---|---|
| P0 | 部分完成 | 已保存源码清单、锁和旧架构五个 Release 组件；完整离线包及性能基线仍待补 |
| P1 | 已实现，组合验收继续 | 五成员 workspace、协议与客户端拆分；依赖边界、版本合同及 Core/Host 回归通过 |
| P2 | 进行中 | 扩展 592 项及 Chrome/Firefox 构建通过；Chrome、Edge、Firefox 的真实 Host/Core 本地鉴权交接各 50 检查点通过，Firefox 重载/强杀 Core 恢复专项通过；真实账号与最终包矩阵未齐 |
| P3 | 进行中 | 有界分页、事件缺口重同步、关键事务 FULL、播放器代数及实际 HWND 嵌入已验证；投屏代数与来源恢复的本机模拟回归通过，新 Release 性能未测 |
| P4–P5 | 进行中 | 恢复原版图标、配色和空状态；列/工具栏编辑器已接入，真实视频及本机模拟投送通过，完整功能映射仍有缺口 |
| P6–P7 | 验收继续 | 原生 Release 五组件构建通过；Core/CLI 免费功能隔离矩阵 10/10 通过。此后 UI 统一样式增量单独构建验收；未替换正式主入口，未删除 Compose/JRE；完整离线包、安装升级和最终验收未完成 |

`release_ready=false` 保持。真实投屏、TVBox 和真实云字幕质量按既有用户排除执行；不计为实测通过。


## 已执行的验证（2026-11-06）

- P1：workspace 全目标 `cargo check --offline --locked`、生产依赖边界、版本合同、PowerShell 修改文件解析通过。
- P1 客户端回归：client 44 passed / 1 ignored（交互 UAC）、Updater 7 passed、protocol 11 passed。见 `artifacts/native-migration/p1-client-tests.log`。
- P3 完整相关 Rust 回归：Core/Host 641 passed / 11 ignored，Core 入口 2 passed，模拟局域网 8 passed，并发创建 1 passed，交接恢复顺序 1 passed；协议与轻客户端也通过。见 `p3-rust-tests.log`。ignored 不计为通过。
- 关键强同步新增回归通过：事务内部 FULL、常规进度 NORMAL、检查点注入失败后的原子回滚及恢复 NORMAL。见 `p3-durability-test.log`。
- 扩展最终全套 54 文件 / 575 项通过，含 TypeScript 检查；Chrome、Firefox 均构建成功。见 `p2-extension-tests.log`、`p2-extension-build.log`。
- 真实 Edge、Firefox 定向媒体夹具通过：HLS MSE、LL-HLS MSE、两个 DASH 播放器、多个 Worker 的归属与面板关联。见 `p2-edge-adaptive.json`、`p2-firefox-adaptive.json`。这些结果不替代带凭据的 Host/Core 成品下载验收。
- 原生 UI 13 项回归通过，包含设置存储键、选择、旧事件拒绝和进度排序。见 `p5-ui-tests.log`。该次通过早于后续投屏窗口和悬停预览增量，不能覆盖这些增量。

## 真实 Core 与原生媒体（2026-11-06）

证据：`artifacts/native-migration/p5-real-native-media.json`，`passed=true`。脚本为 `scripts/validation/smoke_native_media.py`，使用隔离数据目录、本地鉴权 HTTP、真实 FFmpeg 与 libmpv，不读取生产任务。

- 真实下载 7,871,363 字节；成品 SHA-256 为 `9c357fd62312938401ec7d1d1baaa383f59d37ca0783fa499f59cba9ca78f7b6`。本地服务器核对 Authorization、Cookie、Referer、Origin、User-Agent 和签名查询。该证据不包含浏览器或 Native Host。
- 暂停、seek、1.5 倍速、音量 40、音轨切换和 JPEG 预览通过。下载完成后播放的额外源站请求为 0。
- 实际 GUI 完成主窗→画中画→全屏→退出全屏→主窗；视频 host HWND 为 985572，父窗口在主窗 5704092 与画中画 6817330 之间切换；会话代数始终为 1。暂停位置保持 142.6 秒。
- 发现并修复了已启动的独立 mpv 未真正迁入宿主的问题：初次嵌入显式更新 `wid`，后续只移动相同宿主。旧失败证据 `p5-real-native-media-standalone-defect.json` 保留且标记为失败。
- 原生队列窗口完成新增“新队列 1”、保存、关闭、重开；最终 Core 重读含该队列。工具窗口 owner 使用实际主窗 HWND。
- 字幕 Key 保存/读取/清除、字幕偏好往返、站点凭据与默认 Cookie 保存/清除通过；响应不含已存明文，没有启动云端会话。这是 Core 合同验证，尚不等于新设置窗口的完整 GUI 验收。
- 脚本确认 Core、UI 和隔离目录已清理；播放器子进程随 Core 关闭。

该次二进制身份见报告：Core `f29f5106…cfaf`，UI `db34c1c2…312c9`。随后新增投屏 UI、悬停预览与自定义身份头表单；需独立完成编译和定向回归，不能沿用上述二进制身份。

## 本轮实现与未完成项

- 新增浏览器/通知/更新/关于窗口、重复任务和电源动作裁决、字幕与站点凭据专用设置。更新安装与电源动作没有在本机执行。
- 投屏/TVBox 来源及设备选择、会话控制、浏览器媒体推送原 ID 恢复的本机模拟回归通过，见下方更新。真实接收端仍属于排除项，不登记为实机通过。
- 33 项逐项状态见 `native-feature-mapping.md`。列布局/工具栏编辑器已接入；拖出/剪贴板文件、输入法/Tab/读屏、混合 DPI、完整 Host/Core 浏览器交接和工具业务 GUI 验收仍未完成。
- 最新原生候选已获得 UIA 控件树，并完成中文搜索与部分按钮操作；不能据此声称完整无障碍验收通过。
- 同源码 P0 完整包与性能、新 Release 五组件、P6 入口替换和 P7 安装升级回滚仍未完成。Compose/JRE 保留。

## 当前增量与中断记录（2026-11-06）

- 投屏来源元数据由 Core 保持，状态查询返回相同代理地址、标题和任务身份；停止后清空。`p5-cast-source-test.log` 的 13 项投屏相关回归通过，`p5-lifecycle-ui-tests.log` 的 14 项 UI 回归通过，`p5-lifecycle-build.log` 的 Core/UI dev 构建成功（8 分 17 秒）。
- `p5-cast-source-roundtrip.json` 中本机模拟 TVBox 实际 Range 读取、旧地址撤销、旧代数停止拒绝和状态来源恢复断言通过。GUI 恢复推送并成功发送；最终查询遇到测试 TCP 空闲连接断开，本报告整体失败。脚本已改为最终查询前重新连接，尚未重跑，不能登记整体通过。
- `p5-workbench-lifecycle.json` 的千任务恢复游标与单实例激活通过；用户按 Escape 停止 Computer Use，关闭主窗操作没有完成。随后清理测试进程，生命周期断言失败，不能视为关闭释放行为的有效验收。
- 上述构建后又修正了主窗隐藏时详情刷新和待返回种子元数据的生命周期保留，尚未重新编译。新增 `ui/layout_options.slint` 仍是待接入组件，不能算已交付配置功能。
- 用户明确指出当前原生测试界面缺少原版图标与视觉样式。原 Compose 界面及图标资源保留，正式入口未切换。接下来须按原版外观还原原生工作台，不能将简化的测试布局算作视觉迁移完成。
- 本次测试 Core/UI 进程已停止；未提交、推送或发布。

## 原版外观恢复与定向复验（2026-11-06）

- 原生工作台复用项目现有工具栏 SVG 和应用图案，恢复图标导航、分组/数量、工具栏、任务类型图标、进度条、状态徽标与空状态。窄窗收为图标侧栏，筛选为空可重置；任务菜单按 Core 的可用动作启用。正式 Compose 入口仍保留。
- 外观设置已接入列显示、宽度、顺序、重置，以及工具栏显示、顺序、重置。继续使用原有 `task_column_layout` / `toolbar_actions` 格式，仅保存修改键，名称列与新建入口必留。设置不再要求手工编辑这两个序列化字段。
- 修复图标/进度条垂直定位，以及深色时标准输入框和复选框没有跟随主题的问题。最终 UI SHA-256：`707d52e02fb34ff916eab0960c00a0420882c40e0f020081f736af2f62a7115e`。
- `p5-visual-ui-tests.log`：15 项 UI 逻辑回归通过；之后只校准 Slint 定位和主题，最终源码全目标检查 `p5-visual-check.log`、构建 `p5-visual-aligned-build.log` 通过。没有重复运行未受影响的逻辑测试。
- `p5-workbench-appearance.json`：千任务、浅/深色、约 1402/812 像素窗口、命名队列计数、中文搜索、筛选空图案、重置和菜单资格已实际观察。长时间 GUI 检查超过生命周期等待时限，**报告整体仍为失败**；视觉观察与生命周期结果分开记录。
- 同一 UI 的 `p5-empty-workbench-lifecycle.json` 独立复验 **passed=true**：真空列表图案、禁用开始/暂停、主窗关闭后控制器释放、同进程重新激活和重建通过。该证据不代表 UI 进程已按需退出。
- `p5-cast-source-recheck.json` **passed=true**：真实 Core、本机模拟 TVBox、Range 字节哈希、旧代数停止拒绝、来源恢复、替换/停止后的地址撤销通过；GUI 按原推送 ID 恢复并发送，最终原 ID 状态为 done。脚本最终查询重连解决了旧报告的测试连接空闲超时，没有重放投送操作；不包含实机或组播发现验收。
- `p5-media-appearance.json` **passed=true**：最新 UI 与 Core `c11747ca…b0` 完成真实本地鉴权下载、成品哈希、播放器控制与 HWND 嵌入；GUI 主窗/PiP/全屏往返的会话代数始终为 1，暂停位置保持 152.2 秒，额外源站请求为 0。本次未修改外观设置，不能用该报告证明外观保存往返。
- 早期外观报告 `p5-visual-workbench.json` 保留失败标记，记录已修正的图标定位和深色控件问题；以上独立报告不覆盖旧失败结果。

## 空闲投屏状态修复与最终检查（2026-11-06）

- 实际工作台启动时误显示“当前没有投屏会话”。调用路径为 `CastTools::new` → `ControlCast(status)` → `cast::control_session`：没有接收端时返回错误，随后显示到主窗。现将空闲 status 返回为正常的未活动会话；播放/停止等操作仍拒绝不存在的会话，旧代数检查保持。
- 扩展现有替换会话回归，保护停止后查询的未活动状态与拒绝继续播放。`p5-idle-cast-tests.log`：46 项投屏相关测试通过，0 ignored；`p5-idle-cast-build.log`：Core dev 构建通过。
- 最新 Core SHA-256：`9c43029faecc1d0cd9f5a57e227fbafefef008c9964982c8ab53a1421b3241ac`；UI 仍为上述 `707d52e0…15e`，没有重跑未变源码的 UI 构建。
- `p5-idle-cast-roundtrip.json` **passed=true**：新 Core 的模拟接收端 Range、来源恢复、旧代数拒绝、旧地址/停止后地址撤销，以及停止后 status 返回未活动状态通过。本次无待恢复推送，不替代上次 GUI 推送证据。
- `p5-appearance-core-roundtrip.json` **passed=true** 的范围为自动本地鉴权下载、成品哈希、真实播放器控制、嵌入和空闲投屏无错误。源站额外请求为 0，测试进程与隔离目录清理完成。外观配置仍为空：界面检查遇到窗口最小化，恢复操作触发 Computer Use 输入保护，已停止界面输入并清理夹具；**未完成真实 Core 的列/工具栏保存→关闭→重开复验**，不计为通过。
- 三个原生验证脚本的 Python AST 检查通过；相关源码及整个已有 tracked diff 的 `git diff --check` 通过。未执行新 Release 体积/性能、正式入口切换、安装升级或发布。

## 原生工作台视觉精修（2026-11-06）

- 针对界面层次平、字号小和控件零碎的问题，重新调整浅/深色配色、文字层级、留白、边框和轻阴影。增加顶部应用标识，任务区使用独立面板，状态列保留淡色徽标，文件类型使用带底色的图标块。
- 工具栏、搜索、导航、标题操作与任务行采用统一尺寸；宽窗侧栏 216px、窄窗图标栏 64px。任务行改为 64px，同时同步拖动排序的距离换算和可用列宽计算；已有列布局序列化格式不变。
- 设置窗口统一为导航、分区内容和固定底部操作栏，新建表单统一字号与间距；多行地址/请求头保留最小高度。图案和工具栏 SVG 复用现有资源，没有增加图片或图标依赖。
- 真实窗口检查中修正了工具栏/底部按钮贴上沿、搜索占位文字居中、选中标记默认居中及多行输入被压缩的问题。早期 `p5-refined-workbench.json` 与 `p5-refined-aligned-workbench.json` 保留 `passed=false` 和缺陷说明，自动合同通过不等于视觉通过。
- `p5-refined-ui-selection-build.log`：最终原生 UI dev 构建通过。UI SHA-256 为 `1554caadc8d228c9ef0c29740225c1635c145265d8d39bff82941d6ffd835357`。本次未重复运行下载引擎、浏览器和无变化的 UI 逻辑测试。
- `p5-refined-visual-review.json` **passed=true**：最终二进制千任务 IPC 恢复、单实例激活、关闭释放和重建通过；实际检查约 1402px 窗口的浅/深色、工具栏居中、左侧选中标记、中文搜索和筛选空状态，以及重置恢复列表。
- `p5-refined-compact-review.json` **passed=true**：同一二进制约 1026px 窗口的图标侧栏、浅/深色真空列表、禁用开始/暂停、新建入口及表单多行区域、取消返回，以及关闭释放/重建通过。两个夹具的测试 UI 进程和隔离目录均已清理。
- 这是原生工作台、设置和新建表单的视觉增量，不是完整原生迁移验收；正式入口、`release_ready=false` 和其余功能验收缺口按前文保持。没有提交、推送或发布。

## 设置与工具窗口统一、标题栏主题（2026-11-06）

- 统一设置、队列、详情、新建、字幕/凭据、系统、投屏与资源工具的图标标题、间距、表单和固定底部操作区。复用原版 PNG/SVG，浅/深色核心配色对照 Compose；未增加图片或图标依赖。
- 网络设置增加完整站点规则表单：域名、启停、顺序、并发、限速、代理、目录、User-Agent、Referer、Origin 与登录信息入口。保留旧逐行格式、未知字段和最新凭据引用；改域名不继承原域名凭据，无效数值阻止保存，凭据保存未完成时阻止规则抢先提交。
- `p5-dialog-final-tests.log`：18 项 UI 逻辑回归通过，包含旧格式全字段往返、凭据更新/改域名隔离和无效输入拒绝。该次测试早于下面的标题栏与空闲连接增量。
- 用户截图确认：主窗和设置内容为深色时，Windows 原生标题栏仍为浅色。统一主题入口现同步内容和 Winit 原生装饰；主窗显示、子窗 HWND 创建后和后续主题切换均应用；画中画首次显示继承主窗主题。保留系统拖动、缩放、最大化/关闭行为。
- 工作台及独立投屏命令连接在发送前回收闲置超过 60 秒的客户端，低于 Core 的 120 秒空闲关闭时限；不重放已发送的写操作。新行为尚未完成实际空闲复验。
- `p5-caption-theme-check.log`：原生 UI 全目标检查通过；检查中发现的 unused import 随后移除。最终源码 `p5-caption-theme-build.log` dev 构建通过，无警告，耗时 9m36s。
- 当前 UI SHA-256：`85330be875d1fde3bed6418de5e043c67ce45c088d504e27ea7c8685bca3ae9d`；Core 未改，仍为 `9c43029faecc1d0cd9f5a57e227fbafefef008c9964982c8ab53a1421b3241ac`。
- `p5-native-dialogs-roundtrip.json` 保留 **passed=false**：旧候选实际观察了深色设置和外观页面，但 owned-window 输入/捕获失焦，列/工具栏、规则和队列的完整保存复验未完成；原生浅色标题栏缺陷由用户截图确认。
- `p5-themed-dialogs-roundtrip.json` 保留 **passed=false**：新二进制启动后，Computer Use 报告用户物理 Esc 并要求停止；未取得新程序窗口观察，不宣称标题栏视觉、设置保存或空闲重连实测通过。两次夹具的测试 UI/Core/HTTP 进程和隔离目录均清理；失败日志和报告保留。
- 验证脚本 AST 检查通过，修正异常分支也记录目录清理状态。正式入口仍为 Compose，`release_ready=false`；完整原生功能验收继续保持未完成。未暂存、提交、推送或发布。

## 标题栏复验与临时目录收尾（2026-11-06）

- 复核候选 UI 仍为 `85330be8…ae9d`，与 `p5-caption-theme-build.log` 的成功构建一致，没有无变化源码的重复编译。
- `p5-dialog-cleanup-review.json` 保留 **passed=false**：真实 Core/UI 正常启动，但截图返回无关前台应用；Raise/激活恢复后反复报告 `window is minimized`、`user input was detected`。没有向无关窗口输入，也没有把 UIA 控件树当作标题栏视觉通过的证据。设置保存断言未完成，保持失败。
- 此次 UI/Core/HTTP 均退出，隔离目录 `hls-native-tools-9buppz4d` 和结束标记已清理。测试失败报告与日志保留。现有未提交源码未重置、暂存、提交或推送。
- 对历史 Edge 目录 `hls-extension-smoke-qdo2l27h`，只读核对 37 个文件、无进程命令行引用、无 reparse point；随后精确目录删除被自动审批以 `blocked by policy` 拒绝，目录保留，没有换工具或命令绕过。`hls-extension-smoke-arqg0ob0` 按此前拒绝状态保留，未重试。
- 标题栏最终视觉、完整设置保存往返仍未实测通过；需要稳定桌面前台后继续。正式入口和完整迁移状态不变。

## 标题栏明确配色与系统接口复验（2026-11-07）

- 在原有 Winit 主题设置之外，统一标题栏入口增加 Windows DWM 的 `DWMWA_USE_IMMERSIVE_DARK_MODE`、`DWMWA_CAPTION_COLOR` 与 `DWMWA_TEXT_COLOR`。背景和文字直接取当前界面色板，覆盖应用主题与系统主题不一致的情况；主窗和子窗 HWND 创建后均应用。保留系统原生拖动、缩放和窗口按钮，没有增加图片、图标或 crate 依赖。
- `p5-caption-colors-build.log`：最终原生 UI dev 构建通过，耗时 20m42s，无警告。UI SHA-256：`15804b4b62665092ba7ec9227a96ed506d30ec0f5ebf18cdf9084187fdcb602a`；Core 未改，仍为 `9c43029faecc1d0cd9f5a57e227fbafefef008c9964982c8ab53a1421b3241ac`。
- `p5-caption-api-roundtrip.json` **passed=true**，范围为真实 Core + Windows 原生标题栏 API：浅色→深色→浅色三轮中，主题、背景和文字属性调用的 HRESULT 均为 0。后两轮由 Core 保存 `dark_mode` 后的真实设置事件触发。浅色背景/文字为 `#fcfdfe` / `#0f172a`，深色为 `#22262a` / `#f4f5f6`；调用记录在同名 `.caption.log`。验证机器为 Windows 11 build 22621，旧 Windows 未实测。
- 该接口回归未注入桌面输入，报告明确 `visual_verified=false`、`ui_input=false`，不能代替实际标题栏截图、子窗逐一观察或设置表单保存验收。`p5-dialog-foreground-review.json` 保留 **passed=false**：用户置前后，Computer Use 仍返回 `foreground window did not report a process id`，重新绑定并激活同一 HLS 窗口后仍失败，已停止桌面输入。
- 两个夹具的 UI/Core/HTTP 均停止，隔离目录与结束标记均清理完成；失败报告与诊断日志保留。Python AST 和 `git diff --check` 通过。历史两个 Edge 临时目录仍按上节的自动审批拒绝状态保留，没有重试删除。
- 正式入口仍为 Compose，`release_ready=false`；完整原生迁移、完整设置保存往返和本次标题栏视觉验收尚未完成。未暂存、提交、推送或发布。

## 浏览器接管与 Release 功能复验（2026-11-08）

- 扩展类型检查、54 文件 / 592 测试和 Chrome/Firefox MV3 构建通过。Chrome、Edge、Firefox 的真实扩展→Host→Core 隔离凭据交接各 50 检查点通过，报告分别为 `p6-chrome-cdp-context-handoff.json`、`p6-edge-cdp-context-handoff.json`、`p6-firefox-final-context-handoff.json`。
- Firefox 待确认凭据请求经历扩展重载、Core 强杀重启后仍保留原 handoff，未新增任务，最终成品哈希正确；`p6-firefox-durable-recovery-verified.json` 为 53 检查点通过。随后增加的一次性表单恢复场景尚未复跑，不计入该结果。
- `p5-final-settings-roundtrip.json` 仍为失败：列隐藏、批量工具启用、新建队列与凭据引用保留均通过，站点限速输入未完成；用户无法手动操作，未用 API 注入数值冒充 GUI 保存。
- Release workspace 五组件构建通过，耗时 64m08s，证据为 `p7-release-build.log`、`p7-release-source.json`、`p7-release-binaries.json`。该次 Presenter 构建早于随后 UI 样式修改，不代表新样式已编译或验收。Core `--self-test` 通过。
- `p7-free-parity.json` 的 Core/CLI 隔离矩阵 10/10 通过：CLI 启动、暂停/重启恢复、导入导出、队列、凭据脱敏、ZIP 有界预览与抓站/离线/周期同步。隔离运行时已清理。这些是组件验收，不代替对应 GUI 操作或完整安装包验收。

## 基线组件

旧架构 Release 成品及 SHA-256 在 `artifacts/native-migration/baseline/bin` 和 `baseline/binaries.json`。Core 15,462,912 字节，Presenter 28,807,168 字节，CLI 3,743,232 字节，Host 3,880,960 字节，Updater 2,249,728 字节。2026-11-08 的新架构 Release 大小与身份另见 `p7-release-binaries.json`；该 Presenter 早于下面的新样式。本轮没有性能对比，不能据此声称加速。

## 控件样式统一与真实保存复验（2026-11-08）

- 针对用户截图中主题入口、设置表单和小型工具标题栏的差异，将主窗按钮、弹窗按钮、输入框、下拉框的基础高度收敛到 36px，统一圆角、交互色和表单间距。显式固定 Slint `fluent` 控件样式；确认窗输入及复选框复用标准控件，保留编辑与焦点能力，没有新增依赖或图片。
- 右上设置与主题入口使用相同外框、尺寸；深色显示太阳、浅色显示月亮，提示说明将切换到的主题。设置增加分区面板、目录输入与浏览按钮间距，关闭表单横向滚动，切分组仍回到顶部。新建、队列、字幕/凭据、资源与投屏表单使用共享尺寸和间距。
- 普通 owned 弹窗清除 `WS_EX_TOOLWINDOW`，保留 owner 和主窗任务栏归属，恢复正常标题栏及窗口按钮。原生 DWM 标题栏颜色继续跟随内容主题，保留系统移动、缩放与关闭能力。
- `p5-ui-unified-build.log`：新 UI dev 构建通过，无警告，耗时 14m10s；`--self-test` 返回 `hls-v7-presenter/1 ok`。源码清单 `p5-ui-unified-source.json` 的 SHA-256 为 `dc56a39d2da15840c69b6f49b8d748b6c07b4826e6525d6c857f990bfbf27188`，UI 二进制 SHA-256 为 `e90c0ffb9f78d02d64a3f260c9d7eb802f028f1e64c789ffcc25e5c5c76f1874`。此前 Release Presenter 不包含本轮新样式。
- `p5-ui-unified-roundtrip.json` **passed=true**：新 UI 与 Release Core 的独立夹具中，通过真实 UI 隐藏大小列、启用批量入口、将站点限速输入并保存为 512、新建并保存队列；Core 最终重读五项持久化检查全部通过，包括原站点凭据引用仍在、无合成 Cookie 明文泄漏。没有用 API 注入这些表单修改。
- 实际观察浅/深色主窗和设置窗、深色队列与新建表单；设置关闭重开后主题正常，网络分组从外观底部切入时回到顶部。浅→深→浅标题栏三轮 DWM 属性检查通过。截图和观察范围见 `p5-ui-unified-visual.json`，本次未完成 GUI 新建下载、逐页业务、混合 DPI 和完整键盘/读屏验收，不扩大通过范围。
- 夹具报告确认 UI、Core、HTTP 均已停止，独立数据目录及结束标记已清理；日志、截图与失败历史保留。正式入口仍为 Compose，`release_ready=false`，未提交、推送或发布。

## 全窗口样式与图标复核（2026-11-08）

- 检查全部 16 个 Slint 模块，将工作台、表单、系统页、投屏、播放器及热弹窗的图标收敛到同一套共享组件；保留原有工具栏 SVG 和应用图案。共享图标共 52 个，14 类导出窗口均设置应用图标，静态检查未发现缺失的嵌入图片或字面图标引用。源码检查记录为 `all-pages/source-review.json`，不代替实窗观察。
- 网络、安全、字幕、BT、通知等入口使用对应语义图标；播放器使用带名称和提示的图标按钮，保留播放、暂停、跳转、静音、音量、倍速、轨道、字幕、预览、画中画和全屏入口。确认、进度、完成窗口的文字符号改为共享图标。按钮补齐键盘焦点及 Enter/空格操作。
- 详情、资源与投屏页面关闭横向滚动；资源和队列保持最小可用宽度；投屏长操作纵排，确认提示内容可滚动。播放器上下控制区扩大，避免 36px 控件超出原有容器。
- 初检候选 `7d4394af…66d` 构建与自检通过，但实窗发现共享按钮横向拉伸，以及字段较少的设置页把单行输入框纵向拉高。已修正按钮自然宽度、固定单行控件为 36px，并让设置内容从顶部排列。初检截图带 `-initial` 后缀，不能作为修正后验收。
- 第一轮修正候选源码清单为 `p5-all-pages-layout-source.json`，135 个输入文件，源码 SHA-256 为 `c9093324b27344966c699977355a619176ac8cc3fc7141130d2e4434b933ede9`。该轮构建通过，无告警，耗时 10m24s，自检通过；UI SHA-256 为 `c938b4f9d562d3a5b671e76a77c486ed92fe68fa882e9b62406f672ea4ab5102`。实际观察主窗、11 个设置分组和工具窗口；发现的问题记录在 `all-pages/verified/review.json`，不把这些中间截图当作后续修正通过的证据。
- 全页复验发现并修正资源空状态图标偏左、投屏初始窗口过小/大窗纵向拉伸、系统说明挤出关闭栏/关于首行截断。资源图标使用居中容器，投屏内容顶端排列；投屏和系统页通过 Rust 的初始逻辑尺寸保留用户缩放及 Slint 最小尺寸，系统长说明统一进入滚动面板。`p5-all-pages-refinement-fixed-build.log` 构建通过，无告警，耗时 16m32s，自检通过；源码清单为 `p5-all-pages-refinement-source.json`，源码 SHA-256 为 `a406f73afd5534b1707395a943825bbfda779e609e6ac0c3263317d1d09578a4`，UI SHA-256 为 `1d36a2a62fda8c5b689e40aff3e43a8edcab4b1b1fcfae9f3cee112137f47f3f`。该候选实窗复核资源四模式、投屏来源/会话与最大化、系统四页、浅/深色主窗正常。
- `p5-all-pages-refinement-roundtrip.json` 通过：真实 UI 恢复本地 1MiB HTTP 下载，成品 SHA-256 与夹具一致；UI/Core/HTTP 进程及独立数据目录已清理。资源读取、抓站和种子操作仅检查窗口布局，没有宣称完整 GUI 业务往返。
- `p5-all-pages-media-roundtrip.json` 通过：合成鉴权视频下载哈希一致，真实 libmpv 暂停、seek、倍速、音量、音轨和预览帧检查通过；实际操作主窗→PiP→全屏→退出全屏→主窗，保持同一会话，播放未额外请求下载源。UI/Core/播放器和独立目录已清理。进度与完成窗另使用生产组件视觉夹具查看，没有真实电源动作。
- 后续实窗发现重复完成任务仍显示内部状态码、打开按钮误标为继续，以及 PiP 首次打开仍使用紧凑工具窗标题栏。已修正中文说明与打开按钮；无明确动作的重复任务跳转到已有任务，避免发送无效动作。独立播放器保留普通标题栏和自身任务栏身份，无边框临时交接弹窗继续隐藏任务栏。独立确认窗视觉夹具保留正常任务栏身份，便于桌面检查工具选择，不改变生产交接窗口行为。
- 闭环构建 `p5-all-pages-closure-build.log` 通过，无告警，耗时 6m58s，自检通过。源码清单 `p5-all-pages-closure-source.json` 覆盖 135 个输入，SHA-256 为 `9f71e5e11c8159d0ad568b5a4a7b1f0c263469f410949f464ed313401440dcfe`；UI SHA-256 为 `495f8548e424bd9168ac289c72a406abb02ac62e6b5fe9870bc29ac47ebe8c30`，身份另存 `p5-all-pages-closure-binary.json`。最终源码重读与清单一致。
- 最终候选实际复验确认窗及展开详情、重复任务中文说明与取消、播放器主窗/PiP/全屏/返回；PiP 首次打开有应用图标及完整系统按钮，主窗最小化后 PiP 仍可用，回到主窗可恢复原先最小化的窗口。`p5-all-pages-closure-roundtrip.json` 和 `p5-all-pages-closure-media-roundtrip.json` 均通过；鉴权成品哈希、真实播放控制通过，窗口切换保持同一播放会话，源请求增量为 0。测试 UI/Core/播放器/HTTP 进程及独立目录均清理。
- 全部 38 个计划视图已分轮实窗查看，发现的五类问题均有修正后的复验证据。`all-pages/final/review.json` 记录每张截图实际使用的二进制与源码清单；`all-pages/final/index.html` 为真实截图索引。未改动页面沿用已查看的对应候选截图，不冒称 38 张都来自最后一个二进制。完整 GUI 业务往返、真实投屏设备、在线字幕云服务、更新安装、混合 DPI 与完整键盘/读屏仍未验收；正式入口保持 Compose，`release_ready=false`。
- Slint 编译器在 Windows 构建脚本主线程发生栈溢出，改由 16MiB 栈的标准线程完成 UI 编译；仅影响构建过程，不改变应用运行时线程或下载逻辑。

## 临时资源与边界

- 所有新代码、日志、比较产物与测试配置均在项目内。
- Edge 一次夹具在退出后仍有 BITS 文件被占用，遗留目录 `.tool-cache/test-tmp/hls-extension-smoke-qdo2l27h`，保留清理失败日志。后续新夹具目录清理完成。
- 更早的 `.tool-cache/test-tmp/hls-extension-smoke-arqg0ob0` 删除曾被自动审批拒绝，不重试绕过。
- 本轮 Slint 静态检查产生的 `.tool-cache/test-tmp/all-pages-slint`（`hot.rs` 23,654,813 字节）和 `native_ui/.tool-cache/test-tmp/all-pages-slint`（空目录），精确删除被自动审批以 `blocked by policy` 拒绝。两处目录保留，没有换工具或命令绕过；不能宣称临时资源已全部清空。
- 未重置、暂存、提交、推送或发布；现有用户修改与旧候选包均保留。

## HTTP worker 按需初始化与参考更正（2026-11-09）

- `native_shell/src/http_engine.rs` 的范围 worker 改为首次领取区间后才打开 payload、分配 1 MiB 写缓冲；已初始化的 worker 在后续区间间复用缓冲。尚未领取任务的 worker 先检查停止、暂停/取消及调度状态，避免提前分配和将退出错误报告为文件打开失败。领取后打开失败会设置全局停止并释放活动区间。
- 新增 3 项定向回归。修改前执行 `cargo test --offline --locked -p hls-native-shell --lib range_worker -- --test-threads=1`，8 项中既有 5 项通过、新增 3 项失败；`before.log` 保留实际错误。修改后执行整个 `http_engine::tests`，80 通过、0 失败、8 忽略、571 过滤，测试用例运行 32.75 秒。忽略项是 4 项 curl-impersonate 与 4 项 WinHTTP Sandbox 专项，未将其计为通过。
- `rustfmt --check --edition 2021 native_shell/src/http_engine.rs` 及 `git diff --check` 通过。日志与源码/测试二进制身份保存于 `artifacts/idm-worker-lifecycle-20261109/`。没有修改浏览器凭据、责任交接、实验尾段切分开关或 UI；本次未运行 GUI、完整包构建、安装升级或 IDM/HLS 性能对照。
- 更新 [IDM 增量核查](idm-source-audit.md#新增调度解析器与链接测试复核2026-11-09) 与 [历史结论更正](idm-native-handoff.md#新增参考更正2026-11-09)：承认最新恢复代码已修正分段函数地址，并明确新链接/TS 测试的覆盖范围和剩余模拟实现。参考目录保持只读，未执行其程序、构建或验证脚本。
- 正式入口与发布状态保持现状：`release_ready=false`，本次源码和测试构建不等于已经更新正式安装包。

## 原生文件交互与实际回归（2026-11-09）

- 将 OLE 与文件剪贴板实现移到 `crates/client/src/file_transfer.rs`，Core 与 UI 共用。修正无窗口后台线程以 NULL owner 清空剪贴板的问题；写入前检查完整路径和文件存在性，Unicode CF_HDROP 拒绝内嵌 NUL。拖出区分取消与成功，界面在鼠标按住并移动后于 UI 线程启动 OLE。
- 原生多选复制一次提交完整选择，避免逐项覆盖。新增 Ctrl+C/Ctrl+V、Explorer 文件粘贴、多行批量/cURL 文本入口；同次文件拖入合并为一个 ImportPaths，保留 Core 已有的部分成功汇总。自动识别交给 Core 逐 URL 执行，避免整段批量文本猜出的类型污染其余任务。
- 实窗发现已完成任务的主动作错误优先显示“重试”，已修正为播放/打开优先；导航 VecModel 改为保留模型并仅更新变化行，避免 250ms 空闲轮询反复重建控件。这两项及最终拖动阈值修改晚于已验收的 UI 二进制，尚须重新构建与实窗复验，不能用前一轮结果替代。
- `artifacts/native-completion-20261109/` 保留本轮证据。扩展类型检查与全部回归 54 文件 / 592 项通过；原生 UI 初次 20 项回归通过（早于末次列表修正）；共享文件交互定向 3 项通过。随后 workspace 库测试 706 通过、13 忽略、0 失败，其中 Core 647、client 48、protocol 11；新增混合 URL 识别与暂停创建回归通过。忽略项包含 UAC、杀毒超时、curl/WinHTTP Sandbox、FFmpeg 和音轨夹具，未计为通过。
- 初次全部 workspace 测试尝试因真实 GUI 夹具占用 Engine.exe 而构建失败；改为仅运行库测试后通过，没有将文件占用错误当作功能失败或隐藏。Core/UI dev 构建、依赖边界及版本合同检查通过；这次 UI 构建同样早于末次列表修正。
- 真实 Core 本地下载与后台复制成功；GUI 选中两份已完成文件后按 Ctrl+C，Windows 剪贴板重读确认为完整 2 项，见 `clipboard-ui.json`、`copy-ui.png`。导入尚未操作完成：Windows 出现既有 02:17:26 定时关机通知，系统事件记录安排时间为 01:27:26。夹具因此提前结束，`files-roundtrip.json` 的 passed=false 必须保留，不能声称整套 GUI 往返通过。独立 Core/UI/HTTP 进程及夹具目录已清理。
- 取消系统关机属于项目外操作，已向用户请求明确授权，尚未收到答复。最终 UI 检查与清理状态见本轮 `validation.json`。正式入口与 `release_ready=false` 保持现状；没有发布新安装包，也没有声明体积或吞吐收益。

## 原生文件交互闭环与任务动作复核（2026-11-09）

- 旧关机通知已过期，不再构成验证阻碍。新增 ZIP 文件选择筛选、工具栏全部/选择范围回归；无选择的全部开始包含筛选隐藏任务，跳过 completed/done，显式重新下载仍保留。UI 22 项回归、dev 构建和自检通过，候选 UI SHA-256 为 `da8165969dd56a883f5e3573d1fdd06c2be48d2c46ffbad29d43aa443e211720`。
- 该候选真实 GUI 复制两份成品后 Windows CF_HDROP 重读一致；Explorer 三文件粘贴导入两份有效列表，中间无效列表未产生任务，GUI 提示「已导入 2 项任务，1 项导入失败」。真实另存为导出全部 5 项，schema 为 `hls-downloader.tasks.v1`，未泄漏合成 Cookie。已完成筛选页的全部开始恢复隐藏暂停任务，原两份成品的精确修改时间与 SHA-256 均未改变。
- `artifacts/native-ui-closure-20261109/files-roundtrip.json` 的 passed=true，最终 5 份成品哈希一致，夹具 UI/Core/HTTP 与独立目录清理通过；本轮两个 Explorer 测试窗口已关闭。导入结果实际自动完成，未将其描述为「导入后暂停通过」。
- 实窗发现普通 BIN/ZIP 因 playback_ready 被错误提供播放/投屏、更多菜单收缩多选，以及勾选显示与选择计数不一致。已限制就绪播放到媒体类型/扩展名，保留 Core 显式能力，打开动作改用文件图标；已选行更多菜单保留多选，复选框双向绑定选择模型。新候选源码清单 `media-selection-source.json` 覆盖 138 个输入，摘要为 `a0348918fb61d4088c5b29ed78c29fbf721a161748eb3b85500893148e362ac5`。
- 新候选 UI 24 项回归通过（0 失败、0 忽略），测试编译 8m02s；dev 构建 7m56s 与自检通过，UI SHA-256 为 `c5302e9b4b60c32060ada2d0f039a6be971e95d7138e96be68988440914321a0`。最终源码与清单一致，原生客户端依赖边界检查及 `git diff --check` 通过。Core 和扩展本轮未改，未重复先前已通过的库/扩展全量回归。
- 新程序实窗确认普通 BIN/ZIP 的主动作和图标均为打开文件，播放/投屏/TVBox 禁用；勾选两项后打开更多菜单仍是两项，菜单复制后 Windows CF_HDROP 重读确认为同两项。单击一行后其他勾选清除，取消选择后全部清除，原成品哈希和精确修改时间不变。记录为 `media-selection-gui-review.json`，对应 `media-selection-*.png/.txt`。
- 本地 ZIP 经 UI Automation 标准控件输入路径并点击读取，实际显示 `docs/readme.txt`、`media/notes.txt`；远程 ZIP 经真实 Core API 读取通过。系统文件选择器已打开但路径输入未送达，远程 GUI 的 owned 控件查找持续失败，因此这两项未验收，没有将 Core API 结果冒充 GUI 通过。
- `media-selection-roundtrip.json` 的 passed=true，两份成品哈希一致，UI/Core/HTTP 与独立目录清理通过。测试编译专用 TEMP `.tool-cache/test-tmp/native-ui-closure-20261109` 已检查为空、不是重解析点且位于工作区内；删除被自动审批以 `blocked by policy` 拒绝，空目录保留，未重试绕过。构建缓存和验收证据保留。
- OLE 仅观察到一次自投递，不作为外部复制成功；跨窗口目标超出 Computer Use 的窗口坐标范围，外部拖出及 Esc 取消未验收。正式入口仍为 Compose，`release_ready=false`，未发布新包。

## 参考增量核查与组件同步（2026-11-09）

- 证据集中在 `artifacts/component-sync-20261109/`。参考目录只读核查得到 123 个恢复 C 模块、325 个源码/配置输入和 10,165 份 r2 反汇编输入；23,988 份 Ghidra 文件与已有清单一致。文件身份核查不等于人工理解全部函数，也不证明恢复实现全部正确。
- 最新恢复模块已经修正动态切块函数地址、音频累计字段；自研扩展已修正消息头、ACK 和等待裁决后的下载 ID 闭包。直接执行当前参考 codec 的五项检查中三项通过，两项带逗号的字符串仍被截断，见 `reference-codec.json`。未把错误实现移入 HLS，未执行参考程序或其构建脚本。
- 同步前源码摘要为 `28f7cdd8c530da1d51bbc46ba2bcc4516ae1bc997cda737e450c95173aa81981`。原生 UI 24 项回归、扩展类型检查与 54 文件 / 592 项回归通过；workspace 五组件 Release 构建通过，耗时 19m22s。真实新 Core/CLI 十项矩阵和四个并发 Host 联测通过，常驻 Core 为一个，最慢首响应 993.39ms。这些组件证据不替代安装包或完整 GUI 验收。
- Chrome/Firefox 7.0.3 扩展包已生成并检查 ZIP 完整性、版本及 MV3 后台配置。Firefox、Edge 的媒体复验均在 WebDriver 建立会话时读取超时，媒体断言执行数为零；保留失败报告，没有重复盲试或计为通过。
- 该批 Presenter 实窗确认标题栏浅→深→浅、外观列选项与重置显示；资源 owned 窗口输入因自动化缓存/焦点错误未完成。用户随后要求取消细碎显示调节，这部分历史 GUI 结果不作为自动布局的新验收。新 Presenter 的构建和验证单独登记在 `artifacts/responsive-ui-20261109/`。
- 该批新夹具 UI/Core/HTTP、浏览器与独立数据目录均已清理。历史删除被自动审批以 `blocked by policy` 拒绝的目录继续保留，未换工具绕过。没有提交、推送或发布，`release_ready=false`。

## 自动布局与外观设置收敛（2026-11-09）

- 按用户“不需要这么多能调整显示的地方，能自动适应就可”的要求，删除原生 UI 的逐列宽度、列显示/顺序、工具栏排列编辑器、列宽拖动及重置入口，并移除相关模型、回调和持久写入。外观保留主题、减少动态效果和任务排序。旧 Core 布局键仍有 Compose 消费者，因此保留合同；新原生 UI 不应用、不重写这些键。
- 名称占用剩余宽度，名称/进度/状态/操作始终保留；可用宽度不足时收起速度与大小列，放宽后恢复。工具栏固定主要动作并随宽度切换图标，搜索框和侧栏同步收缩，关闭表格及工具栏横向滚动。保留原图案、共享图标、主题和业务功能入口。
- 证据位于 `artifacts/responsive-ui-20261109/`。独立编译执行布局模块的两项回归通过，覆盖 453–1920 每个整数可用宽度的总宽、主要列保留及伸缩恢复。Release 构建通过，耗时 19m16s，自检返回 `hls-v7-presenter/1 ok`；未把修改前的 24 项 UI 回归冒充修改后全量通过。
- 当前 275 输入源码摘要为 `ce72d9c46c0a93db229c8445c07955564d7a505517a07ddc72c5db08793eddc6`，清单重读一致。新 Presenter 为 54,529,024 字节，SHA-256 为 `bb78066301a48243115473456824b5bcbc24fed3e588f8d954519a0727b8dd16`。其他四个组件和双浏览器扩展包复用上节未变化输入的已测成品，并重新核对文件哈希，见 `binaries.json`；没有重复构建无变化的 Core/扩展。
- 真实 Core 与新 UI 隔离运行正常，浅→深→浅的标题栏 API 三轮均成功。Computer Use 在首次列出新窗口时报告用户物理 Esc 中止，立即停止桌面操作；没有截图或宽窄窗口、外观页、旧布局键保存后的实窗结果。`tools-roundtrip.json` 的 passed=true 仅代表运行与 API 检查，不代表上述 GUI 验收通过，具体限制见 `gui-review.json`。
- 已通过结束标记关闭本轮 UI/Core/HTTP，独立数据目录清理完成。构建缓存、源码清单和实际日志保留；历史审批拒绝删除的目录继续保留。正式入口仍为 Compose，`release_ready=false`，未修改正式安装包，未提交、推送或发布。

## IDM 读取协调对照与原生窄窗修正（2026-11-09）

- 按用户要求继续现有原生 UI 和自动布局，没有接入 `ui_next`，没有替换图案、图标或主题。窗口恢复下限从旧的 1024×600 对齐当前 560×360，仍按 DPI 和工作区上限夹取；任务区长队列标题在剩余空间省略，Tooltip 显示全文，操作按钮保留布局空间。
- 只读对照 IDM 的 HTTP worker、状态分派、区间协调恢复文件和原 `0x00590000` 反编译函数。恢复状态名与原分支存在 SSPI/TLS 对应差异，不能把简化恢复 C 视为完整下载状态机；详见 [参考审计](idm-source-audit.md#http-读取阶段与协调停止复核2026-11-09)。本轮没有执行参考程序、脚本或构建。
- HLS 原范围请求只在开始前检查共享停止，正在消费 206 正文或等待限速时仍可能继续工作。新增真实本机 HTTP 回归在响应正文可用前设置共享停止，修改前仍消费/写入全部 65,536 字节，`http-stop-before.log` 实际失败。修复在正文循环、读取返回后、限速等待和重试等待检查停止；中止前刷新并登记已写范围，首次错误不被后续取消覆盖，暂停文件优先于通用停止原因。保留控制文件 50 ms 检查间隔，没有增加每次小块读取的文件打开。
- `ui-regression.log`：25 项 UI 逻辑回归通过。`http-regression.log` 首轮为 80 通过、1 失败、8 忽略，失败项为 `hls_byte_range_rejects_oversize_body_before_eof` 的 3 秒等待超时。没有修改该测试或超时；`hls-range-recheck.log` 单项复测通过，同一编译产物的 `http-regression-recheck.log` 整组复测为 81 通过、0 失败、8 忽略，耗时 29.23 秒。首次超时原因未确定，原失败证据保留。8 项仍是原有要求 Sandbox/curl-impersonate 的忽略项，未计为通过。
- `artifacts/idm-native-refinement-20261109/source.json` 记录 275 个输入，源码摘要为 `fd654f7828836fa63cf1b3fcaab35714cd4b4cfb1cf947c341636759797f2ab3`。相对上一批只改变 `native_shell/src/http_engine.rs`、`native_ui/src/native_window.rs`、`native_ui/ui/workbench.slint` 三个运行代码文件；扩展、协议及客户端输入未变。Core 与 UI 的本轮 Release 构建和自检结果单独登记于该批次证据，不沿用旧二进制冒充最新结果。
- `release-build.log`：Core 与 UI Release 构建通过，耗时 22m11s；`core-self-test.log`、`ui-self-test.log` 的成品自检均通过。Core 为 14,976,512 字节，SHA-256 为 `c9d7edd482de59e724903e1e1115924992f619f37bdc023c4b712c33ac60b076`；UI 为 54,555,648 字节，SHA-256 为 `c6ef35f440f9b1428bba378221b098ca0c2f175f15816db445e5f5bfe7b80a6b`。CLI、Host、Updater 和双浏览器扩展包因输入未变复用已有测试成品，并重新核对哈希，完整清单见 `binaries.json`。`source-check.json` 再次核对 275 个项目输入和 4 个参考输入全部一致。
- 当前停止是协作式检查，不主动中断所有底层传输：已阻塞的网络读取仍需现有超时或传输返回。没有同条件 IDM 性能对照，不宣称吞吐、内存、包体积或稳定性已经达到或超过 IDM。
- 用户明确禁止桌面操作后，本轮没有调用桌面或浏览器自动化，没有新实窗视觉、窗口保存重开或浏览器端到端结果。此前尝试的窗口尺寸直接绑定存在 Slint 布局环/约束冲突，已撤回，失败日志 `ui-tests.log`、`ui-tests-corrected.log` 保留；当前继续使用既有 Winit resize 同步，未增加测试后端或布局配置。
- 正式入口仍为 Compose，`release_ready=false`，正式安装包未切换。保留现有未提交迁移工作和历史审批拒绝删除的目录；本轮未暂存、提交、推送或发布。

## 下载协调停止、播放资源回收与字幕错误回执（2026-11-09）

- 证据目录为 `artifacts/runtime-quality-20261109/`。本轮未操作桌面或浏览器，继续现有 Rust/Slint 界面、图案、图标和自动布局。没有改动扩展、协议、客户端或安装入口。
- 上轮的范围正文循环能看到共享停止，但底层 TCP/WinHTTP/curl 只观察用户控制文件，响应头、正文或 chunked 行等待可能继续阻塞。本轮为 `Job` 加入仅内存使用、Serde 跳过的共享停止信号，由范围 worker 传递到传输层；TCP 短超时轮询、WinHTTP 异步等待和 curl 子进程等待均检查它，仍先保留用户暂停原因。服务器忽略 Range 时，转交给单流的原 200 响应解除范围停止绑定，继续检查用户控制，避免重复请求或误取消唯一响应。DNS/建立连接等阶段仍受底层连接超时约束，不宣称所有阶段立即中止。
- 新增真实本机 HTTP 回归覆盖 TCP 和 WinHTTP 的响应头、正文及 chunked 等待，在服务端保持连接且不发送结束数据时触发共享停止，同时检查暂停优先。`http-regression-corrected.log` 为 82 通过、8 忽略，14.40 秒；原 200 响应复用、检查点及凭据回归均通过。首次编译的七处借用错误已修正，失败日志 `http-regression.log` 保留；未修改回归门槛。curl 分支已接信号，本轮未运行要求 Sandbox 和专用组件的 curl 用例。
- 本地播放挂载现在属于播放会话：成功换片、停止、任务结束播放及切换下载成品时撤销旧挂载；失败的换片保留当前地址，过期代数不能撤销新会话。扩展现有播放器回归，以真实 HTTP 请求检查可用地址为 206、撤销后为 404，同时保持成品可播放。`worker-regression.log` 的下载协调模块 101 项回归通过；播放器使用 null 后端，这不是最新成品的实窗或 libmpv 验收。
- 字幕设置先检查 Core 的 `ok/error`，拒绝回执不再被当作成功配置应用到界面。UI 26 项回归通过，包含错误信息保留；字幕服务 19 项通过、1 项忽略，含本机 WebSocket、PCM 提交、seek/reset、分页与凭据隔离。没有调用真实付费云服务。原生播放器在线翻译的音频提交和 HUD 链路仍未完成，不能将设置回执修复称为该功能已交付。
- `source-initial-candidate.json` 记录与上一批相同范围的 275 个输入，首个候选变更七个文件，摘要为 `712b4e35b2af816e6d47d2a88d2f0b9eb15b76baacf929cbb1bacaa1952f52cc`。其中 DASH、BT 和更新模块仅补上 `Job` 的内存字段默认值。
- 修改前成品的本机 256 MiB 下载基线为 28.08 MiB/s、工作集增长 7.16 MiB、SHA-256 一致、发布后额外读取 0；60 秒空闲 CPU 为 0.026%，18,000 次空任务快照请求 p95 为 0.169 ms，压力阶段句柄增长 0、错误 0。它们只代表旧成品基线，不能作为新成品指标，也不等于长期稳定性或与 IDM 的同条件对照。最新构建及成品验证另行登记。
- 首个候选 Release 构建通过，耗时 12m24s；Core、UI 自检均实际通过（UI 自检只构造隐藏组件，没有显示或操作窗口）。Core 为 14,981,120 字节，SHA-256 为 `52dac47784b0b9f4a8b119f0ecb162f5187b180f85ca162dcbf3d2b8bbd9f82b`；UI 为 54,556,160 字节，SHA-256 为 `34886bb3aec9aa320b8cdd2c8b9f4595d874ea2bba927e5b1cc75b452bda07a8`。其余三个程序和两个扩展输入未变、哈希复核一致，复用既有成品，清单见 `binaries-initial-candidate.json`。275 个源码输入在构建后重读一致，见 `source-check-initial-candidate.json`。
- 新 Core 与原 CLI 的 `free-functions.json` 十组集成检查全部通过：无工作台拉起单 Core、下载生命周期和重启恢复、导入导出、队列动作与显式删除、stdin 请求上下文和默认脱敏、本地/远程 ZIP 有界读取与资源变更、抓站循环和深度边界、跨域重定向前拒绝、ETag 同步、失败保留旧文件及定时同步。夹具清理完成。这是本机开发组件验收，不替代 GUI 往返和安装验收。
- 新 Core 与原 Host 的四客户端并发启动通过：四组各两条 Native Messaging ping 均成功，只启动一个驻留 Core，最慢首次回执 945.50 ms（门槛 1500 ms）。该项运行时 UI 仍在编译，未将它解释为与 IDM 的公平启动对照；它也不是实际浏览器的下载交接、凭据或丢回执矩阵。

## 慢盘调度、回归隔离与性能复核（2026-11-10）

- 首个候选 `52dac477…f82b` 的 256 MiB 下载先测得 22.70 MiB/s，随后有界复测为 **17.76 MiB/s，低于 20 MiB/s 门槛**。失败报告 `transfer-current-recheck.json` 和检查点日志保留，不能选取前一次通过结果声称性能全部通过。该次成品哈希正确、峰值工作集 20.30 MiB，发布后额外读取 0；同盘仅顺序写入并同步的基线为 43.04 MiB/s，与包含网络、状态持久化的下载吞吐不是同一指标。
- 检查点日志显示多次 0.6–1.1 秒等待。曾尝试把慢盘退让下限设为双连接，`601f9f35…1ebc` 的两次 Release 实测为 **11.93、17.82 MiB/s，均未达标**，见 `transfer-storage-final-1.json`、`transfer-storage-final-2.json`；该尝试已撤回，没有作为有效优化保留。同盘当前顺序持久写入基线为 52.00 MiB/s，见 `storage-current.json`。
- 在现有 `HLS_HTTP_IO_PROFILE` 日志中拆分缓冲 flush、检查点锁、payload 同步和元数据提交耗时。`transfer-io-diagnostic.engine.log` 的 dev 成品诊断显示，缓冲 flush 多数不足 1 ms，主要时间在等锁、同步及元数据提交；该 dev 成品的 16.98 MiB/s 不作为 Release 指标。原退让把这些串行事务耗时当成流写入背压。本轮改为测量 `write_all` 的实际阻塞及检查点前的缓冲 flush；持久化函数返回 flush 耗时供调度使用，仍等待完整同步和检查点提交后才成功返回。保留原 8 MiB 间隔、同步、原子保存、网络错误退让及用户连接上限。
- `http-storage-regression.log`：HTTP 83 项通过、8 项忽略，13.24 秒，包含慢盘/网络退让边界、原 200 响应复用和传输停止；忽略项没有算作通过。
- 最终写入背压修改的 `http-write-pressure-regression.log` 为 83 通过、8 忽略，14.72 秒，包含慢写入退让、快速写入不误退让及单连接上限的调度边界回归。
- 整组首次暴露播放器测试的任务身份冲突，独立 Core 夹具复用 `task-1`，使其他完成线程替换全局 null 播放源。改为独立任务身份后，又观察到“刷新地址并恢复”在并行整组中超时；定向运行 0.25 秒通过，移除播放器夹具的诊断组 100 项通过。新夹具使用磁盘数据库，在全局播放器锁内保存状态会拖住并行收尾；现在复用现有 `from_store` 构造路径恢复独立身份，使用内存数据库，不再增加与本测试目标无关的磁盘持久化负载。构造器仅扩大为 crate 内可见，没有新增生产运行分支。
- `worker-storage-isolated.log`：最终下载协调整组 101 项通过，11.80 秒；保留刷新地址原有 5 秒门槛。此前 `worker-storage-regression.log`、`worker-storage-corrected.log`、`worker-storage-recheck.log` 失败记录均保留，没有用跳过用例的诊断组代替最终整组验收。
- 首个候选的 `runtime-soak-current.json`：实际 300 秒空闲 CPU 0.094%，工作集增长 0.083 MiB、句柄减少 3；18,000 次空任务快照 IPC 的 p95 为 0.160 ms、最大 1.989 ms，压力阶段内存/句柄增长 0、错误 0。该结果属于 `52dac477…f82b`，不是新调度成品的数据，也不是 18,000 次下载、长期运行或千任务验证。
- 修正写入背压后的 dev 成品 `transfer-write-pressure-dev.json` 测得 34.11 MiB/s、峰值工作集 30.84 MiB、SHA-256 正确、发布后额外读取 0。它用于确认修改方向，不作为 Release 验收。最新源码的下载协调整组 `worker-write-pressure-regression.log` 为 101 通过，4.69 秒，刷新地址仍保留原 5 秒门槛。
- 最终 275 输入源码清单为 `source.json`，摘要 `1fb09d412dc5f8ab95aab02ac2e3f42007cd69e36b6b514b3d85183942d80f06`，相对上批改变八个文件（新增一处现有构造器可见性调整）。双连接下限候选的输入与成品身份保留为 `source-storage-candidate.json`、`binaries-storage-candidate.json`，没有覆盖失败成品的身份记录。
- `release-write-pressure-build.log`：最终 Core Release 构建通过，5m08s；`core-final-self-test.log` 自检通过。Core 为 14,979,584 字节，SHA-256 为 `3e5f563b7401576ca89031a37c071246e0a79b14051f507104776b287f8bd9ae`。Presenter、CLI、Host、Updater 和双扩展的各自输入未变、成品哈希已重核，复用首个候选对应组件，见 `binaries.json`；没有重复编译 UI。
- 最终 Release 的两个预定 256 MiB 本机下载分别为 **32.58、41.13 MiB/s，均通过原 20 MiB/s 门槛**，见 `transfer-write-pressure-release-1.json`、`transfer-write-pressure-release-2.json`。两次 SHA-256 均正确，发布后额外读取均为 0；Core 峰值工作集为 27.09、27.21 MiB，较启动增长 13.72、13.82 MiB。此前低并发候选的峰值约 20 MiB，故本轮是用更多有效并发改善吞吐，没有声称同时降低下载峰值内存。没有与 IDM 做同条件对照，也不能将本机结果扩大为所有网站或磁盘的速度保证。
- 最新 `3e5f563b…d9ae` 的 `free-functions-final.json` 十组本机功能验收全部通过，覆盖单 Core 启动、下载生命周期/重启恢复、队列/显式删除、导入导出、stdin 凭据及脱敏、ZIP 有界读取/资源变化、抓站边界/跨域拒绝/ETag/失败保留旧文件/定时同步，隔离运行时清理完成。`native-host-final.json` 四个 Host 各两条 ping 全部通过，驻留 Core 数为 1，最慢首次回执 968.29 ms，低于原 1500 ms 门槛。本轮这些检查没有与编译并发；它们仍不是浏览器凭据、一次性响应或丢回执的真实端到端矩阵。
- 最新 `runtime-soak-final.json` 通过：实际 60 秒空闲 CPU 0.052%，工作集变化 −0.020 MiB、私有内存变化 −0.039 MiB、句柄增加 1；18,000 次**空任务快照 IPC**在 2.141 秒内完成，p95 为 0.203 ms，最大 1.937 ms，错误 0。压力阶段工作集增长 0.031 MiB、私有内存增长 0.035 MiB、句柄增长 0，进程保持存活。按请求率折算的时间不当作实际运行时长，也不把它解释为长期或大量下载压力通过。
- 最终 Core 源码的字幕相关过滤回归 `subtitle-all-final-regression.log` 为 26 通过、1 忽略，包含真实本机 WebSocket/PCM/reset、凭据隔离及 HLS/DASH 字幕相关边界；前一次较窄的 12 项检查另存 `subtitle-final-regression.log`。UI 源码和客户端依赖未变，复用本轮首个候选的 26 项 UI 回归，不重复编译。四组回归共执行 236 次通过检查，其中一个用例因过滤范围重合执行两次，观察到 235 个不同通过用例；9 个忽略项不算通过。测试数量本身不代表全部功能、真实云质量或完整 UI 业务已验收。
- 清理失败单元测试夹具前，已核对三个确切目录的边界、无 reparse point、各自唯一文件的合成内容及无进程引用。PowerShell 原生精确递归删除仍被自动审批以 `blocked by policy` 拒绝，未执行也未换工具重试。共 51 字节保留在 `.tool-cache/test-tmp/hls-player-session-19732`、`hls-v7-refresh-23588-1794240243180203000`、`hls-v7-refresh-8772-1794240579560616900`，具体文件与哈希见 `cleanup-failed-fixtures.json`；需用户手动删除这三个目录。本轮不称清理全部完成，失败日志和必要诊断证据继续保留。
- `source-check.json` 对最终 275 个输入重读一致，五组件和双扩展哈希再次核对；`git diff --check` 通过，未暂存、提交、推送或发布。最终测试目录清点 `cleanup-final.json` 没有活动测试运行时进程，除上述三个审批阻止的目录外没有本轮夹具残留。聚合结果与边界见 `summary.json`。
- 本轮使用命令行、隔离 Core、真实本机 HTTP/IPC 和 null 播放器回归，没有桌面/浏览器自动化，也没有真实付费云请求。原生在线字幕音频提交及 HUD、最新组合的真实浏览器凭据/回执矩阵、完整原生安装/升级/回滚、GUI/DPI 与长期千任务压力仍未完成；正式 Compose/JRE 入口未切换，`release_ready=false`。本轮定向修复和有界组件验收已完成，不将其称为整个项目发布完成。


## 原生候选发布准备（2026-11-10，7.0.4）

- 用户授权原生桌面与浏览器插件同步 GitHub、打包发布；保留此前“禁止使用电脑”的约束，本轮仅使用命令行、源码检查和无 GUI 回归。`ui_next/` 是用户保留的替代 UI，不进入产品或发布；未删除。
- 五个 Rust workspace 成员、根锁文件、桌面对照版本和浏览器插件统一为 7.0.4。原生 launcher 启动共享 Slint Presenter，构建脚本默认 native；Compose 源码作为尚未完成全验收前的对照保留。原生 EXE/MSI/Portable 不打入 Java/Compose/JRE。
- 在线字幕接入主播放器与 PiP。第一次启用才创建工作线程，Core 从已有下载字节挂载随机私有 loopback URL，每次最多解码四秒 PCM；FFmpeg 不重新访问源站，也不接收源站 Cookie/Authorization。UI 按媒体时间显示译文/原文，暂停或缓冲时不解码新窗口，缓存限制为 256 个 cue，空闲和失败时阻塞等待唤醒。
- seek、换片、停止、速率或音轨变化及 Core 重连会使旧字幕会话失效；命令 ACK 前不提前解码旧位置。修正周期性状态更新导致字幕闪烁。普通提前四秒、预取提前十二秒；尚未实现暂停等待字幕，不能把本机链路验证解释为云端质量或延迟保证。
- 新增无 Java 的 WiX 3.14.1 安装器构建，固定官方归档与 SHA-256，沿用 UpgradeCode、七浏览器 Native Host 注册、升级前检查点退出和回滚顺序。已公开 Compose 7.0.3 的安装路径注册表项经旧 MSI 只读核对，原生候选支持回收该自定义路径；新 native 路径使用固定项。生命周期回归基线改为实际公开的 v7.0.3-r22，并取消候选安装时显式传入 INSTALLDIR，以暴露自动路径恢复错误。其他历史版本的自定义目录迁移未验收。
- `installer-authoring-migration.log`：固定 WiX 对小型夹具的 MSI/EXE 结构构建通过；`msi-migration-sequence.json` 核对两条序列均为 AppSearch 50 → 旧目录恢复 51 → INSTALLDIR 恢复 52 → CostInitialize 800。该证据不是实际安装、升级或失败回滚通过；Burn 的 per-user 推断警告保留。
- 根据远程 Rust Security 的实际公告，将 crypto-bigint、ctutils、rustls-webpki、tokio 分别锁定为 0.7.11、0.4.8、0.103.16、1.53.3。沿用 RSA 唯一既有忽略项，未增加公告豁免；最终完整安全审计以此次提交的 CI 为准。
- `workspace-serial-tests.log`：Rust `--workspace --all-targets -- --test-threads=1` 成功，13 个测试程序合计 756 通过、14 忽略、0 失败；UI 28 项包含字幕时间轴、偏移、部分文本与旧会话行为。编译 12m55s。默认并行编译曾内存紧张而停止，失败日志保留；最终改为单 Cargo job，没有将停止的编译计通过。
- `player-subtitle-audio.log`：独立运行已编译的 FFmpeg 专用字幕用例，实际本机 WAV → Core 私有 HTTP 挂载 → FFmpeg PCM 字节一致，播放源不变、取消、停止后旧会话拒绝通过。`ffmpeg-publish.log`：真实 FFmpeg 成功后替换成品及原地 mux 回归通过。它们是额外执行的两个原忽略用例，其余忽略项未计通过。
- `extension-tests.log`：54 个测试文件、592 项通过，TypeScript 检查通过。PowerShell 5.1/7 的 60 脚本语法、版本和门禁 ID 合同通过；`release-gap-contract.log` 的正负对照通过；Rust 格式及原生客户端依赖边界检查通过。检查日志集中在 `artifacts/release-native-20261110/`。
- 候选产物、最终源码身份、成品哈希和打包后集成结果以构建生成的 ARTIFACT-MANIFEST、BUILD-PROVENANCE 及发布附件 VALIDATION.json 为准。没有继承历史 Compose 的实窗、浏览器或安装验收为原生通过。完整 GUI/DPI/可访问性、真实双浏览器凭据/回执矩阵、真实安装生命周期、长期/千任务压力未完成，`release_ready=false`；真实投屏与云端字幕质量保持既有排除。

- 最终静态检查 `clippy-verified.log`：整个 workspace 的全部目标通过，保留项目既有结构性豁免（large_enum_variant、too_many_arguments、type_complexity、UI while_let_loop），未新增告警豁免。全量测试后仅整理等价 match 条件、测试初始化及模块位置，并让 HLS 测试服务端读取完整请求头，防止连接短读干扰断言；这些收尾不代表重新执行过整套工作区测试。新增文件暂存后检查并清理两个末尾空行，暂存差异检查通过。Python IPC 七项用例和 17 个修改过的 Python/YAML 文件语法检查通过。

- 收尾后 `client-strict-clippy.log` 对协议、共享客户端和轻客户端全部目标执行无豁免的 `-D warnings` 检查，通过；`client-final-tests.log` 为 48 通过、1 个 UAC 用例忽略；`hls-init-map-final.log` 的初始化段失败不污染缓存用例通过。没有重复整套 UI/工作区回归，生产业务变更此前已完成工作区回归。

### 2026-11-10 发布前 CI 与安全复核

- 首次同步提交 `88a6ecc5` 后，GitHub v7 CI 实际发现两处原生工作台 Clippy 问题及 Compose 分类对照测试的旧路径。工作请求变体改用 Box，缩小有界通道项；队列保存状态使用具名结构，保留收到持久化回执后才指派任务的顺序。分类对照改读 `crates/client/src/media_ext.rs`。最终检查结果以下次提交的 CI 和本地日志为准。
- Rust Security 新报 SQLite 与 Markdown 解析公告，已升级 rusqlite 0.41.1 / libsqlite3-sys 0.39.1 / pulldown-cmark 0.13.6；完整工作区回归在 `workspace-security-final.log`，尚在运行时不计通过。
- 扩展构建链升级 vitest 5.0.3、fast-uri 3.1.9、node-forge 1.4.1、vite 8.1.6、undici 7.30.1。node-forge 原本地补丁由上游修复替代；deepmerge 4.3.1 对应 GHSA-gcqx-w48m-w2h4 尚无已发布修复，锁定补丁覆盖源码、CJS 与 UMD 分发，将结果属性写入改成数据属性，避免调用原型 setter。未放宽其他依赖公告门禁。
- `extension-security-final.log`：实际 web-ext 依赖入口的两种合法签名通过、四种畸形签名拒绝、四组原型污染及正常合并回归通过，其他未处理公告为零。`extension-security-tests.log`：升级后 54 文件、592 项测试及 TypeScript 检查通过。
