# 当前验证状态

当前公开版本为原生 `v7.0.4-rc3`。2026-11-13 的桌面业务、安装启动和长期下载开发诊断已完成；最终正式交付仍要求冻结提交的同源成品验收，不能把其他提交的报告直接计入通过项。

- 当前逐项状态：[验证清单](current-evidence.json)。
- 本轮修改、结果及限制：[收敛记录](convergence.md)。
- 当前开发诊断：工作区 `artifacts/release-native-20261113/`；较早收敛日志保留在 `artifacts/convergence-20261031/`。
- 功能合同：`config/release/feature-parity.json`；readiness 只表示实现已可进入冻结发布门禁，正式发布仍需同源 CI、浏览器、原生完整验收和哈希绑定的发布证据。

历史截图、日志和中间候选记录已从当前工作树移除。完整原文仍在上述 Git 提交，例如：

```powershell
git show 27fd2d06:docs/validation/full-audit-20261027/current-evidence.json
git show 27fd2d06:docs/validation/free-parity-20261030/README.md
```

旧版本安装包和源码的测试结果仅适用于当时的组件与哈希。未解决问题继续记录在当前清单中；移除旧材料不表示问题已解决。
