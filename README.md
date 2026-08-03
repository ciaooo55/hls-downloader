# HLS Downloader

[![CI](https://github.com/ciaooo55/hls-downloader/actions/workflows/ci.yml/badge.svg)](https://github.com/ciaooo55/hls-downloader/actions/workflows/ci.yml)
[![Windows Release](https://github.com/ciaooo55/hls-downloader/actions/workflows/release.yml/badge.svg)](https://github.com/ciaooo55/hls-downloader/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Python](https://img.shields.io/badge/Python-3.12-3776AB?logo=python&logoColor=white)](requirements.txt)
[![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](frontend/src-tauri)
[![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=000)](frontend/package.json)

一个面向 Windows 的本地下载管理器。支持点播 HLS、非 DRM DASH、普通 HTTP/HTTPS 文件和 BT/magnet，提供暂停续传、边下边播、浏览器接管及统一任务列表。

程序只监听 `127.0.0.1`，任务、配置和视频均保存在本机。关闭主窗口后程序会留在系统托盘继续下载，可从托盘重新打开或彻底退出。


## 🔎 项目速览

| 项目 | 说明 |
| --- | --- |
| 适用平台 | Windows 10/11 x64 |
| 当前版本 | `3.0.16` |
| 桌面端 | Tauri 2 + React 19 + TypeScript |
| 下载核心 | Python 3.12 + FastAPI |
| 浏览器扩展 | WXT / Manifest V3 / Native Messaging |
| 支持协议 | HLS、非 DRM DASH、HTTP(S)、BT / magnet |
| 本地服务 | 默认仅监听 `127.0.0.1:8765` |

## 🏗️ 技术架构

桌面主程序锁定 **Tauri 2 + React 19 + Vite + Tailwind CSS v4 + Zustand**，下载核心为 **FastAPI**，浏览器扩展为 **WXT**。详见 [docs/architecture.md](docs/architecture.md) 与 [DESIGN.md](DESIGN.md)。

## 📦 下载

从 [Releases](https://github.com/ciaooo55/hls-downloader/releases/latest) 下载最新版：

| 文件 | 用途 |
| --- | --- |
| `HLSDownloader-v3.0.16-Chrome-Edge-Extension.zip` | Chromium MV3 扩展包，支持 Chrome、Edge、Brave、Chromium、Vivaldi 与 Opera；解压后在扩展管理页开启开发者模式并加载已解压的扩展程序 |
| `HLSDownloader-v3.0.16-Firefox-Web-UI-Unsigned.zip` | 网页显示版 Firefox 插件，AMO 上传或临时测试用 |
| `HLSDownloader-v3.0.16-Firefox-Web-UI-Source.zip` | 网页显示版对应的 AMO 审核源码包 |
| `HLSDownloader-v3.0.16-Firefox-No-Web-UI-Unsigned.zip` | 网页不显示版 Firefox 插件，功能与显示版完全一致，仅发布 ID 不同 |
| `HLSDownloader-v3.0.16-Firefox-No-Web-UI-Source.zip` | 网页不显示版对应的 AMO 审核源码包 |

安装包和便携包由 GitHub Actions 从源码自动构建，不保存在 Git 仓库中。桌面主程序使用 Cockpit Tools 同类的 Tauri + React 架构，包含下载核心、FFmpeg、ffprobe、播放器资源和 Chromium 浏览器插件；Windows 10/11 自带的 Microsoft Edge WebView2 运行时为其渲染界面。

> 当前安装包没有商业代码签名证书。Windows SmartScreen 首次运行时可能显示未知发布者，请只从本仓库 Releases 下载；应用更新使用 GitHub Release 资产提供的 SHA-256 digest 校验安装包。

## 🚀 使用

1. 安装版运行安装程序；便携版完整解压后运行 `HLSDownloader.exe`。
2. 点击顶部“新建”，粘贴普通文件、m3u8 或含视频的网页地址。
3. 确认文件名、保存目录和并发数后开始下载。
4. 下载阶段可暂停、恢复或取消；分片完成后会显示单独的合并进度。
5. 下载达到可播放长度后，可以点击“边下边播”；完成后同一窗口会自动切换为本地 MP4 播放。
6. 播放器显示当前下载速度，进度条支持悬停缩略图、拖动预览、倍速、音量、画中画和全屏。

单个任务无需先勾选：点击任务行右侧菜单即可开始、暂停、恢复、取消、重试、播放、查看日志、打开文件位置或删除。勾选框用于批量操作，支持 Ctrl/Shift 连选。

工具栏右侧有明确的“更新”按钮，可随时检查版本。自动更新安装包保存在设置中的下载目录，安装成功后会自动删除。

安装版的设置和任务历史位于 `%LOCALAPPDATA%\HLS Downloader`，默认视频目录为 `%USERPROFILE%\Downloads\HLS Downloader`。缓存与过程文件目录可在设置中单独指定，默认使用安装目录；卸载会清除程序数据和缓存，并询问是否同时删除视频。便携版的运行数据保存在解压目录中。

## ✨ 功能

- m3u8 直链和网页链接识别
- HLS 文件名会综合服务器响应、播放清单元数据、网页标题和 URL 推断，避免只保存成 `video.mp4`
- 浏览器兼容 TLS 指纹，减少 CDN/Cloudflare 对安装包网络栈的误拦截
- 固定 worker 队列并发下载，默认每任务 12 路、可配置到 256 路
- 暂停、恢复、取消、重试和批量任务
- Tauri + React 桌面主窗口、任务列表、设置、日志和浏览器接管确认（Cockpit 风格悬浮工作区）
- 剪贴板监视：复制 m3u8/磁力/媒体直链自动弹出一键下载提示（可在设置关闭）
- 全部/进行中/已完成与媒体/程序/压缩包/其他分类，支持 Ctrl、Shift 和拖动范围多选
- 分片、速度、ETA、合并与转封装进度
- 浏览器下载确认可预览类型和大小、修改文件名、选择并记忆分类保存目录
- 内置播放器：边下边播、完成后本地 Range 播放、下载速度、缩略图预览和 0.5x-3x 倍速
- 播放器按需加载，缩略图只在悬停时解码并限制缓存，不占用下载 worker
- 断点续传和原子临时文件
- HTTP 字节级断点检查点：每秒持久化已落盘位置，短效签名 URL 更新后按 ETag/Last-Modified 安全续传
- 失败任务可在详情中更新 URL/Cookie 并继续；浏览器捕获同一资源的新签名时自动复活原任务
- LL-HLS 阻塞轮询地址会自动移除过期的 `_HLS_msn/_HLS_part/_HLS_skip` 游标，保留真实会话凭据并合并为同一直播资源
- HLS/DASH 直播分片先强制落盘再提交带文件大小的原子检查点；独立音视频轨可同步录制、停止后无损合并
- 支持粘贴浏览器“复制为 cURL”，导入 URL、请求头、Cookie、Basic Auth 和安全可重放 POST
- 按站点 Host 通配规则设置 UA、Referer、Origin、自定义头、并发和单任务限速
- 系统、直连或手动 HTTP(S)/SOCKS5 代理，支持认证 URL 与 Host 通配绕过
- 下载及直播录制期间自动阻止 Windows 休眠，全部任务停止后立即恢复系统电源策略
- 并发同名输出保护
- AES-128 显式 IV、默认 sequence IV 和 key rotation
- BYTERANGE 显式/连续偏移与严格 Range 校验
- 多层主清单递归、循环检测和最高带宽变体选择
- fMP4 init map、map 切换和 discontinuity
- 重启恢复任务历史
- Windows 系统托盘、单实例唤醒和可靠退出
- 工具栏检查更新、启动更新提示、GitHub SHA256 digest 校验和一键下载安装
- 更新包保存到下载目录并在安装成功后自动删除
- 安装或升级前自动关闭正在运行的安装版或便携版实例
- 设置页、开始菜单和 Windows“已安装的应用”卸载入口
- 深色/浅色界面切换

## 🧭 支持范围

支持点播 HLS（含外挂字幕自动保存为 .vtt/.srt）、直播 HLS 录制（可手动停止或设置时长上限）、非 DRM DASH、严格 Range 的 HTTP 续传和 libtorrent BT。SAMPLE-AES/DRM、受保护 EME、无法重放的 `blob:`/POST 下载不会尝试绕过。

## 🧩 浏览器插件

插件有改动的 Release 会同时生成两个 Firefox 插件包：网页显示版与网页不显示版。它们功能和源码完全一致，仅 Mozilla 发布 ID 不同。安装版内置 Chromium 插件目录，并为 Chrome、Edge、Brave、Chromium、Vivaldi、Opera 和 Firefox 自动注册 `com.ciaooo55.hls_downloader` Native Messaging Host；首次使用时在工具栏打开“浏览器插件”，按界面提示完成一次性加载。插件先通过 Native Messaging 完成本机可信配对和冷启动，随后使用并发友好的 loopback HTTP 通道，断线或核心重启时自动退回 Native 并重新配对；多个浏览器及不同插件版本可同时在线，用户不需要复制或配置 Token。用户点击真实链接或带有下载语义的按钮后，插件才会登记接管意图；随后浏览器创建真实 `DownloadItem` 时，插件立即暂停并暂时隐藏浏览器下载 UI，并按 `webRequest.requestId` 跟踪 PHP/脚本跳转的完整重定向链、`Content-Disposition`、最终文件名、类型和大小。普通的播放、展开、登录等页面按钮不会登记接管意图。媒体悬浮按钮按具体 `<video>` 保存播放证据：直链必须精确匹配，同一 frame 有多个 MSE 播放器时不做模糊归属；iframe 的真实请求头、Referer/Origin 缺失状态和 Cookie 按资源源站分别传递。桌面端成功打开下载确认对话框后，插件立即取消并清除浏览器副本；用户之后选择下载或取消都只由桌面软件处理。只有桌面端离线或无法接收接管请求时才恢复浏览器下载。页面嗅探只登记资源，不会自行启动下载，按住 Alt 点击可临时绕过接管。

扩展支持响应嗅探、页面 fetch/XHR/media/Performance 观察、右键下载和 magnet 链接，主检测器与 MAIN-world 监听器均在 iframe 内运行。识别结果按“标签页 + 当前页面 URL + frame”隔离；只有媒体元素地址精确匹配，或播放前后短时间内同 frame 捕获到的资源才会显示，避免把广告、图片、PHP 页面或后台预览流误认为目标视频。检测到可见视频后，下载按钮会贴在视频右上角，多清晰度时点击选择。无可见视频时仍可使用右侧折叠资源面板。Cookie 必须按站点单独授权，桌面任务中的 Cookie 使用 Windows DPAPI 加密后再写入数据库。Chrome/Edge 商店安装和 AMO 安装由浏览器自动更新；安装包内置的 Chromium 解压插件会随桌面版覆盖升级，但升级后需在扩展管理页重新加载。Windows Chrome 的非商店自托管更新仅适用于企业策略环境；Firefox 自托管自动更新必须使用 Mozilla 签名包和 HTTPS 更新清单，未签名审核 ZIP 不能静默更新或永久安装。

Firefox 网页显示版使用已发布的 AMO ID `browser@hls-downloader.ciaooo55.com`，网页不显示版使用独立 ID `hls-downloader-store@ciaooo55.com`。首次提交时，在 AMO 的“提交新附加组件”页面选择“在此网站上”，上传对应的 `Unsigned.zip`。不要先对商店 ID 执行 `web-ext sign --channel unlisted`，否则它会被注册为自分发扩展，随后创建公开商店条目会提示“发现重复的附加组件 ID”。以后更新必须进入“我的附加组件 → HLS Downloader → 状态和版本 → 上传新版本”，保持该版本对应的 ID 不变并提高版本号。每个 Source.zip 已内置与同名 Unsigned.zip 一致的默认 ID。

校验通过后，源码问题选择“是”，再上传同一 Release、同一变体对应的 `Source.zip`。审核说明见源码包内的 `AMO-BUILD.md`，隐私政策见 [PRIVACY.md](PRIVACY.md)。未签名 ZIP 不能拖进正式版 Firefox；临时测试时先解压，在 `about:debugging#/runtime/this-firefox` 中选择“临时载入附加组件”，再选择解压目录里的 `manifest.json`。

默认每个任务使用 12 个分片并发，最高可配置为 256，最多同时下载 3 个任务。普通 HTTP 文件使用严格 Range 分段并发，源站不支持 Range 时自动退回单连接。设置中可单独指定“缓存与过程文件目录”，默认使用软件安装目录；分片、断点、BT 数据和日志保存在其中的 `.tasks` 子目录。成功任务会立即清理自己的过程文件；暂停或失败任务会保留续传和诊断文件。最终文件位于其他磁盘时会安全复制到目标盘后再原子完成，不会因 Windows 跨盘重命名失败。

播放器使用已下载的连续分片生成临时本地 HLS 清单，默认至少积累 6 秒后开放播放；下载完成后使用带 `faststart` 的 MP4 和 HTTP Range，避免再次读取源站。关闭播放器会释放会话，空闲会话超时后自动清理临时文件。

## 🛠️ 源码运行

需要：

- Windows 10/11 x64
- Python 3.12
- Rust stable（构建 Tauri 桌面端需要）
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

## 🧪 测试

```powershell
python -m pytest -q

cd frontend
pnpm test
pnpm run build
pnpm run tauri:build
```

后端还配置了 Ruff、mypy 与覆盖率规则；修改下载核心或数据模型时可额外运行：

```powershell
python -m ruff check backend tests
python -m mypy backend
python -m pytest --cov
```

真实下载、浏览器接管、安装/升级、系统托盘与休眠抑制涉及 Windows 和外部网络，自动测试之外还应按 `scripts/smoke_*.py`、`scripts/smoke-*.ps1` 的适用条件做针对性冒烟，运行前先查看脚本参数与副作用。

## 📦 本地打包

打包需要 PyInstaller、NSIS、FFmpeg 和 ffprobe：

```powershell
python -m pip install -r requirements-build.txt
choco install ffmpeg nsis -y
.\scripts\build_installer.ps1 -Version 3.0.16
```

输出位于忽略的 `release` 目录：

```text
HLSDownloader-v3.0.16-Windows-x64-Setup.exe
HLSDownloader-v3.0.16-Windows-x64-Portable.zip
```

插件没有改动时不要上传独立插件包。只有需要发布浏览器插件新版本时，打包时追加 `-IncludeExtensionAssets`，GitHub Actions 手动运行时勾选 `include_extensions`。

## 🚢 GitHub 自动发布

- 推送到 `main` 或提交 Pull Request：运行 Python 测试、前端测试和生产构建。
- 在 Actions 页面手动运行 `Windows Release`：生成可下载的工作流产物，不创建正式 Release。
- 推送 `v*` 标签：自动测试、打包、计算 SHA256，并创建对应 GitHub Release；只有相对上一标签检测到 `extension/` 变化时才附带插件资产。

发布示例：

```powershell
git tag v1.4.3
git push origin v1.4.3
```

详细流程见 [docs/releasing.md](docs/releasing.md)。

## 🗂️ 项目结构

```text
backend/       FastAPI、统一任务调度、下载核心与 Native Host
extension/     WXT/React Chrome 与 Firefox MV3 扩展
frontend/      React/TypeScript + Tauri 桌面界面与内置播放器
installer/     NSIS 安装程序定义
scripts/       Windows 打包脚本
tests/         Python 自动化测试
.github/       CI 与自动 Release 工作流
```

## ⚙️ 配置与数据目录

默认配置模板是 [`config.default.json`](config.default.json)。推荐优先通过桌面设置页修改，不要直接共享个人运行配置。

| 配置项 | 默认值 / 说明 |
| --- | --- |
| `host` / `port` | `127.0.0.1` / `8765` |
| `default_concurrency` | 单任务 12 路并发 |
| `max_concurrent_tasks` | 最多 3 个活动任务 |
| `download_dir` | `downloads` |
| `temp_dir` | 当前运行目录 |
| `proxy_mode` | `system`，也支持直连与手动代理 |
| `ffmpeg_path` | `bin\\ffmpeg.exe` |

安装版数据位于 `%LOCALAPPDATA%\HLS Downloader`，视频默认保存到 `%USERPROFILE%\Downloads\HLS Downloader`；便携版将运行数据保存在解压目录。缓存目录中的 `.tasks` 保存分片、检查点、BT 数据与日志，暂停/失败任务可能依赖这些文件继续运行。

## 🔐 隐私与安全

- 服务默认只监听 `127.0.0.1`，不要改成公网地址。
- `config.json` 中的 token 用于本机 UI、浏览器插件和 Native Messaging 通信，不是 GitHub token。
- 不要把 Cookie、网站账号信息、下载记录或个人配置提交到仓库。
- 仓库不跟踪 `bin`、`release`、数据库、下载目录和构建缓存。
- 导入 cURL、站点请求头或代理认证信息时，应把任务数据库与配置目录视为敏感数据。
- 浏览器扩展只应从本仓库 Release 或可信商店安装；Native Messaging 注册会修改本机浏览器相关配置。
- 本项目不会绕过 DRM。下载、录制或投屏前请确认自己拥有访问和保存内容的权利。

## 📄 License

[MIT](LICENSE)

