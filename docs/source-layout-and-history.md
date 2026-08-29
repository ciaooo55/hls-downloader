# 源码布局与历史版本

## 一套源码仓库

这是一个 Git 仓库，不是多套互相复制的源码。当前工作树按运行层拆分：

| 路径 | 代码职责 | 版本关系 |
| --- | --- | --- |
| `extension/` | WXT MV3 浏览器扩展 | v7 唯一浏览器入口 |
| `native_shell/` | Rust Core、协议、下载引擎、数据库 | v7 唯一核心 |
| `presenter_ui/` | Slint 热窗口 presenter | v7 低延迟临时窗口，不是主工作台 |
| `desktop_ui/` | Kotlin Compose 工作台 | v7 唯一主工作台 |
| `scripts/` | v7 构建、升级、测试和清理脚本 | v7 唯一维护入口 |

`desktop_ui/resources/common` 是打包时从受校验的外部缓存生成的运行资源目录，
不是源码并由 `.gitignore` 排除；GitHub 不保存 FFmpeg、libmpv 或构建产物。

## 查看旧版本

旧版本都在同一个 Git 历史中，当前工作树不用切换就能读取：

```powershell
git show v3.0.39:frontend/package.json
git show v5.0.13:frontend/package.json
git show v6.0.1:native_ui/Cargo.toml
git log v3.0.39..v6.0.1 --oneline
```

需要对照完整旧目录时，先复制到项目外临时目录再查看：

```powershell
git archive v3.0.39 | tar -xf - -C D:\HLSDownloader-archives\source-check\v3.0.39
git archive v5.0.13 | tar -xf - -C D:\HLSDownloader-archives\source-check\v5.0.13
```

当前 v7 源码位于 `main` 分支，正式 `v7.0.0` Git tag 已创建；后续修复继续
通过 `main` 分支提交，发布门禁和验证记录见 `docs/v7-verification.md`。

## 当前与历史的边界

- v3.0.39：通过 Git tag 查看 React 页面、任务入口和交互参考。
- v5.0.13：通过 Git tag 查看 Python/React/Tauri 功能和异常处理参考。
- v6.0.1：通过 Git tag 查看 Rust Core/Slint 发布参考。
- v7：当前工作树只保留 `desktop_ui/` + `native_shell/` + `presenter_ui/` + `extension/`；旧活动目录已经移除。
