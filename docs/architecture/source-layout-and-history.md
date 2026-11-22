# 源码布局与历史版本

## 一套源码仓库

这是一个 Git 仓库，不是多套互相复制的源码。当前工作树按运行层拆分：

| 路径 | 代码职责 | 版本关系 |
| --- | --- | --- |
| `extension/` | WXT MV3 浏览器扩展 | 当前唯一浏览器入口 |
| `native_shell/` | Rust Core、协议、下载引擎、数据库 | 当前唯一核心 |
| `native_ui/` | Rust / Slint 主工作台及确认、进度、完成窗口 | 当前唯一桌面 UI |
| `crates/`、`native_clients/` | 协议、共享客户端及 Host/CLI/Updater | 原生客户端边界 |
| `scripts/` | 构建、安装、发布、功能验收和视觉验收工具 | [维护入口](../../scripts/README.md) |
| `config/` | 受版本管理的产品合同 | 非用户运行配置、非生成物 |
| `docs/` | 当前说明与历史证据 | 按使用、架构、发布、验收分组 |

`artifacts/v7-productization` 下的原生打包暂存区从受校验的外部缓存生成运行资源，
不是源码并由 `.gitignore` 排除；GitHub 不保存 FFmpeg、libmpv 或构建产物。

当前版本不使用仓库根目录的静态 `config.default.json`。运行设置由 Rust Core
持久化并校验，桌面端只通过 v7 IPC 读取/写入公开设置键；这样不会再出现“配置模板默认值”
和实际 Core fallback 分叉的第二套事实来源。

## 当前与历史的边界

`main` 只维护 10.x 的 `crates/`、`native_clients/`、`native_shell/`、`native_ui/` 与 `extension/`。旧架构和旧配置迁移不是当前构建或运行前提，已删除的历史标签不作为源码获取入口。

当前稳定成品见 [10.0.0](https://github.com/ciaooo55/hls-downloader/releases/tag/v10.0.0)。发布证据绑定到对应源码与成品，不自动代表后续 main 已验收。历史证据不改写为当前结果。

`v7` 仍用于现行数据目录、IPC、脚本和工作流文件名；它们是稳定运行身份或工具入口，不表示还保留另一套运行时。

源码完整性包括构建脚本、锁文件、测试、许可和维护文档，不要求保留第三方依赖缓存、编译输出、安装包或源码 ZIP。需要时在仓库内重建；受跟踪的资源和许可继续保留。
