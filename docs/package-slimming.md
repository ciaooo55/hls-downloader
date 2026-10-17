# 安装包精简

生产运行保留 FFmpeg 和 libmpv；ffprobe 只供验收脚本使用，不再放入安装包。需要检查流信息的验收脚本通过 `--ffprobe` 显式接收独立测试工具路径，避免依赖被测应用携带测试工具。

桌面图标保留原来的 69 个矢量，路径、视口、填充与自动镜像属性不变。仅将官方 1.7.3 源码中的所用图标合并为 `OutlinedIcons.kt` 和 `AutoMirroredIcons.kt`，改包名和导入，不引入完整 extended 图标库。轻量 core 库仍提供 `Icons` 和矢量构造函数。Apache 2.0 许可证随桌面应用 JAR 分发。

原始源码归档：

- [material-icons-extended-desktop 1.7.3 sources](https://repo.maven.apache.org/maven2/org/jetbrains/compose/material/material-icons-extended-desktop/1.7.3/material-icons-extended-desktop-1.7.3-sources.jar)，SHA256 `4e23237644f7da35c391cc4c0263b95248605f499ab46f6d2c92d4fe48793d5a`。
- [material-icons-core-desktop 1.7.3 sources](https://repo.maven.apache.org/maven2/org/jetbrains/compose/material/material-icons-core-desktop/1.7.3/material-icons-core-desktop-1.7.3-sources.jar)，SHA256 `b3f1be7fda4d13a6d938bb918275ddc143e9d6a2007727f284778480526f5f16`。

字幕预取的 250 ms 音频包从实际发送开始计时，将通信耗时计入间隔。保持串行请求，不追赶积压发送，不改变音频时间轴、窗口大小、超时和就绪判定。该调整不等于保证云端响应耗时或消除所有字幕等待。

2026-10-17 的未签名候选包已在 Windows Sandbox 验收：MSI 从 302,788,608 B 降至 203,100,160 B；安装目录从 675,446,538 B 降至 472,788,108 B，分别减少约 32.9% 和 30.0%。Core、FFmpeg、libmpv 与 r8 相同，未裁剪媒体协议或解码功能。

插件类型检查、Chromium/Firefox 构建和 Firefox 审核源码打包完成；沙盒中 50 个插件测试文件、381 项测试通过，包含 11 项字幕测试。桌面 JAR 和桌面测试代码编译成功，未执行桌面 JUnit 套件。沙盒实际通过 21 项下载/边下边播/缓存清理、9 项播放器按钮与窗口、8 项安装/真实 MDN 下载/卸载、4 项 r8 升级检查。升级实际移除旧 ffprobe 和完整图标库，升级及卸载均保留非安装器管理的自建文件。

首次播放器测试错误地使用了 8.021 秒素材，快进 10 秒被正常限制在片尾；更换为原验收所需的 60 秒双音轨、双字幕素材后全部通过，并补上最小片长前提检查。原失败记录保留，不将其记为功能通过。

验收脚本还补上了 jpackage 启动器退出后对同一沙盒运行目录内进程的清理，并用独立进程句柄记录退出结果；再次定向复核的 9 项播放器检查通过，验收进程退出码为 0。该收尾只作用于验收运行目录。

本轮没有上传音频或重测真实云端字幕延迟，也没有本机安装或替换 GitHub 发布。具体结果见 [候选包验收记录](validation-slim-20261017.json)。
