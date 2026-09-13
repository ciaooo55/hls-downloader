# v7.0.2 当前正式发布路径

当前活动源码版本来自 `artifacts/v7-productization/feature-parity.json`，目前为 `7.0.2`。正式发布流程本身不应把版本硬编码在文档或脚本分支里；`.github/workflows/release-v7.yml` 会从该 canonical contract 解析产品版本和 `v<version>` 标签。

> 历史说明：公开的 `v7.0.1-candidate.1` 仍是上一轮可下载测试包，它及其 manifest/provenance/哈希属于 7.0.1 测试线，不是当前 7.0.2 提交的发布证据。

## 当前状态

- canonical 产品版本：`7.0.2`。
- canonical `release_ready=false`，因此**当前没有正式发布授权**，正式 `package` 必须继续 fail closed。
- HLS-C009/C010/C011 已分别收紧自动更新 signer、Core TCP loopback 和跨 origin replay header 边界；这些安全修复不会自动把 `release_ready` 改成 true。
- `v7.0.0` 的已发布标签、Release 和资产保持不变；历史 `v7.0.1-candidate.1` 也保持历史原样。

## 最短安全发布顺序

1. 完成所有已审查仓库工作，冻结最终 `main` SHA；冻结后不要再合入无关提交。
2. 等待该**同一 SHA** 的四个精确 SHA 工作流（三个 push，候选包为 main 上手动 workflow_dispatch）全部成功：
   - `v7 CI` (`.github/workflows/ci.yml`)
   - `v7 Candidate Package` (`.github/workflows/package-v7-candidate.yml`)
   - `Maintenance Security` (`.github/workflows/maintenance-security.yml`)
   - `Rust Security` (`.github/workflows/rust-security.yml`)
3. 只有在 canonical 验证结果确实满足正式发布条件后，才能通过单独、可审查的源码变更把 `release_ready` 改为 true；本说明本身不授权该变更。
4. 冻结新的 ready SHA，并再次等待它自己的四个精确 SHA 工作流（三个 push，候选包为 main 上手动 workflow_dispatch）成功。任何 `main` 移动都会使之前的正式发布候选失效。
5. 由受保护 `v7-release` environment 中的授权操作者，在专用 self-hosted Windows x64 `hls-release` runner 上 dispatch `.github/workflows/release-v7.yml`。不要用普通 hosted runner 替代正式发布机。
6. 正式 workflow 会在同一冻结 SHA 上重新构建 candidate，运行 browser/performance/installer/rollback 门禁，重新确认 `main` 未移动，再构建 formal package、执行 Authenticode 签名/时间戳验证、生成 SBOM/发布证据、创建或恢复 annotated tag + Draft Release，并逐项验证上传资产的 size/SHA-256 digest。
7. 首次验证可保持 dispatch 的 `publish=false`，让通过 digest 校验的 Release 保持 Draft；只有明确授权发布时才使用 `publish=true`。已有 draft 的安全重试仍必须绑定同一 annotated tag 和同一冻结 commit。

## 本地 candidate 的用途

开发/实机验证仍可从干净 worktree 构建 candidate：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
```

candidate 要求 canonical feature contract、`blocked=0` 和干净 worktree，但允许验证尚未完全结束，也不要求 `release_ready=true`。它用于收集证据，不等于正式发布授权。

正式 `package` 则要求 canonical completeness、当前 release evidence、干净 worktree 和 `release_ready=true`：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task package
```

普通本地执行不能替代 formal workflow 的 exact-SHA 工作流检查、冻结 `main` 复核、专用 `E:\h` 生命周期环境、真实浏览器、签名私钥/证书、时间戳和 GitHub Draft digest 验证。

## 失败处理

任一门禁失败后修复并重新冻结；失败提交生成的 candidate、evidence 和 formal package 不能授权另一个 SHA。若 `main` 在门禁或打包过程中移动，formal workflow 必须终止并从新的冻结 SHA 重新取得四个 prerequisite 结果。

MSI 生命周期验证只要求应用进程级重启/恢复，不宣称执行 Windows 系统重启；报告中的 `system_reboot=false` 是该边界的一部分。构建、测试、临时文件和门禁报告留在仓库/runner 工作区，正式安装根仍固定为 `E:\h`。
