# 全功能对标与根因修复（2026-10-27）

本轮基点 `9711a8c`，一个 main、一个 worktree。产品仅在 Windows Sandbox 运行；不调用收费字幕云端，不验收实体投屏设备，不签名。本文随实际结果更新，不能用“代码存在”或历史 verified 当作本轮通过。

## 已确认问题与处理

| 问题 | 根因与修复 | 证据状态 |
| --- | --- | --- |
| POST 任务列表不能安全重建 | JSON 导出方法却不带正文，导入创建缺少正文的请求。JSON 导入/导出明确拒绝 POST；CSV 仍可作记录，完整请求从浏览器或 cURL 获取 | 旧代码沙盒回归失败；修后 Core 回归通过 |
| 异机命名队列导致导入 0 项 | 列表携带 queue_id、不携带定义，新电脑直接拒绝。仅导入时将不存在的队列映射到默认队列 | 旧代码实际创建 0 项；修后 Core 回归通过；待最终成品 IPC 验收 |
| 导出覆盖原文件时可能损坏旧列表 | 直接 Files.writeString。改为同目录暂存、原子替换，失败清理暂存并保留原目标 | 桌面定向测试通过，待最终成品验收 |
| 正常后台启动难以取得 Core 诊断 | 部分错误仅 stderr，启动器丢弃 stderr。新增有界异步状态日志，记录任务阶段/错误码/HTTP状态/尝试次数及播放器通信操作耗时，不复制请求上下文 | 轮转/故障回归通过；待最终成品日志检查 |
| 桌面日志包含原始异常中的请求数据 | 日志只清理换行。隐藏地址与凭据字段，轮转在写入前计入新记录字节；空闲写线程阻塞等待，无每秒轮询 | 脱敏定向测试通过 |
| FFmpeg 错误日志无大小上限 | 子进程直接写文件。持续排空 stderr，单任务日志最多1MiB，保留末尾错误；写盘失败仍排空管道 | 新增定向回归，待最终合流/暂停取消验收 |
| 模块文档与实际门禁冲突 | 仍写 r12、release_ready=true。修正文档，历史证据保留适用版本 | 静态检查 |

Core 运行日志最多4×5MiB，桌面日志最多2×2MiB，合计不超过24MiB；数据库任务日志每任务500条。合流日志位于任务缓存中，每任务最多1MiB，遵循既有任务缓存保留/清理策略。运行诊断队列有界、拥塞可丢弃，不能替代持久任务状态。

## 入口与分层验收

[inventory.json](inventory.json)从实际命令枚举、调用引用、桌面控件标识和设置页生成；只作逐入口导航，不据文本匹配推断测试覆盖。

| 功能组 | 实现和现有验收入口 | 本轮判断及下一道门槛 |
| --- | --- | --- |
| HTTP、续传、镜像、限速 | http_engine、download_worker；HTTP boundaries/stall、curl/WinHTTP 回归 | 首批实际 Core HTTP 边界通过；最终版本补传输中止与慢请求 |
| HLS/DASH、直播、合流 | media、download_worker；媒体 harness、download_playback、合流回归 | Core 媒体回归通过；最终版本须核对提前播放、音轨、跳转和缓存；原始卡死现场与指定直播403未闭环 |
| FTP/FTPS/SFTP | ftp_engine、sftp_engine/sftp_live；现有协议回归 | 本轮全 Core 中的协议场景与真实公网账号兼容性分别记录 |
| BT/Magnet、多文件 | torrent_engine、ProbeTorrent/SelectTorrentFiles/SetTaskTorrentFiles | 本轮 Core 回归；完整桌面选择操作仍需实屏 |
| 新建、批量、抓取、列表导入导出 | CreateTask/ImportPaths/ExportTasks/HarvestPage | 首批千任务、混合失败下载通过；最终成品核对新修复及导出保护 |
| 队列、计划、重试、配额、完成动作 | CoreRuntime、net_policy、队列配置 | Core 回归通过不代替全部计划时间及系统完成动作实测 |
| 进度、详情、日志、文件操作 | TaskProgress/GetTaskLog/OpenCompleted/TaskAction | 需以最终桌面实际操作验证；API 结果不算按钮验收 |
| 任务缓存生命周期 | download_worker 完成/删除、播放器释放 | Core 回归通过；最终媒体成品补缓存验证；不动宿主用户下载 |
| 本地播放器、边下边播 | Player、playback、VideoPlayer | 以 v3 行为为本项目标准；偶发超时仍需现场证据，不直接增加超时 |
| 下载接管与回退 | background、takeover、Native Host | Firefox/Chromium按入口分类；观测下载创建事件、请求和成品哈希，后台擦记录不算零项 |
| 媒体悬浮与插件菜单 | content/background/popup | 现有418插件回归及双构建通过；最终双播放器/重载/菜单仍需实际浏览器 |
| 字幕配置、预取和时间轴 | 主程序SettingsV7、字幕Core、网页会话 | 本地回归；收费真实云端排除，不算通过 |
| 投屏、TVBox、分享 | cast/playback、设备命令、独立会话 | 模拟协议和真实设备分开；实体设备排除 |
| 工作台、设置和快捷键 | Main/SettingsV7、Protocol；11个设置页 | 共享组件状态与独立操作入口分别验证，不跑所有状态笛卡尔积 |
| 安全、迁移和更新 | credentials/MOTW/av_scan/migrate/updater | Core回归与环境依赖项分开；不以忽略项冒充通过 |
| 安装、升级和卸载 | MSI生命周期工具、installed_footprint | 最终同SHA包沙盒验收后才更新桌面交付 |

## 实测记录

- 首批修后 Core：645通过、0失败、12按原配置忽略；85.67秒。包含新增轮转故障与导入安全边界，不代表最终追加的合流限额已经验证。
- 首批真实 Core：千任务跨环境往返、坏文件后其他文件下载及哈希、HTTP边界通过。
- 桌面 TaskExportFileTest/UiDiagnosticsTest：通过；完整桌面源码编译与JAR构建通过。没有运行主程序于宿主。
- 插件类型检查、52文件/418测试、Chromium/Firefox构建通过。首次并行构建出现资源争用，主动中止后限制工作进程重新执行；中止记录保留，不计为产品失败或通过。

原始输出暂存在 `.tool-cache/full-audit-20261027`，收尾时只归档有用结果，不把编译缓存提交入库。

## 对标边界与未完成项

AB参考固定正式版1.11.1，前端取其选择、信息层级、共享交互行为；IDM参考下载所有权、恢复和动态调度的公开行为。既有三款比较记录见[对照报告](../idm-ab-20261019/README.md)，本轮不能把旧性能数据当新优化效果。

普通Chromium MV3不能使用一般扩展的blocking webRequest；AB插件MV3实现也明确不阻断响应。所有入口零DownloadItem仍未实现，不能用删除记录代替。没有改企业策略、驱动或系统安全设置。

指定直播网站403、原始HLS卡死现场、播放器偶发超时、启动慢点、全UI入口矩阵和当前三款性能对照尚未全部闭环，release_ready维持false。外部账号/有效会话、收费云端和实体设备缺少的证据不由模拟结果补齐。
