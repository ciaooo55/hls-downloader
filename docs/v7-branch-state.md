# HLS Downloader 7 branch state

当前维护约定：2026-09-21。历史性能/验收数字应从对应日期的验证文档读取，不应把旧版本号推断为当前源码版本。

## 当前主线

- 唯一本地开发、整合和发布主线：`main`；唯一远程分支：`origin/main`
- canonical 产品版本：`7.0.2`
- canonical 发布状态：`release_ready=false`
- 活动架构：Compose Desktop + resident Rust Core + native Presenter + WXT MV3
- 本机安装根：`E:\h`
- 已发布稳定升级基线：`v7.0.0`
- 最新公开测试包：`v7.0.2-candidate.1`（远程标签提交 `74f8473`；早于当前 `main`，不代表当前 7.0.2 source state）
- 更早历史标签：`v3.0.39`、`v5.0.13`、`v6.0.1`

v7 活动树不再包含 Python/FastAPI、React/Tauri 或 Slint 主工作台。旧实现保留在同一个 Git 历史和标签中，不复制成多套源码目录。

## 发布边界

`main` 是唯一开发和发布主线，用于源码审查、验证、构建和本机升级；远程推送仍须操作员明确授权，不新增长期分支或额外工作树。Dependabot 版本更新 PR 暂停，依赖更新在 `main` 审查整合，安全审计工作流保留。正式发布版本不是由本文硬编码：`.github/workflows/release-v7.yml` 从 `artifacts/v7-productization/feature-parity.json` 解析 `product_version`，并把 formal tag 设为 `v<product_version>`。

当前 canonical 版本是 `7.0.2` 且 `release_ready=false`，因此没有正式 v7.0.2 发布授权。任何未来 ready 变更都必须先作为可审查源码状态落入 `main`，然后等待该新 SHA 自己的 v7 CI、v7 Candidate Package、Maintenance Security、Rust Security 四个精确 SHA 工作流（三个 push，候选包为 main 上手动 workflow_dispatch）成功，再进入受保护的 formal release workflow。

三个安全审计工作流（`ci.yml`、`rust-security.yml`、`maintenance-security.yml`）除 `push: main` 外仍保留 `pull_request` 触发器：它只在该仓库真的收到外部/机器人 PR 时按路径过滤运行，作为供应链与依赖审查的纵深防线（见 `docs/architecture/dependency-maintenance.md` 的历史 PR 流程），不属于日常开发路径，也不改变“不在本仓库新建 PR”的策略。若某天确认不再接受任何 PR，可再一并移除这三个触发器。
`v7.0.0` 的标签/Release/资产保持不可变；`v7.0.2-candidate.1` 及更早的 `v7.0.1-candidate.1` 保持历史候选原样。旧 candidate 的 manifest、provenance、哈希或浏览器包不能作为当前 7.0.2 SHA 的发布证据。

## 清理边界

Git 只同步源码、配置模板、工作流、版本化 trust contract 和文档。以下内容仅保留在本机/runner 且不提交：

- `artifacts/` 中被 `.gitignore` 排除的测试报告、截图、candidate/formal 包和 release evidence。
- Cargo、Gradle、Kotlin、WXT 和 Node 可重建缓存。
- 数据库、配置、下载文件、日志和本机 IPC 凭据。
- EXE、MSI、Portable ZIP 与本机事务回滚镜像。

清理脚本不得删除源码、用户数据、MSVC/SDK、受控 JDK/tool cache 中仍被当前构建需要的输入，或唯一可验证回滚包。
