# Windows 免费版新增功能

本版本不需要产品账号、会员或激活。网站自身的登录、Cookie、Referer 和自定义请求头仍由原有站点规则与请求上下文处理。

## 命令行

安装包和便携包的 `app/resources/HLSDownloaderCli.exe` 是命令行入口。首次调用会启动同目录的 Core，不启动工作台；后续调用连接同一个 Core。任务状态、队列和设置与工作台共用。

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

将示例路径和任务 ID 替换为实际值。`--paused` 原子地创建暂停任务；不指定时交由队列调度。`remove` 保留成品，只有显式加 `--delete-files` 才删除文件。导出文件已存在时返回错误，不覆盖。`export` 支持 `json`、`csv`、`urls`；导入沿用工作台现有任务文件格式。

标准输出为结果，标准错误为诊断。`--json` 提供机器可读结果。退出码：0 成功、2 参数错误、3 无法连接或启动 Core、4 业务拒绝或操作失败、5 本地文件读写错误。成功添加任务只表示 Core 接受任务，不表示文件已下载完成；用 `show` 或 `list` 查询最终状态。

带敏感信息的请求上下文应经文件或标准输入传入，不把 Cookie、令牌或带签名 URL 放进命令行：

```powershell
Get-Content -Raw -Encoding UTF8 '.\request-context.json' | & $cli --json add --context - --paused
```

上下文 JSON 支持 `url`、`headers`、`request_method`、`body_path`，最大 1 MiB；不要把该文件加入版本控制。普通任务查询不输出 URL 或请求头；显式任务导出、抓站发现结果包含资源地址，应按其内容保管。

## 抓站与同步

工作台“工具 → 抓站与同步项目”提供新建、预览、选择下载、暂停发现与同步检查、继续和立即检查更新。已加入的普通下载仍在任务列表暂停、继续或重试。

默认同源、深度 2、最多 500 页、页面请求并发 2。达到边界后暂停并显示原因；调整范围应新建项目。跨域跳转也受同源边界限制。默认只发现资源，确认下载所选或全部发现后才建立普通下载任务。

也可用 CLI 创建 UTF-8 JSON 项目文件：

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

输出目录下创建独立项目子目录；文件使用稳定名称避免路径冲突。项目、待访问集合和同步状态保存在 Core 数据库，重启可恢复。限制为 32 个项目、每项目 2000 个资源、单个 HTML/CSS 1 MiB、页面元数据累计 32 MiB。

静态离线支持 UTF-8 HTML、CSS 和静态引用，成功保存后改写本地引用；不执行网页脚本，不承诺复现动态网站。同步周期为 0 时关闭自动周期检查，非零最少 60 秒。使用 ETag/Last-Modified 检查资源；未变化不重复下载。更新下载先写暂存文件，验证成功后替换旧文件，失败保留旧文件；远端删除不会删除本地文件。下载失败后可在任务列表重试，再立即检查更新。

## ZIP 内容预览

工作台“工具 → ZIP 内容预览”接受本地路径或 HTTP(S) 地址；CLI 对应：

```powershell
& $cli zip 'D:\下载\文件.zip'
& $cli --json zip 'https://example.com/file.zip'
```

预览显示路径、压缩前后大小、压缩方法和加密标记，支持 ZIP64、UTF-8 名称、Unicode 路径附加字段及传统 CP437 名称。不会解压、执行或提取单个条目。

远程预览需要 Range，优先读取尾部与中央目录；跨多次请求还需要可靠资源标识。最多读取 4 MiB 元数据、展示 10000 个条目，超过限制明确报错。不支持 Range 时提示先完整下载，不自动退回整包下载。损坏、分卷或加密目录明确报错，不显示成空归档。

## 浏览器交接与恢复

明确点击链接的入口可提前交给 Core。浏览器已经发出的 POST、Blob、一次性地址等响应由浏览器保全，扩展弹窗显示接收进度，完成后等待确认或导入。接收中断应在浏览器恢复；拒绝、取消、离线或导入失败时保留可恢复源文件。浏览器创建下载项的行为取决于入口，清除记录不等于从未创建。

## 验证状态

实现、编译通过与候选包验收是不同状态。当前证据及未完成门槛见 [当前记录](validation/convergence.md)。活动慢尾段拆分仍由实验开关控制，未取得重复正确性及收益证据前不进入默认行为。
