# 7.0.10 下载交付与安装停机修复

起点为 `b0f68a99`。公开正式版仍为 7.0.9；7.0.10 是本轮候选，`release_ready=false`。本轮沿用一个 `main` 分支和一个工作区，不操作个人桌面、浏览器或安装注册项。

## 已修复的实际路径

| 问题 | 实现与处理 |
| --- | --- |
| 普通“新建”入口的 BT 多文件任务未逐文件交付 | `torrent_engine::transfer_torrent` 返回本次传输采用的元数据；`run_task_with_throttle` 按原种子的单文件/目录结构交付，空选择表示全部文件。单文件种子保持文件成品，多文件格式即使只有一个条目仍交付目录 |
| 交付时再次抓取种子，可能失败或采用另一份文件布局 | Core 直接使用下载返回的 `TorrentMeta`。现有公开 `TorrentSession` 方法仍返回原来的字节数 |
| 选中文件逐个覆盖，失败时留下半套成品 | 在目标文件系统的临时目录中写入并同步全部选中文件，最后一次重命名发布整个目录；途中失败清理本次临时目录。复制期间按有界缓冲检查暂停/取消 |
| 完成边界仍能追加选择，可能发布稀疏洞 | 进入 checking 后拒绝选择变更；写选择与状态复核在同一 Core 锁下执行，最终显式选择按实际 Piece SHA1 复核。未完成的数据明确失败，恢复后重新进入下载调度 |
| 已完成 BT 目录被当成成品丢失；重名成品被重复下载 | 快照接受 Torrent 目录成品，并关闭目录的播放就绪标记；重复任务裁决读取实际持久成品路径 |
| 安装助手仅知道默认或继承的管道 | 自定义管道的 Core 在正常 IPC 就绪后建立按 PID 命名的维护管道，复用现有 owner DACL，仅接受 Shutdown；不写额外端点记录文件 |
| 核验管道归属后另建连接发命令，存在端点切换窗口 | 助手保留进程句柄、复核进程路径，使用系统 API 核验当前管道的真实服务端 PID，在同一连接发送 Shutdown 并读取回执；支持同目录多个 Core，先核验全部端点再停机 |
| 助手与 Core 都只给八秒，可能抢先取消正常检查点 | Core 保持八秒，助手给十二秒完成请求和进程退出；异常、归属不符、超时均停止安装，不强杀 Core |

目录冲突采用明确的数据保全行为：skip 拒绝已存在目标，其余策略给整套 BT 成品另取名称，不合并或覆盖已有目录中的用户文件。单文件任务继续沿用原有 overwrite/rename/skip 行为。旧任务不需要数据库迁移。

BT 是 HLS 的额外能力，本批修复不冒充来自 IDM 原函数。安装停机依据项目真实协议和系统 API 实现；未采用读取其他进程 PEB 固定偏移的方案，也未改变浏览器请求凭据或持久回执合同。

Windows API 依据：[管道服务端进程身份](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeserverprocessid)、[管道安全](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)、[管道等待模式](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-type-read-and-wait-modes)。

## 验证

- 本机 Core 全套：752 通过、17 按既有条件忽略、0 失败。忽略的 Sandbox/真实 FFmpeg 等场景不算通过。
- 插件：WXT 类型生成、TypeScript 检查和 56 文件/630 测试通过。本批没有调整插件业务逻辑或 UI。
- 停机助手：独立 Rust 编译及两个安装边界单测通过。
- 候选版本合同：7.0.10 全组件同步，严格升级版本检查通过。
- 最终 BT 定向、Core 实际入口构建、Clippy、无界面停机集成及候选包验收仍需以本轮后续实际运行结果为准。

新增默认 CreateTask 回归检查单文件、多个文件、单条目的多文件格式、同名目录保全、实际文件字节、数据库重开，以及交付阶段不重复拉取种子。原有运行中选择变更和对抗路径回归继续保留。

`smoke_v7_installer_shutdown.py` 在工作区的独立安装副本上启动无托盘 Core；助手故意继承另一安装的管道，必须仅停止目标安装，并在重开 Core 后读回原暂停任务。

候选流程新增真实 MSI repair：驻留 Core 的私有管道不传给安装器，普通请求不能通过维护端点，安装修复须取得正常退出，并恢复已有任务到成品 SHA256 一致。此操作仅允许隔离 GitHub Windows runner。原有 MSI 升级/失败回滚、EXE 安装及 Edge/Firefox 冷启动门禁继续执行。

本轮候选与各报告由 workflow 的 source SHA、产物 manifest 和文件哈希关联。完整 Core/插件自测不能代替候选安装、真实浏览器、原生 UI 或 IDM 同条件性能对照；未执行项不写成已通过。
