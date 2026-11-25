# 当前验证状态

> **当前版本状态**：最新已发布正式版为原生 Windows x64 版 [10.0.0](../releases/v10.0.0.md)（成品身份与验收范围以该发布的 `ARTIFACT-MANIFEST.json` 和 `RELEASE-EVIDENCE.json` 为准）。当前源码目标版本为 [10.1.0（发布草案）](../releases/v10.1.0.md)，尚处于发布前迭代阶段（`config/release/feature-parity.json` 中 `release_ready=false`），尚未完成正式打包验收与发布；历史版本的验收结果不自动继承到当前 10.1.0 源码。

Windows 安装包未签名，更新需手动安装；Firefox 正式版永久安装需要 Mozilla 签名。真实投屏设备、云字幕质量和物理多显示器切换仍按既有排除边界处理，不计为已实机验证。

## 已发布正式版验收索引

- **10.0.0**：[当前验证索引](current-evidence.json)记录最新已发布正式版 [10.0.0](../releases/v10.0.0.md)（提交 `ab8fc816e26f95814b3e6170210008f9e20a859c`）的候选构建、CI、安全扫描、浏览器接管、安装器与原生桌面验收记录，不代表 10.1.0 开发分支的最终发布结论。
- **7.0.9 / 7.0.10**：历史 7.0.x 正式版说明保留在 [7.0.9](../releases/v7.0.9.md) 与 [7.0.10](../releases/v7.0.10.md)，逐项规范功能映射见[原生功能映射](native-feature-mapping.md)。

更早版本的发布与验收记录可通过 Git 标签与 `docs/validation/current-evidence.json` 的提交历史查看。
