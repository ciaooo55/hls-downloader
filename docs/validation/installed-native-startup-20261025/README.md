# 已安装产品的正常管道与并发冷启动验收

## 本批目的与源码变化

继续核对历史首次安装 Engine 在 15 秒测试 TCP 期限内未就绪的问题。该次失败没有入口日志，既有证据不足以确定根因。本批补充此前安装验收未覆盖的正常 Windows 命名管道入口，而不是延长连接期限、吞掉错误或把再次成功当作历史问题已修复。

生产代码和桌面成品没有变化。复用当前 MSI SHA-256 `e6c49715dfd122c1858f8b568c69ec45e4273c9ba6c58ce3f0b4e60f696493cf`，Engine SHA-256 `8d9200cb821c899f604cf8b420eb742e6650df3a840f55dbcab9f37ab3e77f9e`，NativeHost SHA-256 `27d6c619e6489fd46aa61a5598dff8a79779621364d43b1b0a4d8f65ab6e7c92`。

在既有 `smoke_v7_installed_footprint.py` 增加可选 `--verify-native-host-startup`，复用 Native Host 帧读写和命名管道夹具。仅在 Windows Sandbox 安装并运行产品，不在宿主安装、不调用云端。

## 方法与实际结果

新启动的沙盒从含空格自选路径安装最终 MSI，核对组件和插件 ZIP 哈希。产品进程不传任何测试 IPC、测试数据目录或自定义 Engine 路径，仅打开已有的阶段诊断开关。四个 Host 通过 barrier 同时发起，每个连续发送两个真实 Native Messaging ping。

| 场景 | 四个 Host 首次响应范围 | Core 启动次数 | 常驻 Core |
| --- | ---: | ---: | --- |
| 首次安装后冷启动 | 1182.95–1185.66 ms | 1 | 唯一 PID 5100 |
| Core 已常驻时重新接入 | 70.41–74.33 ms | 0 | 沿用 PID 5100 |
| 正常关闭后重新拉起 | 126.60–129.23 ms | 1 | 唯一新 PID 1848 |

共 12 个 Host、24 个帧响应全部成功，协议版本正确；每组都核对默认命名管道 hello 的 PID 与实际安装路径下唯一 Engine 一致，加载的默认下载位置为系统 Known Folder 的 Downloads。关闭请求通过同一正常管道发出，确认进程实际退出后才进入下一阶段。沿用现有 Native Host 冷启动验收的 1500ms 目标，未放宽期限。

冷启动阶段日志显示：Core 入口到 IPC 就绪约 1.058 秒，其中 `database_open` 阶段约 1.049 秒；正常关闭后再次启动的 Core 入口到 IPC 就绪约 7.5ms。这只能定位本次冷启动的阶段开销，没有证明它就是历史 15 秒超时的根因。源码检查确认现有建表和版本标记已在同一事务中，未加入无证据的重复优化。

之后同一安装继续通过默认下载目录、公开 MDN 文件内容 SHA、HTTP 故障重试与队列、六组 HLS 普通/范围故障恢复、完成及删除缓存清理、正常卸载和产品注册移除。原始结果及阶段诊断见 [results/installed-native-startup.json](results/installed-native-startup.json)。

## 限制与收尾

本批没有打开真实浏览器、安装扩展或验收桌面按钮；Native Messaging 协议进程验收不能替代浏览器端到端接管验证。没有重建 UI/插件/Core 或改动桌面成品，未增加安装体积。640 个回归的结果仍是[前批同一 Engine 的验证](../hls-range-body-20261025/README.md)，本批没有重复执行它们。

历史首次启动超时仍未复现并确定根因，不能记为已修复；指定直播站点 403、原始 HLS 卡死现场、浏览器 MV3/POST/Blob、设备/云端和长期负载边界仍保留。不能宣称全功能与 IDM/AB 全面对等。

宿主只读核查及本批进程收尾另见 results。构建缓存删除此前被自动审批拒绝（`blocked by policy`），本批不换方法重试；任务创建的运行脚本与报告暂存仍在项目 `.tool-cache/request-control-20261025`，不声称目录只剩源码。没有创建分支或 worktree，没有改动用户下载、密钥、备份或其他项目。
