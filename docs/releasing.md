# Windows 发布流程

> v6 切包后入口改为单一 `HLSDownloader.exe`（见 [v6-cutover.md](v6-cutover.md)）。在行为矩阵通过前，下列 5.x 流程仍是现网发布路径。

本项目使用 GitHub Actions 从源码生成 Tauri + React Windows 安装版和便携版。工作流使用 Rust stable 构建桌面界面、PyInstaller 构建无窗口下载核心；`bin/` 与 `release/` 由 `.gitignore` 排除，不应手动提交二进制产物。

## 手动验证构建

1. 打开仓库的 `Actions` 页面。
2. 选择 `Windows Release`。
3. 点击 `Run workflow`，填写版本号后运行。只有浏览器插件本身有改动时才勾选 `include_extensions`。
4. 等待任务通过，从任务页面下载对应版本的 `HLSDownloader-Windows-x64` artifact。
5. 默认确认其中只包含安装版和便携版；勾选 `include_extensions` 时，再确认包含 Chrome/Edge MV3 扩展包，以及统一 Firefox 扩展的 unsigned/source 文件。

手动运行只生成临时 artifact，不会创建公开 Release。

## 发布正式版本

确认 `main` 的 CI 和手动打包均通过后执行：

```powershell
git switch main
git pull --ff-only
git tag v1.4.2
git push origin v1.4.2
```

`v*` 标签会触发完整 Windows 构建。工作流会比较上一个标签到当前标签的 `extension/` 变更：插件有改动时自动附带插件资产，没有改动时只发布桌面端。成功后工作流自动创建同名 GitHub Release，并始终上传：

```text
HLSDownloader-v3.0.8-Windows-x64-Setup.exe
HLSDownloader-v3.0.8-Windows-x64-Portable.zip
```

标签间检测到插件变化时会自动额外上传以下文件；手动工作流仍需勾选 `include_extensions`，本地构建则传入 `-IncludeExtensionAssets`：

```text
HLSDownloader-v3.0.8-Chrome-Edge-Extension.zip
HLSDownloader-v3.0.8-Firefox-Unsigned.zip
HLSDownloader-v3.0.8-Firefox-Source.zip
```

Firefox 所有发布包统一使用 `hls-downloader-store@ciaooo55.com` ID。首次提交时在 AMO
的“提交新附加组件”页面选择“在此网站上”，上传
对应变体的 `HLSDownloader-v*-Firefox-*-Unsigned.zip`，由 Mozilla 审核和签名。不要先使用同一 ID
执行 `web-ext sign --channel unlisted`；该通道用于自分发，会预先占用 ID，导致
创建公开商店条目时出现“发现重复的附加组件 ID”。后续版本从原附加组件的
“状态和版本”页面上传并保持 ID 不变。

桌面端 Native Messaging 白名单只保留此统一 ID；后续 Firefox 更新必须保持它不变。

## 本机构建的 FFmpeg

常规发布和 CI 使用脚本内置 SHA-256 校验的 FFmpeg 归档。仅当本机镜像不可用、且已自行核验 PATH 中的 `ffmpeg.exe` / `ffprobe.exe` 时，开发者可显式传入 `-UseSystemFfmpeg` 完成本机烟雾构建；该开关不会被 CI 使用。

## 失败处理

- 测试失败：先在本机运行 `python -m pytest -q`、`pnpm test`、`pnpm run build` 和 `pnpm run tauri:build`。
- FFmpeg/NSIS 安装失败：在 GitHub Actions 中重新运行失败任务；持续失败时检查 Chocolatey 服务状态。
- 打包后启动失败：下载工作流日志，查看 `Smoke test packaged app` 步骤。
- 安装包异常偏小或无法合并：确认构建日志中的 FFmpeg/ffprobe 版本验证通过，并实际运行便携包内的两个程序检查版本。
- Release 缺文件：不要手动补传；修复工作流后删除错误标签和 Release，再重新创建标签。

GitHub 自动提供发布所需的 `GITHUB_TOKEN`。工作流不读取或保存本机 `GH_TOKEN`。

