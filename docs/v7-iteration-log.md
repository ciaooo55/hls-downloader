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
| 2 | native_shell 可靠性:5.x 导入有界+续传、旧库只读、设置解码日志、单实例类型化判定 | 进行中 |
| 3 | desktop_ui 可靠性:重连退避上限、taskLogs 回收、批量操作限流、投屏 toast 去噪、Locale.ROOT | 进行中 |
| 4 | desktop_ui UX:深色横幅配色、进度圈动画、交接对话框输入保护、连接状态结构化、事件重同步 | 进行中 |
| 5 | presenter_ui:HUD 确定性主任务、记住目录读-改-写、冷启动预算、删除陈旧 bin 副本 | 进行中 |
| 6 | extension:后台收尾改 alarm 驱动、alarm 周期兼容、权限收敛、版本单源、popup 渲染口径 | 待办 |
| 7 | scripts 可移植化:去除 D:/E: 缓存硬编码,默认仓库内;安装脚本对准 E:\h | 待办 |
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
