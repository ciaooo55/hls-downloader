# 正式发布就绪边界

## 三种状态分开核对

- `config/release/feature-parity.json` 是当前源码的 canonical 功能合同。状态、排除项和版本以受测提交中的实际内容为准。
- 包内 provenance 描述具体交付物。r12 是 `unsigned-prerelease`，自身 `release_ready=false`，不等于 canonical 的状态。
- 验收报告只证明列明的提交、产物哈希、环境和场景。候选构建成功或历史报告不能替代最终提交证据。

正式构建调用脚本检查 canonical completeness、readiness、残留 gap、干净工作树和同提交发布证据；不能只修改元数据或文字绕过。当前范围及排除项见 [验证状态](../validation/v7-verification.md)。

## 冻结与发布

正式发布以最终冻结的 `main` SHA 为单位。后续任何提交都会产生新的待验证 SHA，包括文档提交；旧 SHA 的 CI 不能直接套用。

流水线要求同 SHA 的 CI、安全工作流和手动候选构建成功，再在专用交互式 Windows runner 上产生 candidate 绑定的 browser、performance、browser_media_push、installer 和 rollback 五项证据。构建正式包前复查远端 main 未移动。

runner、真实 Edge/Firefox、局域网接收端、专用 MSI 生命周期环境及发布权限是外部前提。缺少前提应报告阻塞，不能用模拟设备、直接 Core 调用或跳过门禁冒充真实链路。详见 [runner 合同](../releases/v7-release-runner.md)。

## 信任与授权

Windows 包按用户要求不签名，手动安装；自动更新的签名和 signer 信任检查保持启用。网络元数据不能增加受信签名者。

正式流程只为同一冻结提交创建或续用可核对的 annotated tag/draft，逐项核对上传字节数和 SHA-256。只有显式授权并启用发布输入才公开发布。文档整理、候选验收或 metadata readiness 均不构成发布授权。

历史源码直接从 Git 读取；不为发布建立额外分支或源码副本。
