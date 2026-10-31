# 安装包精简

生产运行保留 FFmpeg 和 libmpv；ffprobe 只供验收脚本使用，不再放入安装包。需要检查流信息的验收脚本通过 `--ffprobe` 显式接收独立测试工具路径，避免依赖被测应用携带测试工具。

桌面图标保留原来的 69 个矢量，路径、视口、填充与自动镜像属性不变。仅将官方 1.7.3 源码中的所用图标合并为 `OutlinedIcons.kt` 和 `AutoMirroredIcons.kt`，改包名和导入，不引入完整 extended 图标库。轻量 core 库仍提供 `Icons` 和矢量构造函数。Apache 2.0 许可证随桌面应用 JAR 分发。

原始源码归档：

- [material-icons-extended-desktop 1.7.3 sources](https://repo.maven.apache.org/maven2/org/jetbrains/compose/material/material-icons-extended-desktop/1.7.3/material-icons-extended-desktop-1.7.3-sources.jar)，SHA256 `4e23237644f7da35c391cc4c0263b95248605f499ab46f6d2c92d4fe48793d5a`。
- [material-icons-core-desktop 1.7.3 sources](https://repo.maven.apache.org/maven2/org/jetbrains/compose/material/material-icons-core-desktop/1.7.3/material-icons-core-desktop-1.7.3-sources.jar)，SHA256 `b3f1be7fda4d13a6d938bb918275ddc143e9d6a2007727f284778480526f5f16`。

字幕预取的 250 ms 音频包从实际发送开始计时，将通信耗时计入间隔。保持串行请求，不追赶积压发送，不改变音频时间轴、窗口大小、超时和就绪判定。该调整不等于保证云端响应耗时或消除所有字幕等待。

## 运行依赖

FFmpeg 用于下载合并和音频解码，libmpv 用于内置播放，裁剪 JRE 用于 Compose 工作台。当前包体积及验证范围以[本轮记录](../validation/convergence.md)为准，旧包测量保存在 Git 历史。
