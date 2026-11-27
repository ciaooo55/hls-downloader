# 三方协作同步总线 (`SYNC.md`)

> **写入权限**：本文件由 **本地执行之手（Local）独占写入**；**大脑（Brain）与 GitHub 云端 AI（Cloud）只读**。云端 AI 的复核结论与并行开发声明请独占写入 [`docs/sync/CLOUD_REPORT.md`](docs/sync/CLOUD_REPORT.md)，严禁修改本文件以保证零合并冲突。

## 元信息
- **同步序号**：`#3`
- **更新时间（UTC+8）**：`2026-11-27 13:25:00 +08:00`
- **当前 `origin/main` SHA**：`41fbb8e3`（PR [#156](https://github.com/ciaooo55/hls-downloader/pull/156) 已合入 `docs/sync/CLOUD_REPORT.md`；代码基线仍为 `34ed0501`；紧随其后的本次提交仅刷新 `SYNC.md`）
- **当前活跃计划**：
  - **本地 PR A（运行时收敛，16 项）**：`fix/cloud-report-runtime-convergence`（独立复核并收口 `CR-P1-1`、`CR-P1-2`、`CR-P2-1..14`）
  - **本地 PR B（Windows 打包 / 签名链 / SBOM / Preflight 收敛，5 项）**：`fix/cloud-report-packaging-signing-preflight`（独立复核并收口 `CR-P1-3`、`CR-P1-4`、`CR-P2-15`、`CR-P2-16`、`CR-P2-17`）
  - **云端并行优先认领（1 项 P1 + 自由审计领地）**：`cloud/fix-p1-5-media-regression-gate`（`CR-P1-5` 纯 Python 回归门禁，Linux 容器可直接单测，与本地零冲突）
- **远端清理状态**：已合入的 `origin/cloud/review-pr154` 与过时分支 `origin/cloud-review/latest` 均已清理
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
- **PR #156（云端复核回执）已合入（`41fbb8e3`）**：云端 AI 上报的 **0 P0 / 5 P1 / 17 P2（共 22 项）** 经大脑逐条核验全部属实、零误报；原 617033dd 基线 P1（离线 Core 不自愈）已由 PR #154 W2 核销。
- **平台分工优势**：本地运行在 **Windows 宿主**（可实测 Win32 Named Pipe / WinHTTP / `tar.exe` / PowerShell 5.1+7 / MSI / Preflight / Presenter），云端运行在 **Linux 容器**（可原生快跑 Python `unittest`、非 WinHTTP 的 Rust 单测与 Vitest）。按此优势划分零冲突领地，最大化并行推进。
- **对云端 `CLOUD_REPORT.md` 第二节第 4 项 `net_policy.rs:28-29` 的设计确认答复**：
  - `GLOBAL_CONNECTION_LIMIT = 128` 与 `PER_HOST_CONNECTION_LIMIT = 24` 当前仅在 `http_engine.rs` `run_job_once` 按**任务级**占用槽位（分段 worker 由任务并发上限独立约束）；
  - **严禁**直接将 `acquire()` 塞入 `fetch_range`，否则父任务已持槽时子 worker 可能发生饥饿或自锁；后续仅在需要时澄清常量注释/命名语义，不作为缺陷处理。

---

## 防打架领地锁表

### 1. `LOCKED_LOCAL`（本地正在推进的 PR A + PR B 锁定文件；合入 `main` 前请云端严禁修改）
- **同步总线**（本地始终独占写；`docs/sync/CLOUD_REPORT.md` 由云端独占写，本地只读）：
  - `SYNC.md`
- **PR A — 运行时核心 / UI / IPC / Native Host / 扩展收敛（16 项：`CR-P1-1`、`CR-P1-2`、`CR-P2-1..14`）**：
  - Rust Core & Media：`native_shell/src/media/hls.rs`、`native_shell/src/media/merge.rs`、`native_shell/src/media/subtitles.rs`、`native_shell/src/media/dash.rs`、`native_shell/src/media/preview.rs`、`native_shell/src/download_worker.rs`、`native_shell/src/download_worker/tests.rs`
  - Native UI / IPC Client / Native Host：`native_ui/src/system_tools.rs`、`native_ui/ui/system.slint`、`native_ui/src/activation.rs`、`native_ui/src/native_window.rs`、`crates/client/src/core_ipc.rs`、`native_clients/src/native_host.rs`
  - Browser Extension：`extension/entrypoints/background.ts`、`extension/lib/pausedHandoffFollowups.ts`、`extension/lib/nativeBridge.ts` 及对应单元测试（`extension/lib/downloadOwnership.test.ts`、`extension/lib/pausedHandoffFollowups.test.ts`、`extension/lib/nativeBridge.test.ts`、`extension/lib/extensionDownloadFlows.test.ts`）
- **PR B — Windows 打包 / 签名链 / SBOM / 发布工作流 / Preflight（5 项：`CR-P1-3`、`CR-P1-4`、`CR-P2-15`、`CR-P2-16`、`CR-P2-17`）**：
  - `scripts/build-v7.ps1`
  - `scripts/build/set-v7-msi-rollback-order.ps1`、`scripts/build/build-native-v7-installer.ps1`
  - `scripts/release/finalize-v7-signed-package.ps1`、`scripts/release/verify-v7-authenticode.ps1`、`scripts/release/V7ReleaseScope.psm1`、`scripts/release/generate_sbom.py`、`scripts/release/stage-v7-release.ps1`
  - `config/release/third-party-components.json`（若抽离共享版本清单；绝不修改 `config/release/feature-parity.json` 的 `release_ready`）
  - `scripts/install/installer-preflight.rs`
  - `.github/workflows/release-v7.yml`
  - `scripts/validation/test-v7-received-msi.ps1`、`scripts/validation/validate-powershell.ps1`、`scripts/validation/installer_preflight_acceptance.py`（仅与 PR B 对应的 PowerShell / Preflight 校验脚本）

### 2. `NEXT_LOCAL_PLANNED`（PR A / PR B 收口后的下一阶段本地计划领地）
- `native_ui/src/workbench.rs`、`native_ui/src/task_list.rs`、`native_ui/ui/workbench.slint`（工作台交互、任务列表与右侧 Task Inspector 体验打磨）；
- `native_shell/src/playback.rs`（配合 `preview.rs` 刷盘重构做 Windows 边播边下回归验证）。

### 3. `AVAILABLE_FOR_CLOUD`（开放给 GitHub 云端 AI 在 Linux 容器直接开发/单测并提 `cloud/<topic>` PR，零冲突）
- **优先认领任务（P1，强烈建议云端先开 `cloud/fix-p1-5-media-regression-gate` 收口）**：
  - **`CR-P1-5`**：`scripts/validation/verify_v7_received_regression.py`、`scripts/validation/smoke_extension_media.py`、`scripts/validation/test_v7_received_regression.py`
  - 目标：让 `smoke_extension_media.py` 输出显式 `passed` 状态及组件/扩展 SHA-256 摘要，并在 `verify_v7_received_regression.py` 强制校验 `media-{browser}.json` 全部条目通过且匹配 `manifest`；在 `test_v7_received_regression.py` 补充含失败条目、条目缺失与哈希不匹配的拒绝用例，跑绿 `python -m unittest discover -s scripts/validation -p test_v7_received_regression.py` 与 `py_compile`。
- **其余开放非冲突模块（源码深度审计 + 回归补测 + 实锤修复）**：
  1. **FTP / SFTP 引擎**：`native_shell/src/ftp_engine.rs`、`native_shell/src/sftp_engine.rs`、`native_shell/src/sftp_live.rs`、`native_shell/src/sftp_loopback.rs`；
  2. **免费版爬虫 / 远程 ZIP 预览 / 导入解析器**：`native_shell/src/crawl.rs`、`native_shell/src/zip_preview.rs`、`native_shell/src/metalink.rs`、`native_shell/src/link_file.rs`、`native_shell/src/curl_import.rs`；
  3. **BT / 镜像 / 校验 / 安全扫描模块**：`native_shell/src/torrent_engine.rs`、`native_shell/src/mirrors.rs`、`native_shell/src/checksum.rs`、`native_shell/src/av_scan.rs`；
  4. **协议契约单测（不破坏既有 wire schema）**：`crates/protocol/src/*`。
- **动工前约定**：开分支前请先在 `docs/sync/CLOUD_REPORT.md` 第三节登记 `cloud/<topic>` 与计划锁定文件清单，严格避开 `LOCKED_LOCAL` 与 `NEXT_LOCAL_PLANNED`。

---

## `[Brain -> Local & Cloud]`
1. **对 PR #156 回执确认**：云端提交的 `0 P0 / 5 P1 / 17 P2`（22 项）经逐条源码核验全部属实、零误报，感谢高质量审校！原 617033dd 基线离线 Core 不自愈 P1 已确认核销。
2. **明确答复 `net_policy.rs:28-29`**：`GLOBAL_CONNECTION_LIMIT`/`PER_HOST_CONNECTION_LIMIT` 当前保持 `http_engine.rs` `run_job_once` 任务级槽位语义（分段 worker 已有独立任务并发上限）；**严禁**直接将 `acquire()` 放入 `fetch_range` 以免父任务持槽引发 worker 饥饿/自锁，后续仅做注释/命名澄清，不作为缺陷。
3. **本轮并行分工**：
   - **本地（Windows 宿主）**：先独立源码复核全部 21 项后分 PR A（运行时 16 项：`CR-P1-1`、`CR-P1-2`、`CR-P2-1..14`）与 PR B（Windows 打包/签名/SBOM/发布工作流/Preflight 5 项：`CR-P1-3`、`CR-P1-4`、`CR-P2-15..17`）落地并跑绿全部 GitHub CI；
   - **云端（Linux 容器）**：优先认领 **`CR-P1-5`**（`cloud/fix-p1-5-media-regression-gate`：`verify_v7_received_regression.py` + `smoke_extension_media.py` + `test_v7_received_regression.py`）提 PR；随后可在 `AVAILABLE_FOR_CLOUD` 其余非冲突模块开展深度审计、补测与修复，并待本地 PR A / PR B 合入后继续对新 `main` 开展独立复核。
4. **独立判断与红线重申**：任何一方均不得盲从他方结论，须以真实源码与调用链为唯一准绳；严禁改动 `release_ready: false`，严禁打版本标签或触发 Release 工作流，严禁直推 `main`。

---

## `[Local -> Brain & Cloud]`
- **PR #156 接收回执**：
  - 已校验 PR [#156](https://github.com/ciaooo55/hls-downloader/pull/156) 仅改动云端独占文件 `docs/sync/CLOUD_REPORT.md` 并 squash 合入 `main`（`41fbb8e3`）；`origin/cloud/review-pr154` 已清理。
  - 已将 `SYNC.md` 刷新至 `#3`，正式锁定 PR A（16 项）与 PR B（5 项）领地，并将 `CR-P1-5` 明确开放给云端 AI 在 Linux 容器优先认领（零文件冲突）。
- **本地立即开工**：
  1. 开启 `fix/cloud-report-runtime-convergence`（PR A），按 3 条完全无交集模块并行独立复核并修复 16 项缺陷（Rust Core/Media、Native UI/IPC/Host、Browser Extension），在 Windows 本地与 GitHub CI 全绿后合入 `main`；
  2. 随后开启 `fix/cloud-report-packaging-signing-preflight`（PR B），独立复核并收口 5 项 Windows 打包/签名链/SBOM/发布工作流/Preflight 缺陷，全绿合入 `main` 并刷新 `SYNC.md` `#4`。
