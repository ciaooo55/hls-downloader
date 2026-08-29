# HLS Downloader 7.0.0 模块与功能衔接

## 运行时边界

```text
浏览器扩展 (WXT MV3)
        | Native Messaging: bounded JSON
        v
Rust Core + SQLite (唯一状态/凭据/传输所有者)
        | v7 framed JSON over \\.\pipe\HLSDownloader.v7
        +--> Compose Desktop workbench (主工作台)
        +--> native presenter (热确认/进度/完成)
        +--> player child / LAN cast publisher (隔离子进程或临时服务)
```

| 模块 | 负责的功能 | 对外契约 | 不负责 |
| --- | --- | --- | --- |
| `native_shell/src/core_server.rs` + `core_service.rs` | Core 生命周期、SQLite、任务快照/事件、设置、迁移 | `CoreCommand` / `CoreEvent` / `CoreResponse` | UI 渲染、浏览器 DOM |
| `native_shell/src/download_worker.rs` + `http_engine.rs` + `media/` | HTTP/HLS/DASH/FTP/SFTP/BT 下载、恢复、校验 | `create_task`、`task_action`、`refresh_task_request`、进度事件 | 独立数据库、第二调度器 |
| `native_shell/src/native_host.rs` + `native_host_registration.rs` | Native Messaging、浏览器凭据封装、Host 注册 | `ping`、资源识别、handoff/media push | 直接写 UI 状态 |
| `native_shell/src/core_ipc.rs` + `contract.rs` | v7 命名管道、长度帧、版本协商、错误边界 | `hls-downloader-v7-core`、`\\.\pipe\HLSDownloader.v7` | v6 默认启动路径 |
| `desktop_ui/src/main/.../Main.kt` + `Protocol.kt` | 主工作台、任务/队列/设置/播放/投屏操作 | 只通过 Core IPC 读快照、发命令 | SQLite、浏览器 Cookie |
| `presenter_ui/src/hot_main.rs` | 预热确认、进度、完成窗口；Core 重连 | 同一 Core 事件/命令契约 | 主工作台、SQLite |
| `extension/entrypoints` + `extension/lib` | 下载接管、HLS/DASH 识别、媒体发现、用户授权 | Native Messaging 请求/响应；本地浏览器存储 | 传输执行、持久任务状态 |
| `scripts/install-v7-local.ps1` + `upgrade-v7-portable.ps1` | 本机/便携升级、回滚、Host 注册、扩展分发 | `E:\h` 单一安装；桌面每浏览器一个 ZIP | 编译、正式发布门禁 |

## 功能链路

| 用户功能 | 起点 | Core 命令/事件 | 终点 |
| --- | --- | --- | --- |
| 新建与批量导入 | Compose `NewTaskDialog` / drop target；扩展资源识别 | `probe_url` → `create_task` → `task_created` | 队列中的持久任务 |
| 浏览器接管 | 扩展 `background.ts` / `nativeBridge.ts` | Native Messaging → handoff → `task_created` / 错误 | 热确认 Presenter，失败时 Compose 回退 |
| 下载控制 | Compose `TaskTable`；Presenter 快捷操作 | `task_action` → `task_updated` / `task_progress` | Core worker 持续执行，UI 可关闭 |
| 认证恢复 | Compose 详情表单；扩展 request context | `refresh_task_request`，凭据仅在 Core 出站请求使用 | 断点恢复，不进入公共快照 |
| 播放/投屏/TVBox | Compose player/device picker；扩展 media push | `play_task`、`cast_to_device`、`share_media` → session 事件 | 隔离播放器或 LAN 发布地址 |
| 设置与升级 | Compose Settings；本地脚本 | `get_settings` / `set_settings_atomic`；安装脚本注册 Host | 单一 Core 配置与单一 `E:\h` 安装 |

## 当前收敛点

- 功能矩阵为 `26/28 verified`、`2 partial`；partial 只剩外部真实端点和干净 Windows MSI 安装/升级/卸载/回滚证据。
- 本轮修复安装文档与脚本的路径/回滚语义，使交付约束与运行时架构一致。
- 新增功能时先扩展 `contract.rs` 的命令/事件，再接 Core handler，最后接 Compose/Presenter/extension 的单一路径；不要新增第二个状态所有者。
