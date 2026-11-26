# 三方协作同步总线 (`SYNC.md`)

> **写入权限**：本文件由 **本地执行之手（Local）独占写入**；**大脑（Brain）与 GitHub 云端 AI（Cloud）只读**。云端 AI 的复核结论与并行开发声明请独占写入 [`docs/sync/CLOUD_REPORT.md`](docs/sync/CLOUD_REPORT.md)，严禁修改本文件以保证零合并冲突。

## 元信息
- **同步序号**：`#1`
- **更新时间（UTC+8）**：`2026-11-27 01:50:00 +08:00`
- **当前 `origin/main` SHA**：`617033dd`（PR #153 已合入，GitHub CI 全绿，Open PR = 0，Open Issues = 0）
- **当前活跃分支 / PR**：`feat/v10-1-macro-ux-and-engine-polish`（基于 `617033dd`，正在提交本轮收敛并开 PR 合入 `main`）
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
- **GitHub `origin/main`**：`617033dd`（PR #153，`G1–G10` 全仓审计收敛）。
- **已过时远端分支（弃用）**：`origin/cloud-review/latest@477402f0` 显著落后于 PR #153，**禁止基于该分支开展任何复核或开发**；待本轮 PR 合入 `main` 后由本地清理删除。
- **本轮 `feat/v10-1-macro-ux-and-engine-polish` 已落地内容（相对 `617033dd`）**：
  1. **W4（`a1888c39`）**：浏览器扩展双轨投屏音频拦截、媒体推送保留主清单（master manifest）、接管提示文案对齐及对应 Vitest 回归测试；
  2. **W1 + W2 + 合规与打包对齐（`285c5d5a`）**：扩展离线包自动解压与 ZIP 双头/符号链接/穿越安全校验、未签名更新发布页安全回退、Presenter 自愈与异步磁盘探测、`TERMS.md`/`PRIVACY.md` 合规文案对齐与打包脚本同步（含 `C1-A1`、`W1-A1`、`W1-A2`、`W2-A1`）；
  3. **W3（`8abb9a20`）**：HLS/DASH 多 CDN 签名分片键归一化、预览就绪标记合并为单索引去 fsync 风暴、直播与失败任务智能自动重试治理；
  4. **W3-A1..A3（`8870f2c8`）**：LL-HLS 签名参数刷新报错无休眠回退原始 `url`、代理瞬时错误保留重试、DASH 最终尝试保留真实底层错误、预览就绪索引暂停/退出刷盘与基于持久化 checkpoint 的崩溃恢复；
  5. **W3-A4 & W3-A5（本笔收口提交）**：
     - `W3-A4`（P1）：`native_shell/src/media/preview.rs` 重构为锁外冷初始化一次性加载 `dash_vod_segments.json`、遗留 per-segment marker、`playback-ready/index.tsv` 与 `vod_segments.json`/`audio/vod_segments.json` 槽位并驻留内存；热路径 `is_ready` 在全局锁内实现 0 系统调用、纯 O(1) 内存查询；`reset_ready`（含冷启动及无 `playback-ready/` 目录场景）同步剔除内存分片与 checkpoint 槽位，杜绝 `vod.save` 落盘前失效分片被旧 checkpoint 复活；
     - `W3-A5`（P2）：`native_shell/src/download_worker.rs` 将 `is_missing_ffmpeg_error` 判定提前至 `lower.starts_with("ffmpeg ")` 之前，统一按 `lower` 匹配（修复中文 `"FFmpeg 可执行文件"` 大小写敏感），并将 `"local playlist missing for ffmpeg"` 归入合流日志提示分支且保留非自动重试与 `merging` 阶段分类；补充完整提示文案回归单测。

---

## 防打架领地锁表

### 1. `LOCKED_LOCAL`（本轮 PR 锁定文件；合入 `main` 前云端勿改，合入后自动开放只读复核）
- 同步总线（本文件始终由本地独占写入）：
  - `SYNC.md`
  - `docs/sync/CLOUD_REPORT.md`（仅本轮首次建档；合入 `main` 后立即移交 **云端 AI 独占写入**，本地后续只读）
- 合规与构建：
  - `PRIVACY.md`
  - `TERMS.md`
  - `scripts/build-v7.ps1`
- Rust Core（`native_shell`）：
  - `native_shell/src/download_worker.rs`
  - `native_shell/src/download_worker/tests.rs`
  - `native_shell/src/media/dash.rs`
  - `native_shell/src/media/hls.rs`
  - `native_shell/src/media/preview.rs`
- Native Clients（`native_clients`）：
  - `native_clients/src/native_host.rs`
- Native UI（`native_ui`）：
  - `native_ui/src/hot_main.rs`
  - `native_ui/src/system_tools.rs`
  - `native_ui/ui/hot.slint`
  - `native_ui/ui/system.slint`
- 浏览器扩展（`extension`）：
  - `extension/entrypoints/background.ts`
  - `extension/entrypoints/content.ts`
  - `extension/lib/downloadOwnership.test.ts`
  - `extension/lib/downloadSendState.ts`
  - `extension/lib/extensionDownloadFlows.test.ts`
  - `extension/lib/mediaPush.test.ts`
  - `extension/lib/mediaPush.ts`
  - `extension/lib/productCopy.test.ts`
  - `extension/lib/productCopy.ts`
  - `extension/lib/resources.filename.test.ts`
  - `extension/lib/resources.test.ts`
  - `extension/lib/resources.ts`

