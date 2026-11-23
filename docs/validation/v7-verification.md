# 当前验证状态

> **当前版本状态**：最新已发布正式版为原生 Windows x64 版 [10.0.0](../releases/v10.0.0.md)（成品身份与验收范围以该发布的 `ARTIFACT-MANIFEST.json` 和 `RELEASE-EVIDENCE.json` 为准）。当前源码目标版本为 [10.1.0（发布草案）](../releases/v10.1.0.md)，尚处于发布前迭代阶段（`config/release/feature-parity.json` 中 `release_ready=false`），尚未完成正式打包验收与发布；历史版本的验收结果不自动继承到当前 10.1.0 源码。

Windows 安装包未签名，更新需手动安装；Firefox 正式版永久安装需要 Mozilla 签名。真实投屏设备、云字幕质量和物理多显示器切换仍按既有排除边界处理，不计为已实机验证。

## 历史 7.0.x 验收记录

- **7.0.10**：[验收索引](current-evidence.json)当前保留历史正式版 [7.0.10](../releases/v7.0.10.md)（提交 `deb42d1b77f00616a3aa0d30c66365b1c85f8b58`）的构建、安装、浏览器、原生功能与发布检查链接，不代表 10.1.0 源码的验收结论。
- **7.0.9**：历史原生 Windows x64 版 [7.0.9](../releases/v7.0.9.md) 的发布提交为 `d2fdc0688e02b5bad9cdeec919add2485de524d0`。该版本 EXE 在独立 Windows 环境通过默认目录安装、中文自定义路径安装和开始菜单快捷方式启动；MSI 验证了从 7.0.8 升级、失败回滚及两种情况下的断点恢复；正式发布后，Edge 和 Firefox 分别安装公开 EXE 并加载配套插件，验证了原先没有主程序进程时可从插件打开工作台。

[IDM 对应表](idm-source-audit.md)和[外部源码整合记录](../architecture/source-integration-20261117.md)记录实现依据。旧版验收记录可以通过 Git 标签查看。
