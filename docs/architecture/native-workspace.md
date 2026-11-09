# 原生 workspace 与 IPC

实施计划见 [原生化计划](native-migration-plan.md)，实际验证见 [实施记录](../validation/native-migration-progress.md)。

| 成员 | 责任与依赖 |
|---|---|
| `crates/protocol` | v7 消息、任务与抓站类型、4 字节小端 JSON 编解码；仅依赖 serde |
| `crates/client` | 管道传输、连接、启动、实例锁、配置路径、DPAPI、窗口、文件剪贴板/OLE 和 MSI 安装辅助；依赖协议 |
| `native_shell` | 唯一生产 SQLite、状态机、引擎、调度和后台媒体业务；依赖协议与客户端 |
| `native_clients` | Host、CLI、Updater；生产依赖中没有 Core、SQLite 或传输引擎 |
| `native_ui` | 从原 Presenter 移入的 Slint 窗口；仅通过 IPC 操作 Core |

协议和客户端实现只保留一份。Core 内部沿用原模块引用，由共享模块重导出提供类型与实现。

Host 的已有回归使用 Core 私有的内存数据库夹具，由 Core 单元测试入口编译同一份 Host 源码。此测试关系不进入 Host 的生产依赖图。CLI 和 Updater 的入口回归在 `native_clients` 运行。

根 `Cargo.lock` 合并原两套锁定图；兼容版本统一记录在 `artifacts/native-migration/lock-reconciliation.json`。Slint 保持 1.17.1。Release 配置集中在根清单，保留 Core 的 unwind 语义。

```powershell
cargo test --locked -p hls-protocol -p hls-client -p hls-native-clients -- --test-threads=1
cargo test --locked -p hls-native-shell --all-targets -- --test-threads=1
cargo test --locked -p hls-native-ui
./scripts/validation/verify-native-boundaries.ps1
```

## 分页和事件恢复

保留 v7 协议版本、默认管道名、4 MiB 帧上限和旧 Snapshot/WaitEvents。新能力由 Capabilities 中的 `list_tasks_paged`、`event_batch_resync` 协商。

- `ListTasks`：按稳定 taskId 游标读取，默认 128 项，上限 512 项，同时限制编码后大小；不先复制全部历史任务。
- `TaskPage`：包含任务、下一游标和该页的事件序号。游标所在任务被删除后仍可继续读取。
- `WaitEventBatch`：返回有界事件及 `resync_required`。历史环覆盖、Core 恢复后历史不足、客户端序号超前时，明确要求重读快照。
- 轻客户端按页序号合并扫描期间的创建、更新和删除，旧事件不能覆盖较新的页数据。历史不足时最多重新扫描三次，再向调用方报告需重同步。
- 旧 WaitEvents 遇到历史缺口返回 `resync_required` 错误；既有 Presenter 和 Compose 的断连处理会重新加载快照。
- 单项无法装入帧时返回错误，不截断 JSON。

## 持久接收

数据库仍使用 schema 7、WAL 与 NORMAL 作为常规进度提交策略。凭据、交接、任务创建、控制事件及终态提交在同一 Core 锁内、事务外切换到 FULL，完成事务后恢复原等级。恢复等级失败时保留较强同步，并记录诊断；已成功提交的数据不能因此被误报为失败并回滚内存。

生产运行时、数据库和用户设置路径保持原有规则。Compose/JRE 的删除须等待完整原生工作台及安装升级验收完成。

## 播放与投屏合同

- `PlayerEmbed` 传递实际 HWND、物理客户区坐标、媒体会话代数和布局代数；旧布局或旧会话请求被拒绝。
- `PlayerControl` 请求使用媒体会话代数；`CastControl` 使用独立投屏代数，分别通过 `player_control_generation`、`cast_control_generation` 协商。旧命令入口供迁移期调用方使用。
- 投屏源替换、挂载切换、控制及事件发布使用投屏操作锁；该锁独立于 Core 状态锁和本机播放器。接收端网络操作不能占用 Core 状态锁。
- UI 的投屏搜索/接收端请求和缩略图各用独立、有界 IPC 工作队列。设备搜索按操作启动；会话状态只在可见页面轮询。
- 浏览器媒体推送从既有交接表恢复，保留原请求 ID；传输失败不自动重放投送命令。持久裁决回执到达后才移除当前请求。
