# HLS Downloader 7 branch state

核验时间：2026-09-08

## 当前主线

- 唯一活动主线：`main`
- 产品版本：`7.0.2`
- 规范状态：`artifacts/v7-productization/feature-parity.json` 当前声明 `release_ready=false`
- 活动架构：Compose Desktop + resident Rust Core + native Presenter + WXT MV3
- 本机安装根：`E:\h`
- 历史发布基线：`v3.0.39`、`v5.0.13`、`v6.0.1`、`v7.0.0`

v7 活动树不再包含 Python/FastAPI、React/Tauri 或 Slint 主工作台。旧实现保留在同一个 Git 历史和标签中，不复制成多套源码目录。

## 发布边界

`main` 用于源码审查、构建和本机升级；合并完成后不保留并行的 v7 产品分支。`v7.0.0` 保持不变。当前源码/候选版本由 canonical feature-parity 元数据解析为 `7.0.2`；正式标签和 GitHub Release 必须绑定同一冻结 `main` SHA、candidate manifest、四份门禁 evidence、四条 exact-SHA 主线 workflow 结论以及受控签名/发布流程。

上一轮公开测试包 `v7.0.1-candidate.1` 是历史候选，不代表当前 `main`，也不授权 `v7.0.2` 正式发布。当前 `release_ready=false` 时仍可生成 candidate 用于验证，但 formal package / public release 继续被门禁阻止。

## 清理边界

Git 只同步源码、配置模板、工作流和文档。以下内容仅保留在本机且不提交：

- `artifacts/` 测试报告、截图和包（canonical `feature-parity.json` 除外）。
- Cargo、Gradle、Kotlin、WXT 和 Node 可重建缓存。
- 数据库、配置、下载文件、日志和本机 IPC 凭据。
- EXE、MSI、Portable ZIP 与本机回滚镜像。

清理脚本不得删除源码、用户数据、MSVC/SDK、JDK 或唯一回滚包。
