# HLS Downloader 5.x architecture

> **Frozen.** Product development moved to v6: one resident Rust process, no
> Python/Tauri/WebView2 in-process. See [v6-architecture.md](v6-architecture.md).
> This page remains the 5.x record so the capability matrix can be ported, not
> extended.

5.x 从最后一条稳定 3.x 线继续演进，不以删功能换轻量。架构目标是：下载接管与普通 HTTP 热路径常驻、原生、低延迟；媒体、BT 和跨协议能力继续由已经成熟的核心提供；设置、新建任务和播放器按需加载。

## Product reference and decisions

| 产品 | 借鉴 | 不照搬 |
| --- | --- | --- |
| [IDM dynamic segmentation](https://www.internetdownloadmanager.com/support/segmentation.html) | 动态分段、连接复用、断点位置持久化、下载中直接写最终布局 | 浏览器 DLL 注入、WFP/TDI、封闭协议、把媒体流当普通文件 |
| [AB Download Manager](https://github.com/amir1376/ab-download-manager) | 常驻桌面体验、队列/计划、进度和完成弹窗、统一主题、浏览器扩展 | JBR/Compose 运行时和整仓 Kotlin 重写；其原始 HLS 手动 URL 能力仍有公开缺口 |
| [Free Download Manager](https://www.freedownloadmanager.org/features.htm) | BT、预览、队列、调度、限速和浏览器集成的完整产品面 | 为跨平台牺牲 Windows 热路径和现有媒体识别 |
| [Motrix](https://github.com/agalwood/motrix) / [aria2](https://aria2.github.io/manual/en/html/aria2c.html) | 多协议、任务分类、会话恢复、成熟的分段模型 | Electron 常驻体积；把扩展、媒体归属和 UI 全部交给通用 RPC 引擎 |

结论：5.x 采用 **Rust/Win32 常驻监督进程 + Python 协议核心 + WXT MV3 扩展 + 按需 Tauri UI**。普通 HTTP GET 进入常驻 Rust 进程；HLS、LL-HLS、DASH、直播、BT、FTP/SFTP、投屏和播放器继续保留现有专用实现。

## Process model

```text
Windows 登录 / 用户启动
        │
        ▼
HLSNativeShell.exe                         常驻、小工作集
  托盘 + 单实例
  预创建：确认 / 进度 / 完成 HWND
  按需显示：原生任务列表
  普通 HTTP Range 工作线程
        │  length-prefixed JSON / loopback HTTP
        ▼
HLSDownloaderCore.exe                      能力核心
  FastAPI + SQLite
  HLS / LL-HLS / DASH / live / BT
  FTP / SFTP / POST replay / proxy fallback
  FFmpeg / libtorrent / cast / TVBox
        ▲
        │ Native Messaging + authenticated loopback API
浏览器扩展（WXT MV3，Chrome/Edge/Firefox）
        │
        └── 用户打开设置 / 新建 / 播放器时才启动 HLSDownloader.exe (Tauri/WebView2)
```

### Ownership boundaries

| 组件 | 唯一职责 |
| --- | --- |
| Native shell | 进程监督、托盘、原生热窗、任务列表、普通 HTTP GET、Windows 文件/目录动作 |
| Python core | 任务状态机、持久化、协议选择、媒体时间轴、BT/FTP/SFTP、后处理、更新与本机 API |
| Extension | 捕获候选、证据采集、播放器归属、浏览器下载暂停/放回、Cookie/Referer/重定向上下文 |
| Tauri UI | 复杂设置、新建任务、播放器和需要 React 组件密度的按需界面 |

同一种任务状态只由 Python core 持久化。Native shell 和 UI 都消费同一事件协议，不维护第二份数据库，也不各自发明状态枚举。

## Download engine invariants

### Ordinary HTTP(S)

```text
payload.downloading（创建时固定最终逻辑长度）
  worker 0: Range 0..N       -> seek(0) 写入
  worker 1: Range N+1..M     -> seek(N+1) 写入
  idle worker: 拆最大慢尾巴  -> seek(split) 写入
  checkpoint: 已完成区间 + resource identity + durability barrier
                         │
                         └── 全部覆盖且校验通过后原子 publish 为最终文件名
```

- 不创建 `part1/part2/...` 后再 `cat`，下载期间文件已经处于最终字节布局。
- Windows 使用 NTFS 稀疏预分配；每个 worker 持有独立句柄，按自己的偏移写同一文件。
- 206 必须带有效且匹配的 `Content-Range`；200、压缩表示、ETag/Last-Modified 变化和长度矛盾会关闭 Range 路径。
- 暂停只持久化已经落盘并完成的区间。未完成区间不计入恢复进度，也不发布预分配文件。
- POST、无 Range、全局限速、已有 Python 检查点、代理/TLS 指纹不兼容时走 Python 单连接或现有回退，不重复请求有副作用的 POST。
- 普通 HTTP 热路径允许动态拆分最大剩余区间，连接完成后直接接管慢尾巴，不重新建立一个完整临时文件。

### HLS / DASH / live

媒体分片带时间轴和初始化段，不能套普通 HTTP 字节拼接：

- 纯 MPEG-TS、无 init/discontinuity：本地 concat 输入交给 FFmpeg stream copy。
- fMP4 HLS：生成本地 `ENDLIST` 清单，保留 `EXT-X-MAP`、discontinuity 和时间戳后 mux。
- DASH：分别下载 representation，再按轨道/时间轴 mux；非 DRM 范围内工作。
- live：检查点记录媒体序列、时间轴和已落盘分片，结束后只处理本地结果，不重新下载已完成片段。
- 合并/faststart 是容器处理，不是网络二次下载；UI 必须把“下载完成”和“本地处理中”分成两个阶段显示。

## Recognition contract

识别结果必须携带 `kind + evidence + owner + confidence + replay context`，而不是只凭扩展名返回 URL。

### Accept evidence

- 用户明确点击下载控件或浏览器创建了 DownloadItem。
- `Content-Disposition: attachment`、可信文件 MIME、magnet/metalink、明确媒体清单 MIME。
- 播放中的 `currentSrc`、MSE SourceBuffer/Blob 所有权、fetch/XHR 响应与当前播放器的帧/标签归属。
- HLS/DASH 清单解析成功，variant/representation 可解释，重定向和请求上下文可重放。

### Reject or defer

- 图片、脚本、CSS、API/JSON 页面、登录/OAuth、广告预览和观看页 URL。
- 没有 HTTP 来源归属的 `blob:`，多个播放器之间无法确定 owner 的共享清单。
- EME/DRM、加密密钥不可用或响应上下文不足。
- 仅靠文件名关键词、按钮文案或页面存在 `<video>` 的弱证据。

扩展先轻量记录证据；只有出现 manifest、媒体 MIME、MSE 或用户动作时才激活昂贵面板和深度探测。所有候选使用有界缓存、TTL 和大小上限。

## UI system

### Surfaces

| 层级 | 界面 | 技术 |
| --- | --- | --- |
| 热路径 | 确认、进度、完成、托盘、任务列表 | Win32 原生，启动时创建并隐藏 |
| 工作台 | 设置、新建、批量、详情、播放器 | React 19 + TypeScript + Tauri 2，按需启动 |
| 浏览器 | popup、页面媒体面板、接管提示 | 同一语义 token 和状态文案，WXT MV3 |

### Design rules

- 产品工具而非营销页：高信息密度、系统字体、单一强调色、语义状态色。
- 4/8 px 间距体系；按钮/输入/弹窗共享高度、圆角、焦点环、禁用和 busy 状态。
- 桌面表格使用 tabular figures；大量任务必须虚拟化，筛选/搜索不阻塞主线程。
- 原生窗与 WebView 使用相同术语、动作顺序和状态：确认 → 下载 → 本地处理 → 完成/失败。
- 键盘完整可达：可见焦点、合理 tab 顺序、Esc 关闭非阻塞层、Enter 触发唯一主动作。
- Light/Dark 分别验证 4.5:1 文本对比度；`prefers-reduced-motion` 下不依赖动画表达状态。
- z-index 使用命名层级：toolbar < dropdown < modal-backdrop < modal < toast < tooltip；不散落任意大数。

当前视觉基线在 `DESIGN.md`、`frontend/src/styles.css`、`frontend/src/cockpit-shell.css`。5.x 后续先抽取共享 token/primitive，再逐面迁移，避免一次性重写造成已有功能丢失。

## Resource and latency budgets

以下指标必须在 Windows release 包上测量，开发模式和首次 Defender 扫描不算稳定样本：

| 指标 | 5.x gate |
| --- | --- |
| 热监督进程、核心已就绪：点击到确认窗可见 | P95 < 100 ms |
| 确认窗首帧额外 HTTP 请求 | 0 |
| 空闲热路径 WebView2 进程 | 0 |
| 原生监督进程空闲工作集 | 记录 P50/P95；相同机器上显著低于 3.x 多 WebView 热路径 |
| 普通 HTTP Range 成功后网络二次下载 | 0 bytes |
| 1000 条任务筛选/搜索交互 | P95 < 100 ms，滚动无明显掉帧 |
| 扩展普通页面无媒体时 | 不注入昂贵面板，不轮询核心 |

总内存不和 IDM 的纯原生 C++ 进程做虚假等同：媒体/BT 活跃时 Python、FFmpeg、libtorrent 会按任务加载。衡量重点是空闲常驻、点击热路径和每种任务的增量成本。

## Compatibility and migration

- 3.0.39 是配置/数据库/任务兼容基线；5.x 读取旧 token、目录、任务、队列和检查点。
- 安装包和 Portable 都保留原运行数据，升级过程先准备完整程序树，再原子交换并可回滚。
- Native Messaging manifest 只指向版本化 host；浏览器已连接时升级脚本先有界关闭进程。
- Tauri 保留为按需复杂 UI，直到每个候选原生替代都通过功能矩阵和可访问性验收。

## Release gates

每个 5.x 版本至少通过：

1. Python：ruff、mypy、全量 pytest。
2. Native shell：`cargo fmt --check`、`cargo test --all`、Windows debug/release build。
3. Frontend：Vitest、TypeScript、Vite build、Tauri release build。
4. Extension：TypeScript、Vitest、Chrome/Firefox MV3 build、Firefox `web-ext lint`。
5. Package：PyInstaller core/host、NSIS、Portable、SBOM、版本一致性、包内文件清单。
6. Smoke：隔离端口启动、认证 API、Native Messaging 多消息复用、进程关闭、便携升级/回滚。
7. 行为矩阵：HTTP Range/无 Range/中断、HLS TS/fMP4/live、DASH、BT、FTP/SFTP、投屏、扩展接管与放回。

任何失败都阻止打 tag。GitHub `main` 继续保持可发布 3.x，5.x 在独立分支完成门禁后再合并。

## Roadmap after 5.0.14

1. **5.0.15 稳定化**：固定 Windows 编译/CI、事件循环生命周期、安装/便携 smoke、资源/延迟基准脚本。
2. **5.1 识别评测**：建立带期望 owner/evidence 的站点和协议 fixture，统计 precision、recall、误接管和首次候选延迟。
3. **5.2 HTTP parity**：让 Rust 路径覆盖资源身份检查点、动态慢尾巴、代理/TLS 回退统计；行为与 Python 路径共享契约测试。
4. **5.3 统一 UI**：共享 token、按钮/输入/反馈组件、原生与 WebView 状态文案；主列表虚拟化和键盘/无障碍补齐。
5. **5.4 媒体体验**：候选排序、质量/音轨/字幕解释、直播状态、下载与本地处理分阶段进度，完善扩展页面面板。
6. **5.5 收敛**：测量实际工作集和点击 P95；仅在功能矩阵完全等价后替换更多按需 WebView 页面。
