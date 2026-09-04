# v7.0.0 快速落地发布路径

核验基准：2026-09-04，`main` 当前提交 `b03c34b3c9c90498e597793f3922ce6fadd064be`。

## 结论

最快可交付形态是 **candidate 候选版**，用于当前 Windows 机器或小范围验收；正式 GitHub Release 不是当前最快路径。

- 功能矩阵：`24/28 verified`、`4 partial`、`0 blocked`、`release_ready=false`。
- 候选门禁静态校验已通过：`FEATURE_PARITY=85.7% (24/28 verified, 4 partial, 0 blocked)`。
- 现有 `artifacts/v7-productization/candidate` 产物来自 `50964bc`，不是当前 `b03c34b`，不能直接作为当前版本发布。
- 当前提交已推送 GitHub；远端 CI 运行 `33856414359`（`e42f7b3`）全绿。`b03c34b` 的新 CI `33856845174` 正在运行，必须等其结束后再生成候选产物。

## 最短动作链

1. 等待 `3a6b158` 对应 GitHub Actions 的 v7 CI 变为 success。
2. 在干净 `main` 上执行：

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
   ```

3. 只把当前提交生成的 `candidate` 目录交给 Windows 验收；不要复用旧 ZIP/MSI/EXE。
4. 候选验收通过后，再决定是否执行本机 `E:\h` 安装和桌面扩展发布；每次更新先删除旧副本，只保留一个版本。

## 正式发布阻塞项

正式 `v7.0.0` 需要全部 `28/28 verified`、`release_ready=true`，以及当前提交绑定的 candidate manifest 和四项 release evidence：`browser`、`performance`、`installer`、`rollback`。因此不能通过只改状态字段或复用历史产物提前发布。

剩余 partial 的真实工作集中在：队列/详情专项验证、认证 HLS 与浏览器接管的当前提交复验，以及干净 Windows 机器的 Presenter 崩溃恢复和 MSI 生命周期门禁。

## 本轮约束

本轮未在本机编译、打包或安装；只完成源码静态检查、候选门禁静态校验和 GitHub 同步。候选命令保留为下一步唯一必要构建动作。
