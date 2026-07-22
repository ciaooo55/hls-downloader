# Windows 发布流程

本项目使用 GitHub Actions 从源码生成 Windows 安装版、便携版、油猴脚本和 SHA256 校验文件。`bin/` 与 `release/` 由 `.gitignore` 排除，不应手动提交二进制产物。

## 手动验证构建

1. 打开仓库的 `Actions` 页面。
2. 选择 `Windows Release`。
3. 点击 `Run workflow`，填写版本号后运行。
4. 等待任务通过，从任务页面下载 `HLSDownloader-Windows-x64` artifact。
5. 确认其中包含安装版、便携版、油猴脚本和 `SHA256SUMS.txt`。

手动运行只生成临时 artifact，不会创建公开 Release。

## 发布正式版本

确认 `main` 的 CI 和手动打包均通过后执行：

```powershell
git switch main
git pull --ff-only
git tag v1.3.4
git push origin v1.3.4
```

`v*` 标签会触发完整 Windows 构建。成功后工作流自动创建同名 GitHub Release，并上传：

```text
HLSDownloader-Windows-x64-Setup.exe
HLSDownloader-Windows-x64-Portable.zip
m3u8-sniffer.user.js
HLSDownloader-Chrome.zip
HLSDownloader-Firefox-Unsigned.zip
HLSDownloader-Firefox-Source.zip
SHA256SUMS.txt
```

Firefox 商店版使用 `hls-downloader-store@ciaooo55.com` ID。首次提交时在 AMO
的“提交新附加组件”页面选择“在此网站上”，上传
`HLSDownloader-Firefox-Unsigned.zip`，由 Mozilla 审核和签名。不要先使用同一 ID
执行 `web-ext sign --channel unlisted`；该通道用于自分发，会预先占用 ID，导致
创建公开商店条目时出现“发现重复的附加组件 ID”。后续版本从原附加组件的
“状态和版本”页面上传并保持 ID 不变。

旧的自分发版 ID `browser@hls-downloader.ciaooo55.com` 仍保留在桌面端 Native
Messaging 允许列表中，避免已安装用户失去连接；新商店版需要 v1.3.4 或更高版本
的桌面端。

## 失败处理

- 测试失败：先在本机运行 `python -m pytest -q`、`pnpm test` 和 `pnpm run build`。
- FFmpeg/NSIS 安装失败：在 GitHub Actions 中重新运行失败任务；持续失败时检查 Chocolatey 服务状态。
- 打包后启动失败：下载工作流日志，查看 `Smoke test packaged app` 步骤。
- 安装包异常偏小或无法合并：确认构建日志中的 FFmpeg/ffprobe 版本验证通过，并实际运行便携包内的两个程序检查版本。
- Release 缺文件：不要手动补传；修复工作流后删除错误标签和 Release，再重新创建标签。

GitHub 自动提供发布所需的 `GITHUB_TOKEN`。工作流不读取或保存本机 `GH_TOKEN`。

