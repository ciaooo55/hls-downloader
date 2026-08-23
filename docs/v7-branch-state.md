# HLS Downloader 7 branch state

核验时间：2026-08-24

## 当前主线

- 唯一活动主线：`main`
- 产品版本：`7.0.0`
- 活动架构：Compose Desktop + resident Rust Core + native Presenter + WXT MV3
- 本机安装：`%LOCALAPPDATA%\Programs\HLSDownloader`
- 历史发布基线：`v3.0.39`、`v5.0.13`、`v6.0.1`

v7 活动树不再包含 Python/FastAPI、React/Tauri 或 Slint 主工作台。旧实现保留在同一个 Git 历史和标签中，不复制成多套源码目录。

## 发布边界

`main` 用于源码审查、构建和本机升级；合并完成后不保留并行的 v7 产品分支。正式 `v7.0.0` Git 标签和 GitHub Release 仍要求外部 UI Automation 与干净 Windows 虚拟机 MSI 生命周期门禁通过；具体结果见 `docs/v7-verification.md`。

## 清理边界

Git 只同步源码、配置模板、工作流和文档。以下内容仅保留在本机且不提交：

- `artifacts/` 测试报告、截图和包。
- Cargo、Gradle、Kotlin、WXT 和 Node 可重建缓存。
- 数据库、配置、下载文件、日志和本机 IPC 凭据。
- EXE、MSI、Portable ZIP 与本机回滚镜像。

清理脚本不得删除源码、用户数据、MSVC/SDK、JDK 或唯一回滚包。
