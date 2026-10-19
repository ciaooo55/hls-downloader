# 维护脚本入口

从仓库根目录执行命令。脚本按职责分组，不复制源码、不创建额外分支或 worktree。构建依赖默认放在 `.tool-cache`，生成包和当次验收输出放在 `artifacts`；这些目录不属于源码。

## 常用入口

| 入口 | 用途 |
| --- | --- |
| `bootstrap-v7-toolchain.ps1` | 准备锁定版本的构建工具链 |
| `build-v7.ps1` | `run`、`test`、`candidate`、`package` 等统一构建入口 |
| `run-v7-local.ps1` | 使用本仓库工具链启动开发工作台 |
| `cleanup-v7-build-cache.ps1` | 生成物清理；先阅读参数和预览范围，旧盘符清理需显式启用 |
| `configure-qwencloud.ps1` | 个人字幕凭据的加密配置；密钥不进入源码 |

`upgrade-v7-portable.ps1` 和 `V7HashFunctions.ps1` 还会随 Portable 分发，保留在根层以维持包内升级路径。

播放器依赖锁定为既有版本。libmpv 的上游日构建归档可能过期；可将同版本发布包里的 `libmpv-2.dll` 放入构建缓存的 `libmpv-20260814` 目录，构建脚本会核对源码中固定的 SHA-256 后复用。摘要不匹配会终止，不接受任意播放库，也不会自动升级引擎。

## 按职责查找

| 目录 | 内容 |
| --- | --- |
| [build/](build/) | 媒体工具准备、图标生成、MSI 重打包、Portable 和体积报告 |
| [install/](install/) | Host 注册、事务安装、停止进程及旧安装清理；`install-v7-local.ps1` 只适用于其硬限定的维护环境 |
| [release/](release/) | 版本合同、候选接收、正式门禁、发布资产及可选签名工具；未签名交付不会自动运行签名 |
| [validation/](validation/) | 下载、浏览器、字幕、播放器、安装与发布合同验收 |
| [visual/](visual/) | 截图、布局、对比度和视觉验证器的正负对照 |

`validation` 的 Python 工具互相复用同目录帮助函数，因此保持一个模块目录。按名称定位 `smoke_extension_*`（插件）、`smoke_subtitle_*` / `smoke_qwencloud_*`（字幕）、`smoke_v7_player_*`（播放器）、`smoke_v7_*`（Core/桌面）、`test-*`（脚本回归）。`validation/fixtures` 是测试输入，不是运行缓存。

搬迁脚本必须同时更新调用方、CI、仓库根定位和夹具。PowerShell 默认静态检查及 CI Python 语法检查递归覆盖子目录。历史报告中的旧路径保留原样，不改写成新结果。

执行前阅读参数和当前任务授权：部分工具会下载依赖、启动应用、修改本产品安装或产生云端费用，不能将整个目录当作无副作用的一键检查。
