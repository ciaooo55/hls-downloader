# 技术栈、下载引擎、文件怎么拼成最终文件

对照 IDM 和 AB Download Manager，这一页谈 **技术栈、下载引擎、落盘拼接**。普通 HTTP GET 跑在已经常驻的 `HLSNativeShell.exe` 里（事件队列进线程，Windows 走系统 WinHTTP）。`--job` 只在监督进程没在听事件时当后备。不再多带一个引擎 exe，也不再链 rustls/icu。HLS/DASH/BT 仍留在 Python 核心。

## 1. 技术栈（诚实对照）

| | IDM | AB Download Manager | HLS Downloader 5.0.13 |
| --- | --- | --- | --- |
| 壳 | 很小的 C++ 原生窗 | Kotlin / Compose 原生窗 | **空闲时只有 `HLSNativeShell.exe`：托盘 + 预创建确认/进度/完成 + 原生任务列表。WebView2 只在设置/新建/播放器时启动** |
| 普通 HTTP 文件 | 已运行进程里传 | OkHttp，同一 JVM | **已运行的监督进程：Range seek 写入一个 `payload.downloading`；https 用 WinHTTP** |
| 媒体 | 几乎不管 HLS/DASH 时间轴 | 窄的 m3u8 扫描 | HLS / LL-HLS / 非 DRM DASH + 本地清单 + FFmpeg |
| BT | 无 | 有限/无 | libtorrent |
| 捕获 | 驱动/注入级挂钩 | 扩展 `webRequest` / `downloads.onCreated` | 扩展接管；不复制 IDM 的 DLL/WFP/注入 |
| 体积 | 十到二十兆量级 | 要 JRE | 空闲热路径不再加载 WebView2；Python/FFmpeg/libtorrent 还在，**有媒体任务时总内存不和 IDM 比** |

要和 IDM/ABDM **技术栈持平**，指的是：确认窗是已在跑的原生进程、普通文件用编译运行时按 Range 写入一个文件，并且传输发生在这份已运行的进程里。不是再叠一个引擎进程，也不是去抄挂钩。

5.0.7 曾单独打包 `HLSNativeEngine.exe`（ureq + rustls）。5.0.8 把它收进同一个二进制的 `--job`。5.0.9 不再为每次 GET fork 一份 `--job`：监督进程在听事件时，任务进同一条队列，线程里跑 `run_job`。

回退到 Python 的情况：POST 重放、全局限速、已有 Range 检查点续传、监督进程不在或 `--job` 失败、浏览器 TLS 指纹回退。

## 2. HTTP：和 IDM 同一类拼接（单文件 seek，不是 cat）

普通文件下载 **不是** 下完 `part1`、`part2` 再拼接。

```
payload.downloading     ← 一开始就按最终逻辑长度建好
        ▲
        │  worker A:  Range bytes=0-1048575     → seek(0) 写入
        │  worker B:  Range bytes=1048576-...   → seek(1048576) 写入
        │  慢尾巴切开后再 Range 写入同一文件
        ▼
publish_path() 重命名/挪到最终文件名
```

对应代码：

- 原生 GET 热路径：常驻 `HLSNativeShell.exe` 的 `http_job` 事件（`native_shell/src/http_engine.rs`），探测完成后由 `HTTPDownloader.run()` 入队；https 走 WinHTTP。监督进程没在听时才 spawn `--job`
- Python 回退：`backend/app/downloader/http_file.py`（同一条 `payload.downloading`）
- 预分配：`backend/app/downloader/disk_space.py` → `preallocate_payload`
- 动态拆尾巴（Python 回退）：`backend/app/downloader/http_split.py`
- 检查点：`http-resume.json`（先 `fsync` 载荷，再原子写状态）

服务器后来忽略 Range、If-Range 变成整段 200 时：**丢掉稀疏文件，从 0 单连接重下**。禁止把 200 的 body 接到稀疏偏移上。原生引擎用退出码 30 表示这种情况。

无 Range / POST 重放：同一条 `payload.downloading` 上追加，仍然不是多 part 拼接。

5.0.2 相对 IDM 补的两点（Python 回退路径仍保留）：

1. **NTFS 稀疏预分配**（`FSCTL_SET_SPARSE` 后再 `truncate`）。以前 `open("wb"); truncate(total)` 在 NTFS 上会物理填零，大文件开始阶段明显慢于 IDM。
2. **每个 Range 分片整段只开一次文件**。CDN 把一次 206 截短时，按下一个字节继续请求，不再每个 206 都 `open/close`。每个 worker 仍用自己的句柄（跨任务共享同一个 fd 再 seek+write 不是原子的）。

5.0.3 继续抠同一条热路径：

