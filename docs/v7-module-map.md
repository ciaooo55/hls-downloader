# HLS Downloader 7.0.3 模块与功能衔接

更新于 2026-10-18。当前交付为 r12 未签名预发布版；验收范围见 [验证索引](v7-verification.md)。历史版本的固定安装路径和独立播放器说明不再描述本版。

| 板块 | 主要实现 | 当前行为 |
| --- | --- | --- |
| 任务与持久状态 | `native_shell/src/core_service.rs`、`store.rs`、`contract.rs` | Rust Core 持有任务、设置和凭据；桌面及插件通过 IPC 发命令、读事件 |
| 下载与恢复 | `download_worker.rs`、`http_engine.rs`、`media/` | HTTP、HLS、DASH、FTP/FTPS、SFTP、BT；暂停/续传、重试、计划、校验和、请求刷新 |
| 下载缓存 | `download_worker.rs` 的完成和任务删除路径 | 成品发布和字幕处理后按保留临时文件设置清理；失败/暂停保留续传数据；删除任务按操作处理缓存和成品 |
| 主工作台 | `desktop_ui/src/main/kotlin/com/hlsdownloader/desktop/Main.kt`、`Protocol.kt` | 新建、批量、抓取、队列、搜索筛选、详情、选择操作和错误反馈；设置通过 Core 持久化 |
| 内置播放器 | 同目录 `VideoPlayer.kt`；Core player 会话与媒体服务 | 主窗口内浮层、时间轴、缓冲、音量、倍速、音轨/字幕、全屏/画中画；下载中播放，完成后原会话切换成品 |
| 浏览器插件 | `extension/entrypoints`、`extension/lib` | 媒体发现、清晰度选择、下载接管、复制、字幕设置及推送；实际传输交给 Core |
| Native Messaging | `native_host.rs`、`native_host_registration.rs` | 浏览器与 Core 之间的有界消息及精确 Host 注册 |
| 确认/进度/完成窗口 | `presenter_ui/src/hot_main.rs` | 独立提示窗口，通过同一 Core 获取任务和发送操作 |
| 字幕 | Core 字幕链路、插件字幕入口 | 音频预取和翻译状态；本轮没有上传音频或重新验收真实云端服务 |
| 投屏与 TVBox | Core 媒体推送和控制链路 | 独立控制用途；实体设备仍在本轮验收排除范围 |
| 安装与交付 | MSI、`scripts` 的打包及生命周期工具 | 安装可选路径；新配置默认 Windows Known Folder Downloads，支持被用户迁移的下载文件夹；未签名包手动升级 |

主窗口关闭和传输生命周期由不同进程负责，关闭播放器不会删除任务。任务删除、删除任务及文件、筛选清理的范围不同，操作按界面确认执行。安装目录大小主要来自 FFmpeg、libmpv 和裁剪 JRE，不等同于常驻内存。

当前 canonical 功能合同为 `artifacts/v7-productization/feature-parity.json`：29 项中 27 verified、2 partial，`release_ready=true` 带有其自身排除条件。该合同不等于本轮所有板块都重新完成发布级认证；r12 包的 `BUILD-PROVENANCE.json` 明确为 `release_ready=false`、未签名预发布。
