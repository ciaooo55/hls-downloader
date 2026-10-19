# 中文安装与一次确认覆盖升级（2026-10-19）

本轮只修改安装器流程，应用和插件载荷沿用桌面 `3d3de7b` 版本。未签名，未在宿主安装或修改宿主注册表，未发布 GitHub Release。桌面成品及 SHA-256 见 [desktop-artifacts.json](desktop-artifacts.json)。

## 修复

- 使用 WiX 简体中文标准向导，首次安装保留 MIT 许可和安装路径选择。
- 检测到旧版时，“继续”直接开始覆盖，沿用原路径；成功后自动关闭，不再重复许可、目录和完成确认。Windows UAC 是独立系统授权，不在此一次确认承诺内。
- 在 `InstallValidate` 检查文件占用之前，限定原安装目录退出工作台和 NativeHost，再要求 Core 保存任务后退出。无法确认安全停机时中止，保留旧版。
- 停机助手使用 Rust 标准库并静态链接 CRT，避免在干净 Windows 的安装临时目录缺少 VC 运行库。
- PowerShell 明确等待窗口型 Engine 的停机请求进程，避免读取空的 `LASTEXITCODE`；关闭 Restart Manager 自动重启旧进程。

## 实际验收

环境为 Windows Sandbox，Edge 与已载入的 Chromium 插件保持打开；宿主产品没有运行或安装。

| 场景 | 结果与证据 |
| --- | --- |
| 旧工作台、Core、NativeHost 运行时覆盖 | 图形向导仅点击一次“继续”，成功后自行关闭；[原始日志](upgrade3-ui.log)最终返回 0，产品语言 2052。 |
| 原路径与单一产品登记 | [upgrade3-result.json](upgrade3-result.json)：沿用含空格的旧路径，只登记新产品。 |
| 任务和设置保留 | 暂停任务的状态和已记录字节保留；恢复后完成 32 MiB 文件，SHA-256 为 `e09320c5b00b34bb704802136c599a95b3996332ba84d7c7f21112b6231b6bd0`；[任务及桥接响应](preserved.json)、[下载目录设置](settings-preserved.json)。此项不用于宣称网络只重新传输剩余字节。 |
| 失败回滚 | 在文件安装和旧版移除后注入 deferred action 失败，返回 1603，原产品登记和 Engine 哈希恢复，成品及用户文件保留；[结果](rollback-result.json)、[原始日志](rollback.log)。 |
| 正常卸载 | 返回 0，产品登记、程序和已检查的浏览器 NativeHost 子键移除，用户文件和下载保留；[结果](uninstall-result.json)、[原始日志](uninstall.log)。 |
| 首次安装选路径 | 实际经过中文许可页、路径选择和完成页，输入 `C:\QAWork\Selected Fresh Path\HLSDownloader\`；[原始日志](first-install-ui.log)确认 INSTALLDIR、相应浏览器注册路径以及服务端和客户端均返回 0。 |
| 不误停其他目录同名程序 | `C:\QAWork\Other Project\HLSDownloaderEngine.exe` 测试进程在覆盖后仍运行，见 upgrade3-result。 |

首次安装后的第二次独立检查脚本未返回，不能将它视为通过；首次安装成功依据为实际完成页和 MSI 原始日志。已完成的正常卸载验收来自前一轮同一候选包。结束测试时关闭整台专用沙盒，丢弃其临时安装。

前两次候选尝试分别因独立助手缺 VC 运行库、停机退出码判断失败而在移除旧版前中止，均未交付。最终交付 MSI 的 SHA-256 为 `0544093260bb4f3a7893a0d591a0ec1d8cbb02007cf4873398ad3da48a90d172`，比原桌面 MSI 增加 348,160 字节（约 340 KiB），没有新增产品运行依赖。

本轮执行了助手编译、WiX 中文重打包、MSI 生命周期补丁校验、上述沙盒验收、PowerShell 静态语法检查、Rust 格式检查和 Git 差异检查。未重新执行播放器、下载接管或字幕云端测试；本轮结果仅覆盖安装器修改。

## 清理状态

桌面仅保留同一套 MSI、Chromium 插件、Firefox 未签名插件和审核源码 ZIP。专用沙盒已发起关闭。本轮 `.tool-cache/installer-upgrade` 的递归删除被自动审批拒绝，仅返回 `blocked by policy`，没有具体原因；不能声称工作目录已无缓存。用户可手动运行 [finish-cleanup.ps1](finish-cleanup.ps1)，先核验上述四份交付的哈希，再仅删除本轮缓存。该脚本尚未执行，已做静态语法检查。
