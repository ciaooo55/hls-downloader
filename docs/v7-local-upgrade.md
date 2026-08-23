# HLS Downloader 7.0.0 本地升级说明

## 当前安装

本机 v7 安装在：

```text
%LOCALAPPDATA%\Programs\HLSDownloader
```

开始菜单入口为 `HLS Downloader 7.0.0`。安装目录包含内置 JRE、Rust Engine、Native Host、热确认 Presenter、FFmpeg/FFprobe/FFplay、libmpv、Chromium/Firefox 扩展包和本说明；桌面不放安装包或重复快捷方式。

## 升级命令

先运行测试和生产构建：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task test
pwsh -NoProfile -Command "& { .\scripts\adversarial-v7.ps1 -Scope @('native','browser','transfer') }"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task package
```

再执行本机事务升级：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-v7-local.ps1
```

脚本只允许安装到 `%LOCALAPPDATA%\Programs` 下。它先构建完整暂存镜像，保留原安装为 `HLSDownloader.v7-backup`，再原子切换、注册 Native Messaging 并刷新开始菜单入口。`config.json`、`data.db` 和下载目录由 Core 的 v7 数据目录管理，不从安装镜像中删除。

## 安装后验证

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-installed-v7.ps1
```

验证要求工作台、Engine 和 Presenter 都来自安装目录，版本为 `7.0.0`，窗口可见且图标有效。测试 API 只在该验证脚本启动的进程中启用，正常从开始菜单启动时不会开放。

## 浏览器扩展

扩展包位于：

```text
%LOCALAPPDATA%\Programs\HLSDownloader\extensions\HLSDownloader-7.0.0-Chromium.zip
%LOCALAPPDATA%\Programs\HLSDownloader\extensions\HLSDownloader-7.0.0-Firefox.zip
```

Native Messaging 注册表项指向安装目录中的 `HLSDownloaderNativeHost.exe`。Chrome、Edge、Brave、Chromium、Vivaldi、Opera 和 Firefox 共用同一个 v7 Host 身份，不注册 v6 Host。

确认 v7 正常运行后，可执行 `scripts\cleanup-v7-legacy-install.ps1 -Apply` 移除已知 v6 程序目录和失效快捷方式。脚本检测到旧目录包含 `config.json` 或 `data.db` 时会拒绝删除。

## 回滚

覆盖升级前的程序镜像保存在：

```text
%LOCALAPPDATA%\Programs\HLSDownloader.v7-backup
```

Portable 升级可使用包内 `scripts\upgrade-v7-portable.ps1 -Rollback -RollbackDir <目录>` 回滚。正式清理前必须保留一个已验证 Portable ZIP 或上述本机备份。

## 与旧版本的关系

- v3.0.39：页面几何、任务工作流和功能入口基线。
- v5.x：协议覆盖、异常处理和浏览器行为基线。
- v6.0.1：Rust/Slint 历史发布参考。
- v7.0.0：Compose 唯一主工作台、Rust 唯一 Core、WXT 唯一浏览器扩展、Presenter 仅负责低延迟临时窗口。

旧源码不复制到活动树，通过 Git 标签查看。性能和门禁数据见 `docs/v7-verification.md`。
