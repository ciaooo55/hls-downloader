# 当前验证状态

当前公开版本为原生 `v7.0.6`，发布源码为 `1c1dc06b7dc63e9c728ae75e5dcff9772781cd7d`。发布范围见 [7.0.6 发布说明](../releases/v7.0.6.md)。`main` 后续的 IDM 参考移植与 Firefox 增量尚未发布，实际代码验证见 [源码验证记录](idm-port-validation.md)，不能直接继承旧成品的通过状态。

- 当前逐项状态：[验证清单](current-evidence.json)。
- 本轮修改、结果及限制：[收敛记录](convergence.md)。
- 当前源码移植记录：[IDM 对应表](idm-source-audit.md#逐模块实施2026-11-14)和[源码验证记录](idm-port-validation.md)；原始日志及开发构建保留在本地忽略目录 `artifacts/idm-port-20261114/`。2026-11-13 及更早材料保留为历史诊断。
- 功能合同：`config/release/feature-parity.json`；readiness 只表示实现已可进入冻结发布门禁，正式发布仍需同源 CI、浏览器、原生完整验收和哈希绑定的发布证据。

历史截图、日志和中间候选记录已从当前工作树移除。完整原文仍在上述 Git 提交，例如：

```powershell
git show 27fd2d06:docs/validation/full-audit-20261027/current-evidence.json
git show 27fd2d06:docs/validation/free-parity-20261030/README.md
```

旧版本安装包和源码的测试结果仅适用于当时的组件与哈希。未解决问题继续记录在当前清单中；移除旧材料不表示问题已解决。
