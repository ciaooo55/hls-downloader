# 全项目代码清理记录（2026-10-31）

本页记录首轮清理；后续生产客户端与脚本解耦及新增定向验证见 [代码质量收敛补充](quality-followup.md)。

第三轮的逐文件检查清单、边界修复和全目标检查结果见 [逐文件代码质量检查](module-audit.md)。

第四轮的恶意输入、路径越界、资源上限、BT 文件发布与扩大行为验证见 [对抗检查与修复](adversarial-audit.md)。

第五轮的媒体拼接原子替换、工作台 URL 校验与扩展 ZIP 有界解压见 [对抗检查补充](adversarial-followup.md)。

## 范围与结论

本轮从 `7a1111c699894b12f1a942d3188e38db34e77486` 开始，起始工作区干净。检查范围包括 Rust Core、Native Host、CLI、Presenter、Compose 工作台、浏览器扩展，以及构建、安装、发布和验证脚本。

初始静态扫描覆盖 378 个代码/脚本文件，约 14 万行（含测试）；其中生产路径源文件 146 个，约 7.2 万行。扫描用于定位候选问题，删除和合并依据还包括实际入口、调用链、构建配置及持久化语义，不能把一次文本扫描当成全部代码均无问题的证明。

本轮已清理确认重复、无调用或依赖方向不合理的实现，没有重写产品架构，也没有改变免费版本方向。

## 实际修改

| 区域 | 修改与目的 |
| --- | --- |
| IPC 协议 | `EventEnvelope` 统一放入 `contract.rs`，删除重复的 `client_event.rs`，完整 Core 和轻量客户端使用同一定义，协议层不再依赖运行时实现。 |
| 客户端依赖 | 将 9 项下载/Core 依赖限定到 `full-core`；轻量客户端的直接依赖只保留 serde、serde_json、windows-sys。Presenter 锁文件减少 132 个包，没有新增包或升级版本。 |
| 队列排序 | `place` 和 `reorder` 共用队列排序及索引发布逻辑；避免排序键复制，并修复极端 delta 的 i32 加法溢出。 |
| 设置读取 | 合并重复 SQL 查询及 bool/u64 的反序列化回退，保留历史裸字符串设置的实际兼容需求。 |
| Base64 | 删除 cURL 模块手写编码器，使用已有依赖；下载协调器不再调用 cURL 解析器内部的编码函数。保留具有独立边界语义的请求体解码逻辑。 |
| SHA-1 | 文件校验和 BT 所需的一次性摘要共用流式实现，删除第二份压缩算法；文件校验继续分块读取。 |
| 更新检查 | 将 HTTP 请求移出 Core 状态机和共享锁范围；网络完成后再持锁发布事件，保留静默、错误和版本通知行为。 |
| 扩展归档 | 安装和便携打包共用 `Assert-V7ExtensionArchive`；直接读取 ZIP 中的 manifest，删除重复校验和临时解压目录管理。 |
| Compose | 删除无调用的 `openMain()` 包装和 12 个未使用 import，没有修改界面行为。 |
| 版本门禁 | 给现有 audit_state 补上必需的 `v7_0_3_` 前缀；未提高验收状态，`release_ready` 仍为 false。 |

## 排查后保留的内容

- Kotlin 图标都有实际调用；扩展未发现脱离生产入口的独立 lib 模块，没有按文件名或引用次数猜测删除。
- `V7_CORE_PROTOCOL`、`CHROMIUM_PUBLIC_KEY` 分别由构建检查和 WXT 消费，系统回调也不能用普通文本调用计数判为死代码。
- 更新器独立进程与 Core 各自承担签名校验边界；少量相同 WinTrust 初始化语句未为消除行重复而引入新耦合。
- HTTP Range 校验、历史数据兼容、测试夹具及验证脚本的少量协议/COM 片段有不同上下文；未强行抽成跨工具公共层。
- 大文件体积本身不证明应该拆分。本轮处理确认的重复与锁范围问题，没有为了目录整齐增加转发层。

## 实际验证

| 验证 | 结果与范围 |
| --- | --- |
| 轻量 Rust 库 | `cargo check --offline --manifest-path native_shell/Cargo.toml --lib --no-default-features --features presenter-client` 通过。 |
| 完整 Core 测试目标 | 当前代码的库测试目标编译通过（`--lib --no-run`）；没有执行全量测试。 |
| Rust 定向测试 | 38 项通过，0 失败，636 项过滤；覆盖协议、队列移动、cURL、设置回退、摘要已知答案及流式边界。见 [测试原始输出](core-targeted-tests.txt)。 |
| 依赖树 | 完整与轻量 profile 检查通过；Presenter 锁文件离线更新后 `--offline --locked` 检查通过。 |
| TypeScript | 扩展 `tsc --noEmit --project extension/tsconfig.json` 通过。 |
| PowerShell | `scripts/validation/validate-powershell.ps1` 通过：57 个脚本语法、5 项正式 gate ID、调用/记录/证据合同和 v7.0.3 版本门禁。 |
| 归档校验 | PowerShell 7 下两浏览器真实归档、错误版本、错误身份共 6 个正反例通过；Windows PowerShell 5.1 下两浏览器真实归档通过。 |
| Python | 88 个已跟踪 Python 脚本经 AST 语法检查通过。 |
| 差异检查 | `git diff --check` 通过。 |

首次 PowerShell 检查暴露了现有 audit_state 缺版本前缀，修复后通过。Presenter 首次离线解析所用项目缓存缺少 Slint 索引，改用已有标准 Cargo 缓存完成解析；未联网安装依赖。

没有运行 Compose 完整编译、原生 UI、浏览器端到端、性能测试或真实 GitHub 更新请求。更新检查锁范围的结论来自调用链审查与当前 Core 测试目标编译，不等同于并发集成验收。

## 交付状态与保留材料

- 本轮为源码清理；现有 `bafddff6` 候选包未重新构建，不包含本轮修改。原计划中的完整同包验收不能据此计为完成，发布状态仍为不可发布。
- 原有候选包、用户数据、恢复数据和失败证据均保留。
- 本轮扫描和编译诊断保留在工作区忽略目录 `.tool-cache/code-audit-20261031/`，未复制编译缓存进版本库。
- 此前缓存清理被自动审批拒绝，未继续尝试删除，也未把磁盘缓存清理宣称为完成。
- 本轮未启动产品 UI；结束检查未发现 cargo、rustc 或产品进程遗留。没有推送、发布或签名。
