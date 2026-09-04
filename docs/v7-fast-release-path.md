# v7.0.1 正式发布路径

核验基准：2026-09-04；candidate、四份 evidence、formal package 和标签必须绑定同一 commit/tree。

## 当前状态

- 功能矩阵：`28/28 verified`、`0 partial`、`0 blocked`、`release_ready=true`。
- 未来计划队列、动态全局限速、真实 BT 文件选择、认证 HLS VOD/Live 专项均在冻结前各运行一次并通过。
- `v7.0.0` 的标签、Release 和 10 项资产保持不变。

## 发布顺序

1. 在干净冻结提交上构建一次 candidate：

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
   ```

2. 用 `record-v7-release-gate.ps1` 依次记录 browser、performance、installer、rollback；所有输入只来自 candidate manifest。
3. 快进合入并推送 `main`，等待该提交的唯一一次完整 GitHub CI 全绿。
4. 运行 `build-v7.ps1 -Task package` 生成 formal package，安装到 `E:\h` 并做安装后下载/恢复验证。
5. 桌面只保留 `7.0.1` Chromium 和 Firefox 包各一份。
6. 创建 annotated `v7.0.1` 标签和 Draft Release，核对 10 项资产后发布 Latest，并重新下载核验 SHA-256。

## 失败处理

任一门禁失败后修复并重新冻结；失败提交生成的 candidate、evidence 和 formal package 全部作废。MSI 生命周期只验证应用进程重启，不执行 Windows 系统重启，报告明确记录 `system_reboot=false`。

所有构建、测试、临时文件和门禁报告留在仓库目录；正式安装只位于 `E:\h`。
