# 安装包精简

生产运行保留 FFmpeg 和 libmpv；ffprobe 只供验收脚本使用，不再放入安装包。需要检查流信息的验收脚本通过 `--ffprobe` 显式接收独立测试工具路径，避免依赖被测应用携带测试工具。

桌面图标保留原来的 69 个矢量，路径、视口、填充与自动镜像属性不变。仅将官方 1.7.3 源码中的所用图标合并为 `OutlinedIcons.kt` 和 `AutoMirroredIcons.kt`，改包名和导入，不引入完整 extended 图标库。轻量 core 库仍提供 `Icons` 和矢量构造函数。Apache 2.0 许可证随桌面应用 JAR 分发。

原始源码归档：

- [material-icons-extended-desktop 1.7.3 sources](https://repo.maven.apache.org/maven2/org/jetbrains/compose/material/material-icons-extended-desktop/1.7.3/material-icons-extended-desktop-1.7.3-sources.jar)，SHA256 `4e23237644f7da35c391cc4c0263b95248605f499ab46f6d2c92d4fe48793d5a`。
- [material-icons-core-desktop 1.7.3 sources](https://repo.maven.apache.org/maven2/org/jetbrains/compose/material/material-icons-core-desktop/1.7.3/material-icons-core-desktop-1.7.3-sources.jar)，SHA256 `b3f1be7fda4d13a6d938bb918275ddc143e9d6a2007727f284778480526f5f16`。

字幕预取的 250 ms 音频包从实际发送开始计时，将通信耗时计入间隔。保持串行请求，不追赶积压发送，不改变音频时间轴、窗口大小、超时和就绪判定。该调整不等于保证云端响应耗时或消除所有字幕等待。

## 已发布包体积

r12 MSI 为 **203,108,352 B**，安装目录 **472,749,889 B（450.8 MiB，263 文件）**。早期未精简候选包 MSI 为 302,788,608 B、安装目录 675,446,538 B；该对比说明移除测试工具和完整图标库的收益，不表示安装大小等于运行内存。

FFmpeg 用于下载合并和音频解码，libmpv 用于内置播放，裁剪 JRE 用于 Compose 工作台，不能直接删除以追求体积数字。未新增第二套播放引擎。

r12 的实际安装、组件一致性、下载和卸载证据见 [安装报告](../validation/validation-r12-installer-20261018.json)，模块测试见 [模块报告](../validation/validation-module-audit-20261018.json)。首次瘦身及旧版升级移除 ffprobe/大图标库的历史证据见 [2026-10-17 报告](../validation/validation-slim-20261017.json)。报告中的失败及修正过程保留，不把旧包测试数重复计入 r12，也不表示当前已重新运行测试。
