# 2026-10-29 可靠性交接收敛

当前源码定向结果见 [validation.json](validation.json)。发布状态仍为 `release_ready=false`，尚未完成同SHA候选安装包验收。

- Edge 156：`edge/restart-diagnostic/` 主矩阵8项和首次Host离线通过。
- Firefox 159：`firefox/current-source/` 主矩阵8项和首次Host离线通过。重启时重新安装临时扩展，不代表商店签名升级测试。
- 一次性GET、POST、Blob使用原响应完成后导入；回执丢失仍同一request/handoff，拒绝保留源文件；扩展和浏览器重启后仍同一交接，单任务且SHA256正确。
- 普通响应入口创建浏览器下载项，如实记录 `zero_download_item=false`；符合用户确认的可靠性标准。明确链接点击前接管仍由Core下载。
- 原生按钮验收、候选包安装/升级/回滚/恢复/卸载、优化版性能对照另记，不能由协议结果代替。

当前最终接管回归为 `handoff-tests.json` 的89项；`extension-tests.json` 的456项对应较早快照，`local-import-tests.json` 对应中间版本，不能相加作为最终测试数。类型检查和双浏览器生产构建通过；Rust完成连接图3项、本地源文件5项通过；Core/Host/Updater release构建及Compose compileKotlin通过。

Core/Host浏览器矩阵使用当前dev二进制，确认窗沿用已校验r22组件，具体哈希保留在各bootstrap。JDK和打包依赖在项目缓存内；`dependency-reuse/bootstrap.json` 记录从已校验r22 MSI中提取同版libmpv与FFmpeg的哈希，宿主未安装产品。

历史失败保留：Firefox调试阶段分别遇到新版系统访问授权、CSP禁止eval、reload切换已丢弃窗口、测试误读request_id字段；均修正测试后继续。产品的数字ID跨会话恢复、同URL ID复用和storage写失败边界另有回归。Edge `current-source` 的CDP Runtime.evaluate超时保留；诊断runner分开观测异步回执和CDP生命周期后通过，没有修改产品超时，先前间歇失败原因未完全确证。

收费云字幕、实体投屏与原始特定媒体故障仍按主索引的外部限制记录，不在此声明通过。
