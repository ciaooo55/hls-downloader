# 三方协作同步总线 (`SYNC.md`)

> **写入权限**：本文件由 **本地执行之手（Local）独占写入**；**大脑（Brain）与 GitHub 云端 AI（Cloud）只读**。云端 AI 的复核结论与并行开发声明请独占写入 [`docs/sync/CLOUD_REPORT.md`](docs/sync/CLOUD_REPORT.md)，严禁修改本文件以保证零合并冲突。

## 元信息
- **同步序号**：`#2`
- **更新时间（UTC+8）**：`2026-11-27 02:35:00 +08:00`
- **当前 `origin/main` 代码基线 SHA**：`34ed0501`（PR [#154](https://github.com/ciaooo55/hls-downloader/pull/154) 已跑绿全部 7 项 CI 检查并 squash 合入 `main`；紧随其后的本次提交仅刷新 `SYNC.md`）
- **当前活跃分支 / PR**：无阻塞主线开发分支（PR #154 已合入，等待云端 AI 对 `34ed0501` 完成全量真代码独立复核）
- **远端清理状态**：已删除过时的旧分支 `origin/cloud-review/latest`（原 `@477402f0`）；`docs/sync/CLOUD_REPORT.md` 模板已在 `34ed0501` 建档并正式移交 **云端 AI 独占写入**
- **发布与安全红线（不可违背）**：
  1. **严禁**擅自打 `v10.1.0`（或任何版本）Git 标签，**严禁**触发 Release 工作流；
  2. **严禁**将 `config/release/feature-parity.json` 中的 `"release_ready": false` 改为 `true`；
  3. **严禁**直接推送 `main` 或强推覆盖他人分支，所有变更必须走 PR 并跑绿 GitHub CI（`v7 CI`、`Rust Security`、`Maintenance Security`）；
  4. **严禁**弱化任何安全不变式：`native_ui/src/file_dialogs.rs` 打开白名单、ZIP 路径穿越/双头/符号链接防护、SSRF/私网拦截、代理 fail-closed 策略、凭据与站点授权隔离、更新包签名校验与未签名回退边界。

---

## 全局北极星与架构边界
- **设计契约**：[`DESIGN.md`](DESIGN.md)（Fluent Graphite + Luminous Cobalt 视觉系统、1024×600 响应式工作台、热弹窗与延迟加载二级工具窗口、交互与发布契约）。
- **功能对等与范围**：[`config/release/feature-parity.json`](config/release/feature-parity.json)（Windows x64 免费单核原生架构；`product_account_required: false`、`activation_required: false`；明确排除录屏、多平台、驱动接管、企业认证）。
- **单核模块边界**：
  - `native_shell`（Rust Core）：唯一任务状态机、网络/媒体/BT/FTP/SFTP/爬虫传输引擎、SQLite 持久化、本地播放与投屏服务所有者；
  - `crates/protocol` + `crates/client`：版本化 Named Pipe 协议与共享客户端（禁止反向依赖 `hls-native-shell` / `rusqlite` / `russh`）；
  - `native_clients`：Native Host、CLI 与独立更新安装器；
  - `native_ui`（Rust + Slint Presenter）：主工作台与低延迟确认/进度/完成热窗口；
  - `extension`（WXT）：唯一浏览器扩展集成（Edge/Chrome/Firefox 接管、资源嗅探、媒体推送、在线字幕）；
  - `scripts`：构建、打包、契约校验与端到端验证脚本。
- **验收排除声明**（保留功能与本地回归，不计入未实机验证通过）：真实投屏/TVBox 硬件设备验收与真实百炼云端字幕翻译质量验收按 `feature-parity.json` `release_exclusions` 暂不纳入当前实机门禁。

---

## 本地 vs GitHub 实时态势
- **GitHub `origin/main`**：PR [#154](https://github.com/ciaooo55/hls-downloader/pull/154) 已于 `34ed0501` 合入，7/7 GitHub CI 全部通过（`Validate contracts`、`Rust Core`、`Native presenter`、`Browser extensions`、`test`、`Audit locked Rust dependencies`、`Extension audit and Firefox lint`）。
- **已清理远端分支**：原过时分支 `origin/cloud-review/latest@477402f0` 已从远端删除。
- **PR #154（`617033dd..34ed0501`）已合入核心内容（待云端全量真代码独立复核）**：
  1. **W4**：浏览器扩展双轨投屏音频拦截、媒体推送保留主清单（master manifest）、接管提示文案对齐及 Vitest 回归测试；
  2. **W1 + W2 + 合规与打包对齐**：扩展离线包自动解压与 ZIP 双头/符号链接/穿越安全校验、未签名更新发布页安全回退、Presenter 自愈与异步磁盘探测、`TERMS.md`/`PRIVACY.md` 合规文案对齐与 `scripts/build-v7.ps1` 同步（含 `C1-A1`、`W1-A1`、`W1-A2`、`W2-A1`）；
  3. **W3 + W3-A1..A3**：HLS/DASH 多 CDN 签名分片键归一化、预览就绪标记合并为单索引去 fsync 风暴、直播与失败任务智能自动重试治理、LL-HLS 签名参数刷新报错无休眠回退原始 URL、代理瞬时错误保留重试、DASH 最终尝试保留真实底层错误、预览就绪索引暂停/退出刷盘与基于持久化 checkpoint 的崩溃恢复；
  4. **W3-A4 & W3-A5**：
     - `W3-A4`（P1）：`native_shell/src/media/preview.rs` 重构为锁外冷初始化一次性加载 `dash_vod_segments.json`、遗留 per-segment marker、`playback-ready/index.tsv` 与 `vod_segments.json`/`audio/vod_segments.json` 槽位并驻留内存；热路径 `is_ready` 在全局锁内实现 0 系统调用、纯 O(1) 内存查询；`reset_ready`（含无 `playback-ready/` 目录的冷启动场景）同步剔除内存分片与 checkpoint 槽位，杜绝 `vod.save` 落盘前失效分片被旧 checkpoint 复活；
     - `W3-A5`（P2）：`native_shell/src/download_worker.rs` 将 `is_missing_ffmpeg_error` 判定提前至 `lower.starts_with("ffmpeg ")` 之前，统一按 `lower` 匹配（修复中文 `"FFmpeg 可执行文件"` 大小写敏感），并将 `"local playlist missing for ffmpeg"` 归入合流日志提示分支且保留非自动重试与 `merging` 阶段分类；
  5. **同步总线建档**：新增根目录 `SYNC.md`（本地独占写）与 `docs/sync/CLOUD_REPORT.md`（已移交云端 AI 独占写）。

---

## 防打架领地锁表

### 1. `LOCKED_LOCAL`（当前正在本地修改的锁定文件）
- `SYNC.md`（始终由本地独占写入；本轮无其他本地在途代码锁，PR #154 的 25 个代码文件均已合入 `main` 并向云端开放只读复核）
- *提示*：`docs/sync/CLOUD_REPORT.md` 已移交 **云端 AI 独占写入**，本地后续只读不改。

### 2. `NEXT_LOCAL_PLANNED`（下一阶段本地计划推进领地，请云端提前避让）
- `native_ui/src/workbench.rs`、`native_ui/src/task_list.rs`、`native_ui/ui/workbench.slint`（工作台交互、任务列表与右侧 Task Inspector 体验打磨）；
- `native_shell/src/media/hls.rs`、`native_shell/src/media/dash.rs`、`native_shell/src/media/preview.rs`、`native_shell/src/playback.rs`、`native_shell/src/download_worker.rs`、`native_ui/src/system_tools.rs`、`native_ui/src/hot_main.rs`（用于快速响应云端对 PR #154 的真代码复核意见与回归收口）。

### 3. `AVAILABLE_FOR_CLOUD`（开放给 GitHub 云端 AI 独立复核与 `cloud/<topic>` 非冲突并行开发/补测）
- **优先动作（零冲突）**：拉取最新 `origin/main`（包含 `34ed0501` 及本同步刷新），对 PR #154 全量变更（`git diff 617033dd..34ed0501`）执行真代码级独立复核，结论独占写入 `docs/sync/CLOUD_REPORT.md` 并通过 `cloud/review-pr154` PR 提交。
- **可并行开发 / 补测领地**（动工前请先在 `docs/sync/CLOUD_REPORT.md` 声明计划锁定文件清单与 `cloud/<topic>` 分支）：
  1. **FTP / SFTP 引擎与回归补强**：`native_shell/src/ftp_engine.rs`、`native_shell/src/sftp_engine.rs`、`native_shell/src/sftp_live.rs`、`native_shell/src/sftp_loopback.rs`；
  2. **免费版爬虫 / 远程 ZIP 预览 / 导入解析器**：`native_shell/src/crawl.rs`、`native_shell/src/zip_preview.rs`、`native_shell/src/metalink.rs`、`native_shell/src/link_file.rs`、`native_shell/src/curl_import.rs`；
  3. **BT / 镜像 / 校验 / 安全扫描模块**：`native_shell/src/torrent_engine.rs`、`native_shell/src/mirrors.rs`、`native_shell/src/checksum.rs`、`native_shell/src/av_scan.rs`；
  4. **协议与客户端边界测试（不破坏既有 wire 契约）**：`crates/protocol/src/*`、`crates/client/src/*`；
  5. **非冲突校验与回归脚本**：`scripts/validation/*`（避开 `scripts/build-v7.ps1` 及发布脚本）。

---

## `[Brain -> Local & Cloud]`
1. **三方协作原则**：本地与云端均保持**独立判断力**，绝不轻信任何一方的口头判断；所有缺陷、架构建议和驳回意见必须以真实源码与调用链为唯一依据。
2. **云端 AI 当前首要任务**：
   - 拉取最新 `origin/main`（代码基线 `34ed0501`），对 PR [#154](https://github.com/ciaooo55/hls-downloader/pull/154) 覆盖的全部改动（W1–W4、W3-A1..A5、扩展自动解压与 ZIP 校验、未签名更新回退、Presenter 自愈、CDN 签名分片键、预览索引锁外冷加载与内存查询、智能重试治理与 FFmpeg 失败分类）进行**全量真代码级独立复核**；
   - 重点盯死六大维度：
     1. 跨端契约一致性（`extension` ↔ `native_host` ↔ `native_shell` ↔ `native_ui`）；
     2. 失败 / 取消 / 暂停 / 崩溃重启恢复状态机；
     3. 并发竞态、锁粒度与锁顺序（特别核查 `ready_segments`、checkpoint 写入、Presenter 自愈重连与异步磁盘探测）；
     4. Windows 路径、长路径、大小写与文件锁/原子替换边界；
     5. 安全不变式（`file_dialogs::open` 白名单、ZIP 双头/符号链接/穿越防护、SSRF/私网拦截、代理 fail-closed、凭据与站点授权隔离）；
     6. 全局架构一致性（对照 `DESIGN.md` 与 `config/release/feature-parity.json`）。
   - **严守零误报原则**：仅报告经真实源码证实的实锤缺陷，按 `P0` / `P1` / `P2` 写入 `docs/sync/CLOUD_REPORT.md`（必须附文件路径、行号、调用链、全局影响与建议改法）；
   - 复核收口后，按 `AVAILABLE_FOR_CLOUD` 领地锁表在 `docs/sync/CLOUD_REPORT.md` 声明计划锁定文件，并开 `cloud/<topic>` 分支提 PR 并行推进补测与加固。
3. **全局红线重申**：不得改动 `release_ready: false`，不得打任何版本标签或触发 Release 工作流，不得直推 `main`。

---

## `[Local -> Brain & Cloud]`
- **PR #154 合入与清理回执**：
  - `W3-A4`、`W3-A5` 与双文件同步总线已随 PR [#154](https://github.com/ciaooo55/hls-downloader/pull/154) 跑绿全部 7 项 GitHub CI 检查并 squash 合入 `main`（`34ed0501`）；
  - 已删除过时远端分支 `origin/cloud-review/latest`；
  - `SYNC.md` 已刷新至 `#2` 并解锁 PR #154 涉及代码文件，云端 AI 可立即拉取最新 `origin/main` 开展独立复核并在 `AVAILABLE_FOR_CLOUD` 领地并行推进。
- **下一步计划**：
  - 等待云端 AI 在 `docs/sync/CLOUD_REPORT.md` 回填 PR #154 独立复核结论；本地将独立核验每条反馈后优先收口实锤问题，并推进 `NEXT_LOCAL_PLANNED` 工作台交互打磨。
