# HLS Downloader

[![CI](https://github.com/ciaooo55/hls-downloader/actions/workflows/ci.yml/badge.svg)](https://github.com/ciaooo55/hls-downloader/actions/workflows/ci.yml)
[![Windows Release](https://github.com/ciaooo55/hls-downloader/actions/workflows/release.yml/badge.svg)](https://github.com/ciaooo55/hls-downloader/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

一个面向 Windows 的本地 HLS/m3u8 下载器。它提供传统下载管理器风格的桌面界面、实时分片与合并进度、暂停续传、批量任务，以及配套的油猴嗅探脚本。

程序只监听 `127.0.0.1`，任务、配置和视频均保存在本机。关闭主窗口后程序会留在系统托盘继续下载，可从托盘重新打开或彻底退出。

## 下载

从 [Releases](https://github.com/ciaooo55/hls-downloader/releases/latest) 下载最新版：

| 文件 | 用途 |
| --- | --- |
| `HLSDownloader-Windows-x64-Setup.exe` | Windows 10/11 x64 安装版，带卸载程序和快捷方式 |
| `HLSDownloader-Windows-x64-Portable.zip` | Windows 10/11 x64 便携版，解压后直接运行 |
| `m3u8-sniffer.user.js` | 可单独导入 Tampermonkey 的油猴脚本 |
| `SHA256SUMS.txt` | Release 文件的 SHA256 校验值 |

安装包和便携包由 GitHub Actions 从源码自动构建，不保存在 Git 仓库中。程序内已包含 FFmpeg、ffprobe、前端资源和油猴脚本。

> 当前安装包没有商业代码签名证书。Windows SmartScreen 首次运行时可能显示未知发布者，请只从本仓库 Releases 下载并核对 SHA256。

## 使用

1. 安装版运行安装程序；便携版完整解压后运行 `HLSDownloader.exe`。
2. 在顶部输入框粘贴 m3u8 链接，或粘贴含视频的网页地址进行识别。
3. 确认文件名、保存目录和并发数后开始下载。
4. 下载阶段可暂停、恢复或取消；分片完成后会显示单独的合并进度。
5. 完成后可直接打开视频或所在目录。

安装版的设置、任务历史和 WebView 缓存位于 `%LOCALAPPDATA%\HLS Downloader`，默认视频目录为 `%USERPROFILE%\Downloads\HLS Downloader`。卸载会清除程序数据和缓存，并询问是否同时删除视频；默认保留视频。便携版的所有运行数据仍保存在解压目录中。

## 功能

- m3u8 直链和网页链接识别
- 浏览器兼容 TLS 指纹，减少 CDN/Cloudflare 对安装包网络栈的误拦截
- 固定 worker 队列并发下载
- 暂停、恢复、取消、重试和批量任务
- 分片、速度、ETA、合并与转封装进度
- 断点续传和原子临时文件
- 并发同名输出保护
- AES-128 显式 IV、默认 sequence IV 和 key rotation
- BYTERANGE 显式/连续偏移与严格 Range 校验
- 多层主清单递归、循环检测和最高带宽变体选择
- fMP4 init map、map 切换和 discontinuity
- 重启恢复任务历史
- Windows 系统托盘、单实例唤醒和可靠退出
- 设置页、开始菜单和 Windows“已安装的应用”卸载入口
- 深色/浅色界面切换
- 油猴脚本安装、导出和运行状态检测

## 支持范围

当前只支持点播 HLS。直播清单、SAMPLE-AES/DRM、独立音轨下载和字幕封装会明确提示不支持；外部字幕会跳过并记录提示。

## 油猴脚本

1. 在浏览器安装 [Tampermonkey](https://www.tampermonkey.net/)。
2. 启动下载器，在工具栏点击油猴脚本按钮。
3. 可直接打开安装地址，也可一键导出 `m3u8-sniffer.user.js` 到指定目录后手动导入。
4. 打开含视频的 HTTPS 页面并播放，脚本会显示捕获到的 HLS 地址。
5. 点击“发送下载”，任务会出现在桌面下载器中。

浏览器扩展不允许普通程序读取其脚本安装列表，因此下载器通过脚本向本地服务报到来判断它是否正在运行。

## 源码运行

需要：

- Windows 10/11 x64
- Python 3.12
- Node.js 24
- pnpm 11
- FFmpeg 与 ffprobe

安装依赖：

```powershell
python -m pip install -r requirements-dev.txt
cd frontend
corepack enable
corepack prepare pnpm@11.7.0 --activate
pnpm install --frozen-lockfile
cd ..
```

把 `ffmpeg.exe` 和 `ffprobe.exe` 放到项目的 `bin` 目录，然后运行：

```powershell
.\build_frontend.ps1
.\run_backend.ps1
```

打开 `http://127.0.0.1:8765/ui`。也可以运行 `start.cmd` 完成依赖检查、启动服务和打开教程。

前端开发模式：

```powershell
.\run_frontend.ps1
```

## 测试

```powershell
python -m pytest -q

cd frontend
pnpm test
pnpm run build
```

## 本地打包

打包需要 PyInstaller、NSIS、FFmpeg 和 ffprobe：

```powershell
python -m pip install -r requirements-build.txt
choco install ffmpeg nsis -y
.\scripts\build_installer.ps1 -Version 1.1.6
```

输出位于忽略的 `release` 目录：

```text
HLSDownloader-Windows-x64-Setup.exe
HLSDownloader-Windows-x64-Portable.zip
```

## GitHub 自动发布

- 推送到 `main` 或提交 Pull Request：运行 Python 测试、前端测试和生产构建。
- 在 Actions 页面手动运行 `Windows Release`：生成可下载的工作流产物，不创建正式 Release。
- 推送 `v*` 标签：自动测试、打包、计算 SHA256，并创建对应 GitHub Release。

发布示例：

```powershell
git tag v1.1.6
git push origin v1.1.6
```

详细流程见 [docs/releasing.md](docs/releasing.md)。

## 项目结构

```text
backend/       FastAPI、任务管理、HLS 下载和桌面入口
frontend/      React/TypeScript 下载管理界面
installer/     NSIS 安装程序定义
scripts/       Windows 打包脚本
tests/         Python 自动化测试
userscript/    Tampermonkey 嗅探脚本
.github/       CI 与自动 Release 工作流
```

## 安全说明

- 服务默认只监听 `127.0.0.1`，不要改成公网地址。
- `config.json` 中的 token 用于本机 UI 和油猴脚本通信，不是 GitHub token。
- 不要把 Cookie、网站账号信息、下载记录或个人配置提交到仓库。
- 仓库不跟踪 `bin`、`release`、数据库、下载目录和构建缓存。

## License

[MIT](LICENSE)

