# HLS Downloader 7.0.0 升级说明

## 架构升级

- Compose Desktop 是唯一主工作台；主界面不访问 SQLite。
- Rust Core 是唯一下载、调度、数据库、迁移和恢复进程；关闭 UI 不停止下载。
- Native Presenter 是预热的浏览器确认、进度和完成窗口；工作台启动时会确保它常驻，单实例锁防止重复窗口。
- WXT MV3 扩展支持 Chromium 和 Firefox，通过 Native Messaging 连接同一个 Core。
- 播放器使用独立进程，崩溃或关闭不会终止下载。

## 用户体验

- 保留 v3 的标题栏、工具栏、队列/分类栏、任务表和底部状态栏结构。
- 任务行只显示一个总进度；协议和真实后缀直接显示在文件名下。
- 点击任务行任意非命令区域即可选择；支持按住左键框选、Ctrl 增减选择、Shift 连续范围、多选批量操作和键盘导航。
- 右键菜单跟随鼠标，并按媒体、程序、压缩包等文件能力过滤操作。
- 主窗口不持续置顶，只在浏览器新任务、下载完成、失败或错误时请求关注。
- 设置按下载与目录、连接、计划、浏览器、媒体、投屏与推送、外观、通知、维护和关于分类。
- 投屏与 TVBox 推送使用不同入口和状态，支持真实局域网发现、离线反馈和局域网媒体发布。
- 浏览器悬浮层仅显示与资源匹配的下载、投屏和 TVBox 操作，并保留页面 Referer/Origin 与同源凭据边界。
- 错误日志以 UTF-8 JSONL 保存，包含时间、组件、事件、任务和请求编号，便于维护定位。

## 功能范围

支持 HTTP/HTTPS、FTP/FTPS、SFTP、HLS/LL-HLS、DASH、直播、BT/磁力、本地种子、Curl、Metalink、批量链接和网页抓取；支持任务筛选、队列、导入导出、日志、校验、播放、DLNA/Chromecast、TVBox、浏览器接管和更新检查。

## 本机位置

- 程序：`%LOCALAPPDATA%\Programs\HLSDownloader`
- Chromium 扩展：`extensions\HLSDownloader-7.0.0-Chromium.zip`
- Firefox 扩展：`extensions\HLSDownloader-7.0.0-Firefox.zip`
- 开始菜单：`HLS Downloader 7.0.0`
- 回滚镜像：`%LOCALAPPDATA%\Programs\HLSDownloader.v7-backup`

验证数据和正式标签前门槛见 `docs/v7-verification.md`。
