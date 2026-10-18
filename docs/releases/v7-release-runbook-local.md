# 专用环境正式发布操作顺序

本手册服务于现有正式发布流水线，不表示当前电脑安装了产品，也不是普通 MSI 的安装指南。前提见 [runner 合同](v7-release-runner.md)，当前包与 canonical 状态区别见 [验证状态](../validation/v7-verification.md)。

1. 完成源码修改，取得同步授权，冻结唯一 `main` 的最终 SHA。
2. 确认同 SHA 的 `v7 CI`、`Maintenance Security`、`Rust Security` push 工作流成功；显式触发并等待 `v7 Candidate Package` 的 main 手动工作流成功。
3. 检查 canonical 的实际 completeness、readiness、gap 和 exclusions，不能凭旧文档提升状态。
4. 需要预就绪媒体推送证据时，先检查 `v7-media-push-readiness.yml` 的前置条件：它只接受 `release_ready=false`。当前合同若为 true，该工作流会拒绝，不能直接套用或为通过而改状态。设备证据要求见 [媒体推送门禁](../architecture/browser-media-push-release-gate.md)。
5. 经授权在合格的专用交互式 runner 触发 `release-v7.yml`，先用 `publish=false`。流水线构建同提交 candidate，记录五项包绑定门禁，再复查 main，构建正式包和证据资产。
6. 核对 draft/tag 的提交身份、每个上传文件的大小与 SHA-256。任一门禁、接收端拉取、升级/回滚或身份检查失败即停止。
7. 仅在获得公开发布授权后执行发布。未签名说明和手动更新要求必须保留。

专用脚本的 `E:\h` 是 MSI 生命周期验证约束；普通安装允许选择路径。执行前备份用户数据，清理仅针对本轮已确认的验收数据；不在日常电脑为“验证完成”而留下安装。
