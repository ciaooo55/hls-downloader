# 文档索引

当前源码版本为 7.0.4；原生实现、验证和交付状态见[原生迁移记录](validation/native-migration-progress.md)和[功能映射](validation/native-feature-mapping.md)。历史记录通过 Git 查看，历史通过不能代替当前源码验收。

目录按用途划分：`guides/` 为使用说明，`architecture/` 为架构与维护约束，`releases/` 为发布说明和流程，`validation/` 为当前验收范围和证据索引，`images/` 为项目介绍图片。维护入口见 [脚本索引](../scripts/README.md)，产品合同见 [配置说明](../config/README.md)。

## 根目录文件

| 文件 | 用途与保留理由 |
| --- | --- |
| [README.md](../README.md)、[README_EN.md](../README_EN.md) | 中英文项目入口、使用与构建方法 |
| [DESIGN.md](../DESIGN.md) | 桌面布局、交互和进程职责的设计约束 |
| [LICENSE](../LICENSE) | 项目源码许可证 |
| [SECURITY.md](../SECURITY.md) | 漏洞报告方式与安全边界 |
| [PRIVACY.md](../PRIVACY.md)、[TERMS.md](../TERMS.md) | 程序展示或引用的隐私政策、用户协议，不作为临时说明删除 |
| [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) | FFmpeg、libmpv、图标等第三方组件的许可说明 |

## 使用和开发

| 文件 | 说明 |
| --- | --- |
| [源码布局与历史](architecture/source-layout-and-history.md) | 一份源码包含哪些模块、怎样直接读 Git 历史 |
| [模块衔接](architecture/v7-module-map.md) | 功能入口与主要代码位置 |
| [运行架构](architecture/v7-architecture.md) | Core、桌面、插件、Presenter、播放器的所有权边界 |
| [缓存与播放](guides/download-cache-and-playback.md) | 清理规则、边播边下能力和限制 |
| [字幕配置](guides/qwencloud-subtitles.md) | 服务配置、音频预取、等待行为及历史实测边界 |
| [安装与升级](guides/v7-local-upgrade.md) | 普通 MSI 安装、用户目录、手动升级和当前数据边界 |
| [包体积](guides/package-slimming.md) | 保留哪些运行依赖、哪些开发工具不进生产包 |
| [桌面模块](../desktop_ui/README.md)、[Core](../native_shell/README.md)、[Presenter](../native_ui/README.md) | 各模块职责和开发入口 |
| [插件](../extension/README.md)、[Firefox 审核构建](../extension/AMO-BUILD.md)、[插件隐私](../extension/PRIVACY.md) | 插件源码、打包审核和数据流说明 |

## 验收与发布维护

| 文件 | 说明 |
| --- | --- |
| [验证状态](validation/v7-verification.md) | 当前证据索引、未覆盖范围和发布合同区别 |
| [浏览器插件参考与功能补齐](validation/browser-reference-completion.md) | 参考插件机制、本次源码修复、定向检查与尚未闭环项 |
| [IDM 源码与完整性复核](validation/idm-source-audit.md) | 全量导出清点、原版工作链、119 C 模块与重建插件的独立核验及采用边界 |
| [正式发布 runner](releases/v7-release-runner.md)、[运行手册](releases/v7-release-runbook-local.md) | 专用发布环境和按提交绑定的门禁；不是本机当前安装状态 |
| [发布就绪边界](architecture/formal-release-readiness.md) | 功能合同、包 provenance、发布授权的区别 |
| [浏览器媒体推送门禁](architecture/browser-media-push-release-gate.md) | 真实浏览器到设备的证据要求 |
| [更新签名信任](architecture/update-signer-trust.md) | 自动更新信任边界；未签名包仍需手动安装 |
| [代理路由](architecture/proxy-routing.md)、[依赖维护](architecture/dependency-maintenance.md) | 维护时需要保留的行为和依赖约束 |

`validation/` 保留当前记录；旧报告和截图保存在 Git 历史，检索方式见[验证状态](validation/v7-verification.md)。实际测试输出与构建包在工作区 `artifacts/` 中，不修改旧报告来伪造新结果。

工作目录只保留一份源码。依赖、测试输出和安装包属于可重建生成物；需要时在仓库内部生成，用完清理。个人密钥、任务、下载和备份不提交到 Git，也不包含在源码交付中。