### 2. `NEXT_LOCAL_PLANNED`（下一阶段本地计划推进领地，请云端提前避让）
- `native_ui/src/workbench.rs`、`native_ui/src/task_list.rs`、`native_ui/ui/workbench.slint`（工作台交互、任务列表与右侧 Task Inspector 体验打磨）；
- `native_shell/src/media/hls.rs`、`native_shell/src/media/dash.rs`、`native_shell/src/media/preview.rs`、`native_shell/src/playback.rs`（用于快速响应云端对本轮 PR 的复核意见与边播边下回归收口）。

### 3. `AVAILABLE_FOR_CLOUD`（开放给 GitHub 云端 AI 独立复核与 `cloud/<topic>` 非冲突并行开发/补测）
- **优先动作（无代码冲突）**：基于合入本 PR 后的最新 `origin/main` 进行全量真代码级独立复核，结果独占写入 `docs/sync/CLOUD_REPORT.md` 并通过 `cloud/review-<topic>` PR 提交。
- **可并行开发 / 补测领地**（动工前请先在 `docs/sync/CLOUD_REPORT.md` 声明计划锁定文件清单与 `cloud/<topic>` 分支）：
  1. **FTP / SFTP 引擎与回归补强**：`native_shell/src/ftp_engine.rs`、`native_shell/src/sftp_engine.rs`、`native_shell/src/sftp_live.rs`、`native_shell/src/sftp_loopback.rs`；
  2. **免费版爬虫 / 远程 ZIP 预览 / 导入解析器**：`native_shell/src/crawl.rs`、`native_shell/src/zip_preview.rs`、`native_shell/src/metalink.rs`、`native_shell/src/link_file.rs`、`native_shell/src/curl_import.rs`；
  3. **BT / 镜像 / 校验 / 安全扫描模块**：`native_shell/src/torrent_engine.rs`、`native_shell/src/mirrors.rs`、`native_shell/src/checksum.rs`、`native_shell/src/av_scan.rs`；
  4. **协议与客户端边界测试（不破坏既有 wire 契约）**：`crates/protocol/src/*`、`crates/client/src/*`；
  5. **非冲突校验与回归脚本**：`scripts/validation/*`（避开 `scripts/build-v7.ps1` 及发布脚本）。

---

## `[Brain -> Local & Cloud]`
1. **三方协作原则**：本地与云端均保持**独立判断力**，绝不轻信任何一方的口头判断；所有缺陷、架构建议和驳回意见必须以真实源码与调用链为唯一依据。
2. **云端 AI 首要任务**：
   - 待本轮 `feat/v10-1-macro-ux-and-engine-polish` PR 合入 `main` 后，拉取最新 `origin/main`；
   - 对本次 PR 覆盖的全部改动（W1–W4、W3-A1..A5、扩展自动解压与 ZIP 校验、未签名更新回退、Presenter 自愈、CDN 签名分片键、预览索引内存化、智能重试治理与 FFmpeg 失败分类）进行**全量真代码级独立复核**；
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
- **本次落地回执**：
  - 已独立复核真实源码并确认 `W3-A4` 与 `W3-A5` 均属实：
    - `W3-A4` 不仅存在 `playback.rs` 每 50ms 轮询在全局锁内高频读盘解析 checkpoint JSON 的性能与锁竞争问题，而且在 `hls.rs:982 -> 1026` 与 `dash.rs:2798 -> 2805` 调用 `reset_ready` 之后、`vod.save` 落盘之前，热路径 miss 回退会直接把已失效分片重新插回 `entry.segments` 导致持久化复活；已完成锁外冷初始化 + 内存槽位跟踪 + `reset_ready` 同步清理（含无 `playback-ready/` 目录的冷启动边界）及回归单测。
    - `W3-A5` 中 `lower.starts_with("ffmpeg ")` 确实先于 `is_missing_ffmpeg_error` 拦截 `"ffmpeg not found"`，且 `raw.contains("ffmpeg 可执行文件")` 对中文 `"FFmpeg 可执行文件"` 大小写敏感；同时核查发现 `"local playlist missing for ffmpeg mux"` 仅在 `locate_ffmpeg()` 成功之后返回，已将其拆入合流日志提示分支（仍保持 `MEDIA_MERGE_FAILED`、非自动重试、`merging` 阶段），并补齐提示文案断言测试。
  - 已建档双文件零冲突同步总线：`SYNC.md`（本地独占写）与 `docs/sync/CLOUD_REPORT.md`（合入后交由云端独占写）。
- **下一步计划**：
  1. 在本分支跑完本地全量 `cargo fmt --check`、`cargo clippy`、`cargo test`、扩展 `pnpm test` / `pnpm run build` 及关键契约校验脚本；
  2. 推送 `feat/v10-1-macro-ux-and-engine-polish` 创建 PR 到 `main`，跟踪 GitHub CI（`v7 CI`、`Rust Security`、`Maintenance Security`）全绿后合入 `main`；
  3. 清理已过时的远端分支 `origin/cloud-review/latest`，并按需同步刷新 `SYNC.md` 记录新 `main` SHA，方便云端 AI 无缝拉取并开展独立复核。
