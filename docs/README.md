# 文档索引

桌面程序及浏览器扩展请从 [GitHub Releases](https://github.com/ciaooo55/hls-downloader/releases/latest) 获取。当前源码目标版本为 [10.1.0（发布草案）](releases/v10.1.0.md)，最新已发布正式版为 [10.0.0](releases/v10.0.0.md)；历史 [7.0.9](releases/v7.0.9.md) 与 [7.0.10](releases/v7.0.10.md) 发布说明保留在 `releases/` 目录供参考。

文档目录按职责划分：`guides/` 为用户功能与配置指南，`architecture/` 为系统架构与维护契约，`releases/` 为版本说明与发布流程，`validation/` 为功能映射与当前验证索引。维护脚本见 [脚本索引](../scripts/README.md)，版本化配置契约见 [配置说明](../config/README.md)。

## 根目录文件

| 文件 | 用途说明 |
| --- | --- |
| [README.md](../README.md)、[README_EN.md](../README_EN.md) | 中英文项目主页、功能概览、安装与源码构建指南 |
| [DESIGN.md](../DESIGN.md) | 桌面工作台布局、视觉令牌与交互设计契约 |
| [LICENSE](../LICENSE) | 项目开源许可证 |
| [SECURITY.md](../SECURITY.md) | 安全漏洞报告方式与信任边界 |
| [PRIVACY.md](../PRIVACY.md)、[TERMS.md](../TERMS.md) | 应用内引用的隐私政策与使用条款 |
| [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) | FFmpeg、libmpv、图标等第三方组件许可声明 |

## 使用和开发

| 文件 | 说明 |
| --- | --- |
| [免费版高级工具指南](free-edition.md) | 命令行（CLI）、站点抓取同步与远程 ZIP 内容预览用法 |
| [任务导入与导出](guides/task-import-export.md) | JSON / CSV / TXT 任务列表批量导出与导入规则 |
| [缓存与边播边下](guides/download-cache-and-playback.md) | 临时分片清理生命周期、边播边下条件与播放切换机制 |
| [在线字幕配置](guides/qwencloud-subtitles.md) | QwenCloud / 百炼实时字幕翻译配置、音频预取与加密凭据说明 |
| [安装、升级与数据目录](guides/v7-local-upgrade.md) | MSI / EXE / Portable 安装路径、数据目录结构与升级注意事项 |
| [安装包精简](guides/package-slimming.md) | 生产运行依赖构成与测试工具分离原则 |
| [运行架构](architecture/v7-architecture.md) | Rust Core、Slint 桌面端、Native Host 与隔离播放器的进程所有权边界 |
| [模块与功能衔接](architecture/v7-module-map.md) | 核心业务板块与对应源码模块位置 |
| [Workspace 与 IPC 契约](architecture/native-workspace.md) | Cargo Workspace 分层、分页快照、事件恢复与持久化同步策略 |
| [源码布局与历史版本](architecture/source-layout-and-history.md) | 仓库目录职责与历史版本演进边界 |
| [Slint 工作台](../native_ui/README.md)、[Rust Core](../native_shell/README.md) | 桌面工作台与核心引擎模块开发说明 |
| [浏览器扩展](../extension/README.md)、[Firefox 审核构建](../extension/AMO-BUILD.md)、[扩展隐私说明](../extension/PRIVACY.md) | 扩展功能机制、可复现构建与隐私边界说明 |

## 验收与发布维护

| 文件 | 说明 |
| --- | --- |
| [验证状态说明](validation/v7-verification.md) | 当前已发布版本与开发分支验证状态概览 |
| [当前验证索引](validation/current-evidence.json) | 最新已发布正式版的源码提交与自动化检查记录 |
| [原生功能映射](validation/native-feature-mapping.md) | 33 项规范功能契约与传输特性的模块映射记录 |
| [正式发布流水线](releases/v7-release-runner.md)、[发布操作顺序](releases/v7-release-runbook-local.md) | Hosted Windows 候选构建、门禁验收与正式发布流程 |
| [发布就绪边界](architecture/formal-release-readiness.md) | 功能契约、构建快照与正式发布门禁的职责划分 |
| [浏览器媒体推送门禁](architecture/browser-media-push-release-gate.md) | 浏览器到局域网接收端的端到端验证要求 |
| [自动更新签名信任](architecture/update-signer-trust.md) | 自动更新 Authenticode 签名者校验与证书轮换机制 |
| [代理路由策略](architecture/proxy-routing.md)、[依赖维护策略](architecture/dependency-maintenance.md) | 系统/手动代理路由契约与第三方依赖升级规范 |
