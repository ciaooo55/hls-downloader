# 源码布局与历史版本

## 一套源码仓库

这是一个 Git 仓库，不是多套互相复制的源码。当前工作树按运行层拆分：

| 路径 | 代码职责 | 版本关系 |
| --- | --- | --- |
| `extension/` | WXT MV3 浏览器扩展 | v7 唯一浏览器入口 |
| `native_shell/` | Rust Core、协议、下载引擎、数据库 | v7 唯一核心 |
| `presenter_ui/` | Slint 热窗口 presenter | v7 低延迟临时窗口，不是主工作台 |
| `desktop_ui/` | Kotlin Compose 工作台 | v7 唯一主工作台 |
| `scripts/` | 构建、安装、发布、功能验收和视觉验收工具 | [维护入口](../../scripts/README.md) |
| `config/` | 受版本管理的产品合同 | 非用户运行配置、非生成物 |
| `docs/` | 当前说明与历史证据 | 按使用、架构、发布、验收分组 |

`desktop_ui/resources/common` 是打包时从受校验的外部缓存生成的运行资源目录，
不是源码并由 `.gitignore` 排除；GitHub 不保存 FFmpeg、libmpv 或构建产物。

v7 不再使用仓库根目录的静态 `config.default.json`。运行设置由 Rust Core
持久化并校验，桌面端只通过 v7 IPC 读取/写入公开设置键；这样不会再出现“配置模板默认值”
和实际 Core fallback 分叉的第二套事实来源。历史版本的配置模板仍可从对应 Git tag 查看。

## 查看旧版本

旧版本都在同一个 Git 历史中，当前工作树不用切换就能读取：

```powershell
git show v3.0.39:frontend/package.json
git show v5.0.13:frontend/package.json
git show v6.0.1:native_ui/Cargo.toml
git log v3.0.39..v6.0.1 --oneline
```

使用 `git ls-tree -r <标签>` 查看旧目录清单，再用 `git show` 读取需要的文件。Git 历史不是另一份工作树，无需导出整套旧源码、创建分支或 worktree。

当前 v7 源码位于唯一的 `main` 分支，最近公开包为 `v7.0.3-r12`。已发布标签保持原样；
源码与候选/正式发布证据绑定到各自提交。是否创建新的公开 Release
以 `docs/validation/v7-verification.md` 的当前发布门禁为准，而不是以历史 tag 状态或单次 CI 结果推断。

## 当前与历史的边界

- v3.0.39：通过 Git tag 查看 React 页面、任务入口和交互参考。
- v5.0.13：通过 Git tag 查看 Python/React/Tauri 功能和异常处理参考。
- v6.0.1：通过 Git tag 查看 Rust Core/Slint 发布参考。
- v7：当前工作树只保留 `desktop_ui/` + `native_shell/` + `presenter_ui/` + `extension/`；旧活动目录已经移除。

源码完整性包括构建脚本、锁文件、测试、许可和维护文档，不要求保留第三方依赖缓存、编译输出、安装包或源码 ZIP。需要时在仓库内重建；受跟踪的 wrapper、资源和许可继续保留。
