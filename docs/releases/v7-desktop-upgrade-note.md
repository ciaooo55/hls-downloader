# HLS Downloader 升级提示

本文件供 `scripts/install/install-v7-local.ps1` 读取。产品版本以安装 manifest 和 canonical 元数据为准，不在此固定旧版本号或声称本机已安装。

- Compose 是主工作台；Rust Core 负责下载、调度、数据库和恢复。关闭工作台不等于停止下载。
- 浏览器确认、进度和完成反馈由轻量 Presenter 展示，扩展通过 Native Messaging 连接同一个 Core。
- 默认内置播放位于主界面浮层；libmpv 在隔离子进程中运行，画中画复用播放会话。
- 普通 MSI 可选择安装位置，默认下载目录为当前用户的 Windows“下载”文件夹。
- 任务、设置、密钥和下载成品不应随程序镜像清理。升级前备份，失败时保留可恢复来源。
- 未签名包手动安装；自动更新仍校验签名及项目签名者信任。

专用维护脚本的 `E:\h` 限制、Portable 与普通安装的数据区别，见 [安装与升级](../guides/v7-local-upgrade.md)。已执行验收及限制见 [验证状态](../validation/v7-verification.md)。
