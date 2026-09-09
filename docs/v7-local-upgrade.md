# HLS Downloader 7.0.2 本地升级说明

本文描述当前 `7.0.2` source line 的本机升级路径。安装/扩展版本实际上由 `artifacts/v7-productization/feature-parity.json.product_version` 驱动，不应在脚本逻辑中依赖本文标题。

> 历史说明：公开 `v7.0.1-candidate.1` 是上一轮测试包；它可以作为历史验收证据，但不能替代当前 7.0.2 提交重新生成的 candidate/provenance。

## 当前安装

本机 v7 安装根固定为：

```text
E:\h
```

对于当前 canonical 版本，开始菜单/桌面快捷方式和发布的扩展包应体现 `7.0.2`。安装镜像包含内置 JRE、Rust Engine、Native Host、热确认 Presenter、FFmpeg/FFprobe、libmpv、Chromium/Firefox 扩展包和本说明；bootstrap 工具链可能包含 `ffplay.exe`，但正式运行镜像不依赖它，因为本地播放使用 bundled libmpv。安装脚本同时将当前浏览器扩展包复制到桌面，并清理同浏览器的旧副本。

## 升级命令

干净开发机先 bootstrap 固定工具链，然后运行测试和 candidate：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\bootstrap-v7-toolchain.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task test
pwsh -NoProfile -Command "& { .\scripts\adversarial-v7.ps1 -Scope @('native','browser','transfer') }"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
```

candidate 用于外部 Windows 实机验证，要求 canonical feature contract、无 blocked 项且 Git worktree 干净，允许验证尚未完全结束，不要求 `release_ready=true`。`candidate`/`package` 会校验固定 pnpm/toolchain 并执行 WXT production build；生成的 Chromium/Firefox ZIP、Portable、EXE/MSI manifest/provenance 的产品版本必须等于当前 canonical `product_version`（当前 `7.0.2`），而不是固定写成 7.0.1。

formal package 只有在 canonical completeness、当前 release evidence、clean worktree 和 `release_ready=true` 都满足时才允许生成：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task package
```

当前 canonical `release_ready=false`，因此上面的 formal package 命令在当前状态应保持 fail closed；candidate 构建仍可用于收集验证证据。

## 本机事务升级

使用当前 candidate/formal manifest：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-v7-local.ps1 `
  -ArtifactManifestPath .\artifacts\v7-productization\candidate\ARTIFACT-MANIFEST.json
```

`install-v7-local.ps1` 会从 canonical feature-parity 读取当前 `$productVersion`，只允许安装到 `E:\h`，并只从 manifest 记录且哈希匹配的 candidate/formal Portable 解包安装；不会读取构建缓存或工作树 `extension/.output`。

脚本先构建完整暂存镜像，保留原安装为临时 `E:\h.v7-backup`，再原子切换、注册 Native Messaging 并刷新开始菜单、桌面快捷方式和浏览器扩展包，同时移除已知旧插件目录及浏览器名后的旧 ZIP 副本。后置步骤失败时会自动恢复旧镜像及桌面插件；成功后删除临时备份，因此本机始终只保留一个活动安装。

本机数据库、默认下载文件和可恢复任务状态位于 `%LOCALAPPDATA%\HLS Downloader\v7`，不随 `E:\h` 程序镜像替换。覆盖前脚本会先请求现有 v7 工作台优雅退出；若退出或目录移动失败，不会删除原 `E:\h`。

Portable helper 会校验 App-Image provenance 的产品版本**等于当前 canonical product version**、tier 为 candidate/formal、source commit/tree 和 feature parity SHA-256；旧的或未绑定当前源码的 App-Image 不会被打包。包内升级脚本同样拒绝缺少这些 provenance 字段或任一浏览器扩展包的镜像。

## 安装后验证

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-installed-v7.ps1 -InstallDir E:\h
```

验证要求工作台、Engine 和 Presenter 都来自安装目录，产品/扩展版本与当前 canonical `product_version` 一致，窗口可见且图标有效。测试 API 只在该验证脚本启动的进程中启用，正常从开始菜单启动时不会开放。

## 浏览器扩展

当前 7.0.2 candidate/formal 安装镜像中的扩展包名称为：

```text
E:\h\extensions\HLSDownloader-7.0.2-Chromium.zip
E:\h\extensions\HLSDownloader-7.0.2-Firefox.zip
```

未来版本由 manifest/canonical contract 决定，不应继续复制这两个字面版本到安装脚本。Native Messaging 注册表项指向安装目录中的 `HLSDownloaderNativeHost.exe`。Chrome、Edge、Brave、Chromium、Vivaldi、Opera 和 Firefox 共用同一个 v7 Host 身份，不注册 v6 Host。

## V6 数据迁移

退出 V6 后，本机 V7 首次创建数据库时，如果精确检测到 `%LOCALAPPDATA%\HLS Downloader\v6\data.db`，Core 会先通过 SQLite Online Backup 建立一致快照，在临时库中校验 schema、任务/spec、handoff 和 JSON，并把可定位的相对下载目录固定为绝对路径；全部成功后才原子发布为 V7 数据库。

已有 V7 数据库、Portable 或显式 `HLS_V7_DATA_DIR` 不会触发自动整库迁移，也不会覆盖现有数据。V6 仍在运行或迁移失败时，Core 会保留 V6 源库、删除本次临时库并停止首次启动。

确认 v7 正常运行后，可执行 `scripts\cleanup-v7-legacy-install.ps1 -Apply` 移除已知 v6 程序目录和失效快捷方式。脚本检测到旧目录包含数据库、配置、`downloads` 文件或 `.v6-tasks` 断点时会拒绝删除。

## 回滚

覆盖升级前的程序镜像仅在事务失败恢复期间短暂保存在：

```text
E:\h.v7-backup
```

Portable 使用包根的 `data` 保存数据库，使用 `downloads` 保存下载文件及 `.hls-tasks` 断点；不会与本机安装或另一份 Portable 共享状态。升级可使用包内 `scripts\upgrade-v7-portable.ps1 -Rollback -RollbackDir <目录>` 回滚。升级和回滚会先复制这两个状态目录；事务失败时会恢复两侧镜像。正式清理前必须保留一个已验证 Portable ZIP 或其他明确的可验证回滚来源。

## 与旧版本的关系

- v3.0.39：页面几何、任务工作流和功能入口基线。
- v5.x：协议覆盖、异常处理和浏览器行为基线。
- v6.0.1：Rust/Slint 历史发布参考。
- v7.0.0：已发布稳定升级基线，标签、Release 和资产保持不变。
- v7.0.1：历史 Compose/Rust/WXT candidate 测试线；公开 `v7.0.1-candidate.1` 保留原样。
- v7.0.2：当前活动 source/candidate 版本；正式发布仍由 `release_ready=false` 和 formal release gates 阻止。

旧源码不复制到活动树，通过 Git 标签查看。历史性能数字与当前门禁边界见 `docs/v7-verification.md`。
