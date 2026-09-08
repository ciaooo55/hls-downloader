# HLS Downloader 7.0.2 升级说明

> 当前活动源码版本为 `7.0.2`。上一轮 `v7.0.1-candidate.1` 公开测试包仍可作为历史候选参考，但不是当前 `main` 的产物，也不代表 v7.0.2 已正式发布。当前正式发布状态以 canonical `feature-parity.json` 与 formal-release readiness 文档为准；现为 `release_ready=false`。

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

- 程序：`E:\h`
- 浏览器扩展：`extensions\HLSDownloader-<manifest-version>-Chromium.zip` 与 `extensions\HLSDownloader-<manifest-version>-Firefox.zip`
- 开始菜单：由当前 candidate/formal manifest 的产品版本生成
- 回滚镜像：`E:\h.v7-backup`（仅在事务失败恢复期间短暂存在）

不要从历史文档中的 `7.0.1` 文件名推断当前安装版本；候选/正式包的产品版本必须与 `ARTIFACT-MANIFEST.json` 和 canonical feature-parity 元数据一致。

历史验证数据见 `docs/v7-verification.md`；当前正式发布门槛见 `docs/architecture/formal-release-readiness.md` 与 `docs/v7-release-runner.md`。
