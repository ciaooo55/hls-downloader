# Historical v7.0.1 formal-release path snapshot

> **Historical evidence only.** This file records the v7.0.1 release-path state observed on 2026-09-04. It is not current release guidance for `main`. The active source version is now `7.0.2`, and canonical `artifacts/v7-productization/feature-parity.json` currently declares `release_ready=false`. For the current formal-release contract use `docs/architecture/formal-release-readiness.md` and `docs/v7-release-runner.md`.

历史核验基准：2026-09-04；当时 candidate、四份 evidence、formal package 和标签要求绑定同一 commit/tree。

## Historical snapshot state

The statements in this section describe that v7.0.1 snapshot, not current `main`:

- 当时功能矩阵记录为 `28/28 verified`、`0 partial`、`0 blocked`、`release_ready=true`。
- 未来计划队列、动态全局限速、真实 BT 文件选择、认证 HLS VOD/Live 专项均在当时冻结前各运行一次并通过。
- `v7.0.0` 的标签、Release 和 10 项资产保持不变。

## Historical release sequence

The following sequence is preserved as historical evidence. Do **not** use its literal `v7.0.1` tag/version as authorization for the current v7.0.2 line.

1. 在干净冻结提交上构建一次 candidate：

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
   ```

2. 用 `record-v7-release-gate.ps1` 依次记录 browser、performance、installer、rollback；所有输入只来自 candidate manifest。
3. 快进合入并推送 `main`，等待该提交的完整 GitHub CI 全绿。
4. 运行 `build-v7.ps1 -Task package` 生成 formal package，安装到 `E:\h` 并做安装后下载/恢复验证。
5. 当时桌面只保留 `7.0.1` Chromium 和 Firefox 包各一份。
6. 当时计划创建 annotated `v7.0.1` 标签和 Draft Release，核对资产后发布 Latest，并重新下载核验 SHA-256。

## Historical failure handling

任一门禁失败后修复并重新冻结；失败提交生成的 candidate、evidence 和 formal package 全部作废。MSI 生命周期只验证应用进程重启，不执行 Windows 系统重启，报告明确记录 `system_reboot=false`。

所有构建、测试、临时文件和门禁报告留在仓库目录；正式安装只位于 `E:\h`。

## Current boundary

Current v7.0.2 release preparation must resolve the version from canonical feature-parity metadata, keep `release_ready=false` fail-closed until a reviewed readiness decision changes it, require all four exact-SHA `main` push workflows, and use the dedicated `hls-release` signing/release runner. This historical file cannot satisfy or waive any of those gates.
