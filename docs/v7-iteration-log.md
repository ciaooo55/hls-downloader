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
