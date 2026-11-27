# 三方协作同步总线 (`SYNC.md`)

> **写入权限**：本文件由 **本地执行之手（Local）独占写入**；**大脑（Brain）与 GitHub 云端 AI（Cloud）只读**。云端 AI 的复核结论与并行开发声明请独占写入 [`docs/sync/CLOUD_REPORT.md`](docs/sync/CLOUD_REPORT.md)，严禁修改本文件以保证零合并冲突。

## 元信息
- **同步序号**：`#4`
- **更新时间（UTC+8）**：`2026-11-27 18:38:00 +08:00`
- **当前 `origin/main` SHA**：`74848fe3`（已依次合入云端 PR [#157](https://github.com/ciaooo55/hls-downloader/pull/157) `9feab70f`、PR [#159](https://github.com/ciaooo55/hls-downloader/pull/159) `4fb1ae09`、本地 PR [#160](https://github.com/ciaooo55/hls-downloader/pull/160) `16324a8f` 与 PR [#161](https://github.com/ciaooo55/hls-downloader/pull/161) `74848fe3`；紧随其后的本次提交仅刷新 `SYNC.md`）
- **当前活跃状态**：
  - **本地 PR A（[#160](https://github.com/ciaooo55/hls-downloader/pull/160)，`16324a8f`）已全绿合入**：收口 `CR-P1-1`、`CR-P1-2`、`CR-P2-1..14`（16 项运行时缺陷）及合并前复核新增的 `PRA-P2-1`，GitHub CI 7/7 通过。
  - **本地 PR B（[#161](https://github.com/ciaooo55/hls-downloader/pull/161)，`74848fe3`）已全绿合入**：收口 `CR-P1-3`、`CR-P1-4`、`CR-P2-15..17`（5 项 Windows 打包 / 签名链 / SBOM / 发布工作流 / Preflight 缺陷）及合并前复核新增的 `PRB-P1-1`、`PRB-P1-2`、`PRB-P1-3`、`PRB-P2-1`，GitHub CI 5/5 通过。
  - **云端并行首要任务（唯一未收口 `CLOUD_REPORT.md` P1 + 新 `main` 复核）**：请云端 AI 拉取最新 `origin/main` 与本 `SYNC.md` `#4`，优先认领 **`CR-P1-5`**（`cloud/fix-p1-5-media-regression-gate`），随后对合入 PR #160 / PR #161 后的最新 `main` 开展独立实码复核并更新 `docs/sync/CLOUD_REPORT.md`。
- **远端清理状态**：已清理已合入的 `origin/fix/cloud-report-runtime-convergence` 与 `origin/fix/cloud-report-packaging-signing-preflight`；此前已清理 `origin/cloud/review-pr154` 与 `origin/cloud-review/latest`。
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
- **本轮已合入 `main` 清单（累计 21/22 项 `CLOUD_REPORT.md` 缺陷 + 5 项预审新增项 + 25 项协议/校验边界单测）**：
  1. **云端 PR [#157](https://github.com/ciaooo55/hls-downloader/pull/157) & [#159](https://github.com/ciaooo55/hls-downloader/pull/159)（`9feab70f` / `4fb1ae09`）**：新增 `crates/protocol/tests/wire_contract_boundaries.rs` 与 `crates/client/tests/request_validation_boundaries.rs` 共 25 项边界测试（0 生产代码改动），7/7 CI 全绿合入。
  2. **本地 PR [#160](https://github.com/ciaooo55/hls-downloader/pull/160)（`16324a8f`，7/7 CI 全绿）**：
     - **Core & Media**（`CR-P1-1`、`CR-P2-1`、`CR-P2-2`、`CR-P2-8`、`CR-P2-9`、`CR-P2-10`、`PRA-P2-1`）：按 `Variant` 声明的 `AUDIO`/`SUBTITLES` `GROUP-ID` 严格筛选轨道（空组保留内嵌音轨）；错误归因优先识别 DRM/网络策略而非 `INVALID_URL`；`preview::reset_ready` 移除全局锁内同步磁盘刷盘并建立单点 `flush_lock -> ready_segments()` 锁序与代数防重；VOD/Live WebVTT 共享 `EXT-X-MAP` 缓存与 33-bit MPEG-TS wraparound 展开（`EXT-X-DISCONTINUITY` 仅在后续首个分片落盘成功后清零）；DASH `r=-1` 直播 DVR 预跳过使用 `u128`/`i128` 零溢出算术并保持百万分片与 URL 预算上限。
     - **Native UI / IPC / Native Host**（`CR-P2-3`、`CR-P2-4`、`CR-P2-5`、`CR-P2-6`、`CR-P2-11`、`CR-P2-12`、`CR-P2-13`）：ZIP 降级解包前安全重置非符号链接 staging 目录；拒绝 POSIX `S_IFLNK`（含 Darwin `19`）/ VFAT 重解析点、多卷 ZIP 与加密位；发布目标目录防符号链接 + 降级覆盖清理陈旧文件 + `.archive-stamp` 前落盘树校验；`UpdateAvailable` 补齐 `ensure_fallback_release_url()`；`media_push_ui_request_id` 仅在 `open_workbench()` 成功后缓存；`NamedPipeClient` 与 `OverlappedPipe` 支持端到端 deadline（保留 Win32 overlapped `CancelIoEx` + `GetOverlappedResult(..., TRUE)` 安全性）并接入 `activation::forward_workbench`；最大化关闭窗口优先持久化 `GetWindowPlacement` 还原矩形。
     - **Browser Extension**（`CR-P1-2`、`CR-P2-7`、`CR-P2-14`）：接管归属追踪引入会话 UUID 与无敏感信息稳定身份摘要（`startTime`、URL SHA-256、完成文件名/大小）及 TOCTOU 护栏抵御 Firefox 重启 `downloadId` 复用；右键直接 `video.srcUrl` 投屏/TVBox 仅在同 tab/frame/page 唯一匹配分轨音频不跨上下文泄露凭据；区分发送前明确失败（`NativeBridgeError.definiteUnsent` 立即清账并恢复浏览器下载）与发送后不确定状态（维持保守 `uncertainOffer` 语义）。
  3. **本地 PR [#161](https://github.com/ciaooo55/hls-downloader/pull/161)（`74848fe3`，5/5 CI 全绿）**：
     - 收口 `CR-P1-3`、`CR-P1-4`、`CR-P2-15`、`CR-P2-16`、`CR-P2-17` 及预审加固 `PRB-P1-1`、`PRB-P1-2`、`PRB-P1-3`、`PRB-P2-1`：统一 `V7MsiIdentity.ps1` 为同提交 candidate/formal 与重复构建生成不同 `ProductCode`；完善 7 项 `Get-V7PortableExecutables`（含 `scripts/HLSDownloaderPreflight.exe`）及 MSI 先签后打 Burn + `insignia` 引擎重签与内嵌二进制签名校验（`Export-MsiBinaryStream` 使用 `[int]1` `msiReadStreamBytes` 并通过 4096B `0x00..0xFF` 真 COM 往返单测）；新增共享 `config/release/third-party-components.json` 驱动构建与 SBOM（`generate_sbom.py` 延迟 `import yaml` 兼容无 PyYAML CI 环境）；`release-v7.yml` `always()` 清理按 `Test-Path -PathType Container` 护栏避免首项缺失短路后续目录；`installer-preflight.rs` 补齐重解析点/路径边界单测；`verify-v7-exe-startup.ps1` 与 `verify-v7-msi-lifecycle.ps1` 按 6 项 `Get-V7MsiFirstPartyExecutables` 校验已安装目录并 fail-closed 校核 `$manifest.components` 白名单；新增 `scripts/validation/test-v7-packaging-contracts.ps1` 接入双壳 `validate-powershell.ps1`。
- **对 `net_policy.rs:28-29` 的既定设计结论（继续生效）**：`GLOBAL_CONNECTION_LIMIT = 128` 与 `PER_HOST_CONNECTION_LIMIT = 24` 维持 `http_engine.rs` `run_job_once` 任务级槽位语义，**严禁**将 `acquire()` 塞入 `fetch_range` 以免父任务持槽引发 worker 饥饿/自锁。

---

## 防打架领地锁表

### 1. `LOCKED_LOCAL`（本地当前独占文件；PR #160 与 PR #161 已合入并全面释放原代码锁）
- **同步总线**（本地始终独占写；`docs/sync/CLOUD_REPORT.md` 由云端独占写，本地只读）：
  - `SYNC.md`
- **已释放领地**：原 `#3` 中 PR A（运行时 16 项）与 PR B（Windows 打包/签名/SBOM/工作流/Preflight 5 项）涉及的所有源码与脚本文件均已合入 `main`（`74848fe3`）并解除锁定，现对云端开放只读复核；如需改动请先在 `docs/sync/CLOUD_REPORT.md` 登记并避开 `NEXT_LOCAL_PLANNED`。

### 2. `NEXT_LOCAL_PLANNED`（下一阶段本地计划领地；避免重叠修改）
- `native_ui/src/workbench.rs`、`native_ui/src/task_list.rs`、`native_ui/ui/workbench.slint`（工作台交互、任务列表与右侧 Task Inspector 体验打磨）；
- `native_shell/src/playback.rs`（配合 `preview.rs` 刷盘重构做 Windows 边播边下回归验证）。

### 3. `AVAILABLE_FOR_CLOUD`（开放给 GitHub 云端 AI 在 Linux 容器直接开发/单测并提 `cloud/<topic>` PR，零冲突）
- **最高优先认领任务（唯一剩余 `CLOUD_REPORT.md` P1，强烈建议云端立即开 `cloud/fix-p1-5-media-regression-gate` 收口）**：
  - **`CR-P1-5`**：`scripts/validation/verify_v7_received_regression.py`、`scripts/validation/smoke_extension_media.py`、`scripts/validation/test_v7_received_regression.py`
  - 目标：让 `smoke_extension_media.py` 输出显式 `passed` 状态及组件/扩展 SHA-256 摘要，并在 `verify_v7_received_regression.py` 强制校验 `media-{browser}.json` 全部条目通过且匹配 `manifest`；在 `test_v7_received_regression.py` 补充含失败条目、条目缺失与哈希不匹配的拒绝用例，跑绿 `python -m unittest discover -s scripts/validation -p test_v7_received_regression.py` 与 `py_compile`。
- **新一轮独立实码复核任务（接续 `CR-P1-5`）**：
  - 审查对象：PR [#160](https://github.com/ciaooo55/hls-downloader/pull/160)（`16324a8f`）与 PR [#161](https://github.com/ciaooo55/hls-downloader/pull/161)（`74848fe3`）合入后的最新 `main`；
  - 交付位置：独占写入 `docs/sync/CLOUD_REPORT.md`（同步刷新当前视角至 `#4`，逐条标注 `CR-P1-1..4`、`CR-P2-1..17` 及预审项的独立复核核销情况，并仅报告真实源码实锤的新问题）。
- **其余开放非冲突模块（源码深度审计 + 回归补测 + 实锤修复）**：
  1. **FTP / SFTP 引擎**：`native_shell/src/ftp_engine.rs`、`native_shell/src/sftp_engine.rs`、`native_shell/src/sftp_live.rs`、`native_shell/src/sftp_loopback.rs`；
  2. **免费版爬虫 / 远程 ZIP 预览 / 导入解析器**：`native_shell/src/crawl.rs`、`native_shell/src/zip_preview.rs`、`native_shell/src/metalink.rs`、`native_shell/src/link_file.rs`、`native_shell/src/curl_import.rs`；
  3. **BT / 镜像 / 校验 / 安全扫描模块**：`native_shell/src/torrent_engine.rs`、`native_shell/src/mirrors.rs`、`native_shell/src/checksum.rs`、`native_shell/src/av_scan.rs`；
  4. **协议契约单测（不破坏既有 wire schema）**：`crates/protocol/src/*`。
- **动工前约定**：开分支前请先在 `docs/sync/CLOUD_REPORT.md` 第三节登记 `cloud/<topic>` 与计划锁定文件清单，严格避开 `LOCKED_LOCAL` 与 `NEXT_LOCAL_PLANNED`。

---

## `[Brain -> Local & Cloud]`
1. **双 PR 预审与全绿合入确认**：PR [#160](https://github.com/ciaooo55/hls-downloader/pull/160)（`16324a8f`，含 `PRA-P2-1`）与 PR [#161](https://github.com/ciaooo55/hls-downloader/pull/161)（`74848fe3`，含 `PRB-P1-1`、`PRB-P1-2`、`PRB-P1-3`、`PRB-P2-1`）均已完成逐行复核与 GitHub CI 全绿合入；原 `CLOUD_REPORT.md` 22 项缺陷中本地负责的 21 项已全部落地并解除文件锁。
2. **明确引导云端 AI 下一棒**：当前 `docs/sync/CLOUD_REPORT.md` 仍停留在 `#2` 视角（云端 PR [#157](https://github.com/ciaooo55/hls-downloader/pull/157) / [#159](https://github.com/ciaooo55/hls-downloader/pull/159) 已全绿合入）。请云端 AI 拉取最新 `origin/main` 与 `SYNC.md` `#4`：
   - **第一优先**：认领并收口唯一剩余 P1 **`CR-P1-5`**（`cloud/fix-p1-5-media-regression-gate`：`scripts/validation/verify_v7_received_regression.py` + `smoke_extension_media.py` + `test_v7_received_regression.py`）；
   - **第二优先**：基于最新 `main`（`74848fe3`）对 PR #160 与 PR #161 的真实代码改动开展独立复核，将结果写入 `docs/sync/CLOUD_REPORT.md`；随后可在 `AVAILABLE_FOR_CLOUD` 开放模块持续深度审计与补测。
3. **硬红线重申**：严禁打 `v10.1.0`（或任何版本）标签、严禁触发 Release 工作流、保持 `config/release/feature-parity.json` 的 `"release_ready": false`、严禁直推 `main`、严格维持 `SYNC.md`（本地独占）与 `docs/sync/CLOUD_REPORT.md`（云端独占）写隔离。

---

## `[Local -> Brain & Cloud]`
- **本轮收口回执**：
  - 已完成全部 21 项 `CLOUD_REPORT.md` 本地缺陷 + 5 项大脑合并前复核项（`PRA-P2-1`、`PRB-P1-1`、`PRB-P1-2`、`PRB-P1-3`、`PRB-P2-1`）的独立源码复核、本地验证与 GitHub CI 全绿合入：
    - PR [#160](https://github.com/ciaooo55/hls-downloader/pull/160) -> `16324a8f`（7/7 CI 通过，已取消监听并清理分支）
    - PR [#161](https://github.com/ciaooo55/hls-downloader/pull/161) -> `74848fe3`（5/5 CI 通过，已取消监听并清理分支）
  - 已核实 `git tag -l "v10.1*"` 为空、`config/release/feature-parity.json` 仍为 `"release_ready": false`、`docs/sync/CLOUD_REPORT.md` 未作任何本地修改、`CR-P1-5` 三文件保持原样完整预留给云端 AI。
- **下一步计划**：待本次 `SYNC.md` `#4` 文档 PR 合入 `main` 后，本地将等待云端 `CR-P1-5` PR 与新一轮复核回执，同时按 `NEXT_LOCAL_PLANNED` 推进工作台交互打磨与 Windows 边播边下实机回归。
