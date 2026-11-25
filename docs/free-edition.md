# Windows 免费版高级工具指南

HLS Downloader 完全免费开源，无需注册产品账号、开通会员或输入激活码。网站自身的登录态、Cookie、Referer 与自定义请求头通过站点规则或单任务请求上下文在本地安全管理。

## 命令行工具（CLI）

安装版与便携版的 `app/resources/HLSDownloaderCli.exe` 提供完整的无界面命令行入口。首次调用会自动在后台启动同目录的 Rust Core（不弹出主界面窗口），后续调用复用同一个 Core 实例，任务队列、断点状态与全局设置与桌面工作台实时共享。

```powershell
$cli = 'C:\Program Files\HLSDownloader\app\resources\HLSDownloaderCli.exe'
& $cli --help
& $cli --json add 'https://example.com/file.zip' --dir 'D:\下载 文件' --name '文件.zip' --paused
& $cli --json list
& $cli show task-1
& $cli resume task-1
& $cli pause task-1
& $cli retry task-1
& $cli queue stop default
& $cli queue start default
& $cli export --format json --output '.\tasks.json'
& $cli import '.\tasks.json'
& $cli remove task-1
```

请将示例路径与任务 ID 替换为实际值。常用行为约定：

- `--paused` 原子创建暂停态任务；不指定时直接交由目标队列调度下载；流媒体任务可通过 `--variant` 指定清晰度变体。
- `remove` 默认仅移除任务记录并保留已下载成品文件；显式传入 `--delete-files` 时才会一并删除本地成品。
- `export` 支持 `json`、`csv`、`urls` 三种格式，目标文件已存在时拒绝覆盖以防误删；`import` 兼容工作台导出的任务列表文件。
- 标准输出仅打印结果（加 `--json` 输出结构化 JSON），诊断信息输出至标准错误。退出码：`0` 成功、`2` 参数错误、`3` 无法连接或启动 Core、`4` 业务拒绝或操作失败、`5` 本地文件读写错误。`add` 返回成功代表任务已被 Core 接收，可通过 `show` 或 `list` 查询完成状态。

包含 Cookie、令牌或预签名 URL 的敏感请求上下文建议通过文件或标准输入传入，避免留在命令行历史中：

```powershell
Get-Content -Raw -Encoding UTF8 '.\request-context.json' | & $cli --json add --context - --paused
```

上下文 JSON 支持 `url`、`headers`、`request_method`、`body_path`（上限 1 MiB）。常规任务列表查询默认隐去完整敏感请求头；导出的任务文件与抓站结果包含资源 URL，请妥善保管。

## 抓站与同步项目

桌面工作台“工具 → 抓站与同步项目”支持站点资源发现、筛选入队、静态离线化及周期增量检查。已确认加入下载的条目会作为标准下载任务进入主任务队列统一管理。

默认限制为同源抓取、最大深度 2、最多 500 页、页面并发 2；达到边界后自动暂停并提示原因。发现阶段仅解析链接，由用户勾选所需资源或确认全部入队后才真正建立下载任务。

也可通过 CLI 与 UTF-8 JSON 配置文件管理抓站项目：

```json
{
  "entry_url": "https://example.com/docs/",
  "output_dir": "D:\\下载",
  "queue_id": "default",
  "same_origin": true,
  "max_depth": 2,
  "max_pages": 500,
  "page_concurrency": 2,
  "extensions": [],
  "include_paths": ["/docs/"],
  "exclude_paths": ["/docs/private/"],
  "offline": true,
  "sync_interval_secs": 0
}
```

```powershell
& $cli crawl create --spec '.\crawl-project.json'
& $cli crawl list
& $cli crawl get PROJECT_ID
& $cli crawl enqueue PROJECT_ID
& $cli crawl pause PROJECT_ID
& $cli crawl resume PROJECT_ID
& $cli crawl sync PROJECT_ID
```

每个抓站项目在输出目录下使用独立子目录并以稳定文件名落盘；项目配置、待抓取队列与同步状态持久化在 Core 数据库中，重启后可无缝恢复（上限 32 个项目、单项目最多 2,000 个资源、单份 HTML/CSS 上限 1 MiB、累计页面元数据上限 32 MiB）。

开启 `offline` 时支持重写静态 UTF-8 HTML/CSS 本地引用（不执行动态网页脚本）。`sync_interval_secs` 设为 `0` 表示仅手动触发检查，非零最小间隔为 60 秒。增量同步通过 `ETag` / `Last-Modified` 比对远端变化：未变化资源不重复拉取，更新资源先写入暂存文件校验通过后再原子替换旧文件，下载失败或远端移除不会破坏本地已有文件。

## 远程与本地 ZIP 内容预览

桌面工作台“工具 → ZIP 内容预览”与 CLI 均支持在不下载整包、不解压文件的前提下直接浏览本地路径或远程 HTTP(S) ZIP 归档目录结构：

```powershell
& $cli zip 'D:\下载\文件.zip'
& $cli --json zip 'https://example.com/file.zip'
```

- 展示条目路径、压缩前后大小、压缩算法与加密标记，支持 ZIP64、UTF-8 文件名、Unicode 路径扩展字段及传统 CP437 编码文件名，不会向磁盘解压或执行任何归档内容。
- 远程预览利用 HTTP `Range` 按需读取归档尾部与中央目录（最多读取 4 MiB 元数据、最多列出 10,000 个条目）。若目标服务器不支持 `Range` 请求，工具会明确提示需先完整下载文件，而不会静默消耗全量流量拉取整包。

## 浏览器接管与恢复

- **直连加速与流媒体合流**：普通 HTTP/HTTPS 文件链接、HLS（`.m3u8`）、DASH（`.mpd`）以及音视频分离双轨流（如 `.m4s`）由浏览器扩展通过 Native Messaging 直接移交 Rust Core，享受多线程分段加速、动态慢尾段切分、断点续传（含过期签名链接刷新续传）与 FFmpeg 无损 `.mp4` 封装。
- **一次性响应保全**：一次性 `POST` 表单下载与网页生成的 `Blob` / `data:` 导出文件由浏览器完成数据接收后移交桌面端导入，避免重复提交请求；导入或确认取消时始终保留原文件以防数据丢失。
- **灵活控制**：支持确认弹窗与一键“静默快速下载”两种接管方式；按住 `Alt` 点击链接、关闭自动接管开关或命中站点排除规则时，下载将保留在浏览器中正常进行。
