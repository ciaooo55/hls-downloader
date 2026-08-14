# 技术栈、下载引擎、文件怎么拼成最终文件

对照 IDM 和 AB Download Manager，差在壳的已经在 5.0.0 / 5.0.1 收过一轮。这一页只谈 **技术栈、下载引擎、落盘拼接**。不重写核心语言，也不把 HLS 当成「把分片 cat 成一个文件」。

## 1. 技术栈（诚实对照）

| | IDM | AB Download Manager | HLS Downloader |
| --- | --- | --- | --- |
| 壳 | 很小的 C++ 原生窗 | Kotlin / Compose 原生窗 | 5.0：Rust Win32 监督进程 + 预创建确认/进度/完成；主列表仍是 Tauri/WebView |
| 下载核心 | C++ HTTP/FTP | JVM + OkHttp | **Python 3.12 + httpx + FastAPI** |
| 媒体 | 几乎不管 HLS/DASH 时间轴 | 窄的 m3u8 扫描 | HLS / LL-HLS / 非 DRM DASH + 本地清单 + FFmpeg |
| BT | 无 | 有限/无 | libtorrent |
| 体积 | 十到二十兆量级 | 要 JRE | 核心还在 Python + FFmpeg + libtorrent，**总内存不和 IDM 比** |

保留 Python 引擎是产品选择，不是还没改完。HLS/DASH/直播/BT/接管都在这套核心里；换成 C++ 只为了「看起来像 IDM」会丢掉现在能用的能力。

热路径该抠的是：**Range 写入少开文件、预分配不要在 NTFS 上整文件填零**，不是换语言。

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

- 预分配：`backend/app/downloader/disk_space.py` → `preallocate_payload`
- 多连接写入：`backend/app/downloader/http_file.py`（`Range` + `seek` + 同一 `r+b` 句柄）
- 动态拆尾巴：`backend/app/downloader/http_split.py`
- 检查点：`http-resume.json`（先 `fsync` 载荷，再原子写状态）

服务器后来忽略 Range、If-Range 变成整段 200 时：**丢掉稀疏文件，从 0 单连接重下**。禁止把 200 的 body 接到稀疏偏移上。

无 Range / POST 重放：同一条 `payload.downloading` 上追加，仍然不是多 part 拼接。

5.0.2 相对 IDM 补的两点：

1. **NTFS 稀疏预分配**（`FSCTL_SET_SPARSE` 后再 `truncate`）。以前 `open("wb"); truncate(total)` 在 NTFS 上会物理填零，大文件开始阶段明显慢于 IDM。
2. **每个 Range 分片整段只开一次文件**。CDN 把一次 206 截短时，按下一个字节继续请求，不再每个 206 都 `open/close`。每个 worker 仍用自己的句柄（跨任务共享同一个 fd 再 seek+write 不是原子的）。

5.0.3 继续抠同一条热路径：

3. **落盘不堵事件循环**：256 KiB 一批 `asyncio.to_thread` 写入，网络 worker 不再被 Windows Defender/机械盘的 `write` 卡住。
4. **检查点复用已打开的载荷句柄** `fsync`，不再每 5 秒对几 GB 稀疏文件 `open/close`。
5. **Windows `FILE_FLAG_RANDOM_ACCESS`** 打开 Range 载荷，符合散写而不是顺序拷贝。
6. **`aiter_raw()`** 代替 `aiter_bytes()` 的 ByteChunker，Range 已经拒绝了 Content-Encoding。

HLS/DASH 分片同样把 `write` 移出事件循环。MPEG-TS 仍走 `concatf` + FFmpeg copy；fMP4 仍走本地清单，不改拼接语义。

ABDM 也是 RandomAccessFile / 动态 part 写进一个文件，模型和这里一样；差别在运行时（JVM vs Python）和媒体能力，不在「要不要 cat part」。

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
- 不 C++/Kotlin 重写下载核心
- 不复制 IDM 捕获 DLL、WFP/TDI、注入
- 不把接管范围扩到图片、脚本、登录、OAuth、不相干的 `blob:`
