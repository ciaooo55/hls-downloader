# 对抗检查补充（第五轮）

## 范围

本轮从 `9af92be0a4300e8f7bb70e96f3f35f3dee02debc` 开始，起始工作区干净。沿媒体拼接及 HLS/DASH 调用方、工作台下载地址校验、Chromium 扩展安装解压调用链继续检查。

对照第三轮清单并叠加第四轮变更哈希，419 个活动文件中 **414 个未变、5 个修改、0 个缺失**。具体路径及前后 SHA-256 见 [本轮源码变更](adversarial-followup-source-changes.csv)。清单核对只说明文件状态，不表示未修改文件重新接受了逐行人工审查或运行验证。

## 已复现并修复

| 问题 | 实际影响与最小修复 |
| --- | --- |
| 媒体拼接提前截断输出 | `concat_files` 先创建输出，再打开输入。后续输入缺失会破坏已有输出；输入包含输出自身时还会持续复制新增字节。改为独占创建同目录暂存文件，流式复制、同步、关闭后复用现有 `atomic_replace::replace_file`。失败清理本次暂存文件，不改旧输出。HLS TS 拼接与 DASH 字幕拼接共用这一实现。 |
| torrent 后缀绕过 UI URL 校验 | 原谓词把任意以 `.torrent` 结尾的字符串当成本地文件，错误放行伪协议和缺主机名的远程地址。远程协议始终校验 URI 主机名；本地种子必须是绝对路径、完整字符串以 `.torrent` 结尾，拒绝 Windows 设备路径前缀。Core 仍有独立校验，此处不宣称已证实任意代码执行。 |
| 扩展 ZIP 解压无资源边界 | 原实现仅检查规范化路径是否在目标内，解压体积及条目数量无限制。现在限制总解压正文 32 MiB、2048 个条目，固定 8 KiB 缓冲区，读取至剩余额度加 1 字节后拒绝超额；目录条目正文同样计入，避免 `closeEntry` 隐式耗尽超大正文。拒绝路径穿越、ADS、设备名、尾点/空格、非法组件及大小写重复条目。沿用现有唯一暂存目录与失败回收流程。 |

没有新增依赖、下载引擎、状态管理层或兼容包装。正常目录、中文文件名、合法 HTTP(S)/FTPS 地址和绝对种子路径仍由定向测试覆盖。

## 实际验证

修复前：Rust 旧输出保留回归实际失败，`old-output` 被替换成部分输入 `input`，见 [红测试输出](followup-red-rust.txt)。早期输出别名样本出现持续扩张后已中止，没有继续对旧实现运行该样本。

Kotlin 修复前共 29 项、3 项失败，分别是 URL 放行、超大解压正文、过多 ZIP 条目，见 [红测试输出](followup-red-kotlin.txt)及 [ZIP 红测试 XML](followup-red-BrowserExtensionInstallerTest.xml)、[URL 红测试 XML](followup-red-ProtocolTest.xml)。旧实现的条目数断言先失败，因此不把同测试中尚未执行的路径反例计为修复前已复现。

| 检查 | 结果 |
| --- | --- |
| Rust 媒体拼接模块 | **7 通过、0 失败、1 ignored**。验证缺输入保留原文件、输出别名不自我扩张、成功和失败均无暂存文件残留，以及既有合流控制/日志行为。真实 FFmpeg 测试仍要求 Windows Sandbox。见 [原始输出](followup-green-rust.txt)。 |
| HLS 现有调用方 | TS 点播拼接测试 **1 通过**，验证两段内容正确进入最终输出。见 [原始输出](followup-green-hls-caller.txt)。 |
| Kotlin 定向测试 | 最终 **32 通过、0 失败**（扩展安装 6、Protocol 26）。包括跨条目累计上限、恰好 32 MiB 正例、超大目录正文、2049 条目、路径别名、重复条目及合法地址。见 [构建输出](followup-green-kotlin-boundaries.txt)、[ZIP 结果](followup-green-BrowserExtensionInstallerTest.xml)、[Protocol 结果](followup-green-ProtocolTest.xml)。 |
| Core 严格 Clippy | 离线、锁定依赖、全目标通过；仅沿用 CI 的三项接口结构 lint 例外。见 [原始输出](followup-checks-rust.txt)。 |
| Rust 格式及差异 | `cargo fmt --all -- --check`、`git diff --check` 通过。 |

Kotlin 测试离线运行，使用已有 JDK 21/Gradle 缓存及 `java.awt.headless=true`，临时文件定向到工作区。第一轮 31 项通过后，为新补的累计及目录正文边界再次定向执行，最终 32 项通过；不叠加两轮通过数。没有运行完整 Compose UI 测试集合，也没有启动宿主产品窗口。

## 未验证范围与保留材料

普通媒体分片仍可能整片入内存，本轮没有实施流式分片改造。未修改的浏览器扩展、Presenter、脚本沿用前轮证据，未重新执行其全部测试。DASH 调用方完成静态核对，未重新运行 DASH 端到端或真实 FFmpeg。未运行原生 UI、安装/卸载、浏览器矩阵、性能、投屏/TVBox 或收费字幕验收。

候选包未重建，`release_ready` 仍为 false；本轮源码测试不能替代同包验收。此前全量清单及本轮有限样本均不能证明仓库每行代码无缺陷。

本轮诊断位于 `.tool-cache/adversarial-followup-20261031/`，保留原始日志；本目录归档的文本日志仅规范化换行及行尾空白。保留失败样本 `runtime/hls-concat-failure-18584/`，含 5 字节输入与 1,996,075 字节扩张输出，供复核。其余本轮成功测试使用的临时目录已由测试自行回收。已有编译缓存继续保留。结束检查未发现测试、编译或产品进程遗留。

前轮被自动审批拒绝删除的 `.tool-cache/adversarial-audit-20261031/runtime/` 及 Downloads 下两个 control 文件继续保留，未重试或换工具绕过。具体位置和拒绝原因见 [前轮交付记录](adversarial-audit.md#限制与交付)。不宣称任务残留已全部清理。

没有推送、发布或签名。
