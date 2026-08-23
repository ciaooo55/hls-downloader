# HLS Downloader 7.0.0 验证状态

核验时间：2026-08-24

## 已通过

- 功能合同：`docs/feature-parity.json` 的 `25/25` 项均绑定旧版入口、v7 入口、Core 合同、状态事件和测试，机器校验为 `100%`。
- Rust Core：`279/279`，覆盖 IPC、数据库、HTTP/HLS/DASH、FTP/SFTP、BT、播放器、投屏、迁移和恶意输入。
- HTTP 分段专项：`39/39`；96 MiB 实际 Range 下载 `76.82 MiB/s`，32 个分段无重叠，发布后额外网络字节为 `0`。
- Compose：协议、组件、选择、右键菜单、设置、敌对输入和性能测试通过；1000 任务模型 P95 `14.802ms`。
- 热确认窗口：可见 P95 `31.03ms`，Native Host 提交 P95 `6.65ms`，提交到可见 P95 `27.95ms`，门槛均为 `100ms`。
- 浏览器扩展：`30/30` 文件、`190/190` 测试；Chromium MV3 和 Firefox MV3 生产构建成功。
- Native Host：安装目录二进制冷启动首响应 `618.76ms`，双响应总耗时 `620.64ms`，低于 `1500ms` 门槛。
- Presenter 和播放器：单实例、强杀隔离、Core 重连与播放器子进程退出测试通过。
- TVBox：真实接收端完成 `Range: bytes=0-` 拉取，HTTP `206`，共 `42521` 字节。
- 挂机：Core 30 秒工作集 `11.809 -> 11.797 MiB`；1000 次 IPC P95 `0.376ms`，错误和句柄增长均为 `0`。
- 安装后验证：Compose、Engine、Presenter 均从 `%LOCALAPPDATA%\Programs\HLSDownloader` 运行；窗口 `1280x760`，版本 `7.0.0`，图标和未授权 `401` 门禁通过。
- 便携升级：配置、数据库和下载目录保留，升级后真实回滚通过。
- PowerShell：15 个用户/维护脚本在 Windows PowerShell 5.1 与 PowerShell 7.6 中均通过语法解析。

## 本机构建哈希

- EXE：`2A66C8A83508BB5C140884157731D74FB0A8AE7C645DA2D28FD430BEFAD5E06F`
- MSI：`3D2873E0F7215E9EE68293B3F0BB66E3FAEE17BD9952E6F7F17BF6C9ADC4161A`
- Portable ZIP：`0F9485BB60EBC10051C66B51321E3EE9A73A9D363205B4EEF07F93667409B406`

这些哈希描述 2026-08-24 的本机受测构建，不作为后续不同提交产物的固定哈希。

## 正式标签前门槛

本地源码、运行、升级和浏览器桥接已经可用。创建公开 `v7.0.0` 标签前仍保留两项发布工程门槛：

1. 使用外部 Windows UI Automation 工具完成全部窗口、控件语义和键盘路径验收。
2. 在全新 Windows 虚拟机完成 MSI 安装、覆盖升级、重启、卸载和回滚矩阵。

这两项不影响当前 per-user 本地安装，但决定是否创建正式 GitHub Release。
