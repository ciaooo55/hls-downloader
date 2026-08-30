# HLS Downloader 7.0.0 本地升级说明

## 当前安装

本机 v7 安装在：

```text
E:\h
```

开始菜单入口和桌面快捷方式均为 `HLS Downloader 7.0.0`。安装目录包含内置 JRE、Rust Engine、Native Host、热确认 Presenter、FFmpeg/FFprobe/FFplay、libmpv、Chromium/Firefox 扩展包和本说明；安装脚本同时将当前浏览器扩展包复制到桌面，并清理同浏览器的旧副本。

## 升级命令

先运行测试和候选构建：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task test
pwsh -NoProfile -Command "& { .\scripts\adversarial-v7.ps1 -Scope @('native','browser','transfer') }"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
```

候选包用于外部 Windows 实机验证，要求 canonical feature parity、无
blocked 项且 Git 工作树干净，允许 partial 以便用验收证据关闭它们，不要求
`release_ready=true`。全部 28 项 verified 且满足正式发布门禁后，再构建正式包：
`candidate` 和 `package` 会先校验 pnpm `11.7.0`，再执行 `pnpm install --frozen-lockfile` 与 WXT 生产构建，
并把版本为 `7.0.0` 的 Chromium/Firefox ZIP 一起写入 Portable 和安装镜像；每个产物目录同时写入
`ARTIFACT-MANIFEST.json`，记录提交、版本和 SHA-256。

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task package
```

再执行本机事务升级：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-v7-local.ps1 `
  -ArtifactManifestPath .\artifacts\v7-productization\candidate\ARTIFACT-MANIFEST.json
```

脚本只允许安装到 `E:\h`，并只从 manifest 记录且哈希匹配的 candidate/formal Portable
解包安装；不会读取构建缓存或工作树 `extension/.output`。它先构建完整暂存镜像，保留原安装为临时
`E:\h.v7-backup`，再原子切换、注册 Native Messaging 并刷新开始菜单、桌面
快捷方式和浏览器扩展包，同时移除已知旧插件目录及浏览器名后的所有 ZIP 副本后缀。后置步骤失败时会自动恢复旧镜像及桌面插件；成功后删除临时备份，
因此本机始终只保留一个安装。本机数据库、默认下载文件和可恢复任务状态位于
`%LOCALAPPDATA%\HLS Downloader\v7`，不随 `E:\h` 程序镜像替换。覆盖前脚本会先请求现有 v7 工作台优雅退出；若退出或目录移动失败，不会删除原 `E:\h`。

Portable helper 还会校验 App-Image 内 provenance 的 v7.0.0 版本、candidate/formal tier、当前 commit/tree 和 feature parity SHA-256；旧的或未绑定当前源码的 App-Image 不会被打包。包内升级脚本同样拒绝缺少这些 provenance 字段或任一浏览器扩展包的镜像。

## 安装后验证

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-installed-v7.ps1 -InstallDir E:\h
```

验证要求工作台、Engine 和 Presenter 都来自安装目录，版本为 `7.0.0`，窗口可见且图标有效。测试 API 只在该验证脚本启动的进程中启用，正常从开始菜单启动时不会开放。

## 浏览器扩展

扩展包位于：

```text
E:\h\extensions\HLSDownloader-7.0.0-Chromium.zip
E:\h\extensions\HLSDownloader-7.0.0-Firefox.zip
```

Native Messaging 注册表项指向安装目录中的 `HLSDownloaderNativeHost.exe`。Chrome、Edge、Brave、Chromium、Vivaldi、Opera 和 Firefox 共用同一个 v7 Host 身份，不注册 v6 Host。

确认 v7 正常运行后，可执行 `scripts\cleanup-v7-legacy-install.ps1 -Apply` 移除已知 v6 程序目录和失效快捷方式。脚本检测到旧目录包含 `config.json` 或 `data.db` 时会拒绝删除。

## 回滚

覆盖升级前的程序镜像保存在：

```text
E:\h.v7-backup（仅在事务失败恢复期间短暂存在）
```

Portable 使用包根的 `data` 保存数据库，使用 `downloads` 保存下载文件及 `.hls-tasks` 断点；不会与本机安装或另一份 Portable 共享状态。升级可使用包内 `scripts\upgrade-v7-portable.ps1 -Rollback -RollbackDir <目录>` 回滚。升级和回滚会先复制这两个状态目录；事务失败时会恢复两侧镜像。正式清理前必须保留一个已验证 Portable ZIP 或上述本机备份。

## 与旧版本的关系

- v3.0.39：页面几何、任务工作流和功能入口基线。
- v5.x：协议覆盖、异常处理和浏览器行为基线。
- v6.0.1：Rust/Slint 历史发布参考。
- v7.0.0：Compose 唯一主工作台、Rust 唯一 Core、WXT 唯一浏览器扩展、Presenter 仅负责低延迟临时窗口。

旧源码不复制到活动树，通过 Git 标签查看。性能和门禁数据见 `docs/v7-verification.md`。