3. **Range 落盘不堵事件循环**：256 KiB 一批 `asyncio.to_thread` 写入，多连接时网络 worker 不再被磁盘 `write` 卡住。
4. **检查点复用已打开的载荷句柄** `flush` + `fsync`，不再每 5 秒对几 GB 稀疏文件 `open/close`。
5. **真正的流式 Range 走 `aiter_raw()`**；已经读进内存的响应（测试 Mock 等）仍走 `aiter_bytes()`。

5.0.4 撤回 5.0.3 里看走眼的两处：

- 不在热路径上调用从未在 CI 跑过的 Windows `CreateFile` / `FILE_FLAG_RANDOM_ACCESS`。每个 worker 句柄是 seek 一次再顺序写，RANDOM_ACCESS 提示是反的。
- 无 Range 单连接改回随到随写（`buffering=0`）。5.0.3 误把 `RANGE_WRITE_BATCH` 套到单连接上，崩溃/暂停会丢掉最多 256 KiB 已收到但未落盘的前缀。多连接 Range 本来就是 256 KiB 一批，窗口没变。

HLS/DASH 分片同样把 `write` 移出事件循环。MPEG-TS 仍走 `concatf` + FFmpeg copy；fMP4 仍走本地清单，不改拼接语义。

ABDM 也是 RandomAccessFile / 动态 part 写进一个文件，模型和这里一样。普通 GET 的运行时在已经常驻的监督进程里，不再为每次传输再开一份 `--job`。

## 3. HLS / DASH：不能按 IDM 的字节拼接

媒体分片 **不是** 普通文件的 Range 块。

| 流类型 | 错误做法 | 这里的做法 |
| --- | --- | --- |
| 纯 MPEG-TS，无 init、无 DISCONTINUITY | 可以本地按 TS 拼 | FFmpeg `concatf:` 快速本地拼接（`merge.py`） |
| fMP4 / `init.mp4` + `.m4s` | `cat` 成「一个 MP4」 | 生成带 `EXT-X-MAP` / `DISCONTINUITY` 的本地 `ENDLIST` 清单，FFmpeg 按播放器时间轴 copy；`tfdt` 把时长撑爆时再重编码 |
| DASH 非 DRM | 把 Representation 字节接起来 | 先收成轨道，再走同一套本地 HLS 时间轴 + mux |
| 加密 / 多音轨 / 直播检查点 | 当普通文件 | 现有 HLS/LL-HLS/DASH worker，不绕过 DRM |

`init.mp4 + fragment.m4s` 每个后续片段还带着绝对 `tfdt`。FFmpeg concat demuxer 会把那个时间戳当成片段时长，文件越拼越「超长」。这不是性能问题，是容器语义。

## 4. 明确不抄

- 不把 HTTP 改回多 part 再 `copy /b`
- 不把 fMP4 当字节流 cat
- 不 C++/Kotlin 重写 HLS/DASH/BT（那会丢掉现在能用的能力）
- 不复制 IDM 捕获 DLL、WFP/TDI、注入
- 不把接管范围扩到图片、脚本、登录、OAuth、不相干的 `blob:`

## 5. 现在还剩什么

普通 GET 文件已经用编译引擎走 IDM/ABDM 那一类写入。下面这些不是「再换一次 HTTP 语言」就能抹平的。

| 还在 | 为什么还在 |
| --- | --- |
| 设置 / 新建 / 播放器仍是 Tauri | 空闲不再创建 WebView2；点「设置/新建」才启动 `HLSDownloader.exe --settings` |
| HLS/DASH 收尾仍走 FFmpeg，`+faststart` 会再写一遍 MP4 | 本地点播/Range 播放需要 moov 在文件头；MPEG-TS 也要转成播放器常用的 MP4。fMP4 不能字节拼接 |
| Python + FFmpeg + libtorrent 的体积 | 媒体/BT 还在这套核心里；总内存到不了 IDM 那档 |
| FTP/SFTP 仍是单连接追加 | 本来就不是 HTTP Range 那套；传输已经在独立线程里写盘 |
| NTFS 稀疏 `DeviceIoControl` 只在 Windows 生效 | 失败就退回 `truncate`，不会把文件打坏；Linux CI 覆盖不到这条 ioctl |
| 多连接 Range 崩溃窗口仍是 256 KiB | 为了少 syscall，有意保留。单连接已经随到随写 |
| 浏览器 POST 重放不能 Range 续传 | 只下一遍，避免表单/API 副作用；5.0.5 只把写盘改成和无 Range GET 一样随到随写 |
| 已有 `http-resume.json` 的 Range 续传仍走 Python | 原生引擎这一刀先覆盖新任务；检查点格式对齐后可以再迁 |

5.0.10 把安装入口改成常驻 `HLSNativeShell.exe`，空闲不创建 WebView2。5.0.13 把普通文件接管收回热路径：zip/exe/pdf 不靠播放点击也能 offer。HLS/DASH/BT 仍在 Python 核心。下一刀若还做产品，是设置/播放器原生化，或把 Range 续传检查点交给原生引擎。
