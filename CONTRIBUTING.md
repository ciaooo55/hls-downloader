# 贡献指南 (Contributing to HLS Downloader)

感谢关注与参与 **HLS Downloader** 的开发。本文档面向维护者与外部贡献者，说明本仓库的架构边界、固定工具链版本、本地构建与 CI 验证命令、工程安全红线以及 Pull Request 流程。

在提交代码或 Issue 前，请先阅读 [README.md](README.md)、[文档索引](docs/README.md) 与 [安全政策](SECURITY.md)。

---

## 1. 架构概览与模块边界

HLS Downloader 采用 **常驻后台引擎 + 按需原生界面 + 浏览器 Native Messaging 扩展** 的多进程协作架构：

| 模块目录 | Crate / 包名 | 核心职责与所有权边界 |
| --- | --- | --- |
| `native_shell/` | `hls-native-shell` | **常驻 Rust Core 守护引擎**。SQLite（WAL 模式）任务数据库、活动传输状态机（HTTP/HTTPS、HLS、DASH、FTP/FTPS、SFTP、BitTorrent）、FFmpeg 混流、本地播放代理与局域网投屏服务的**唯一持有者**；通过 Win32 Job Object 托管外部子进程生命周期。 |
| `native_ui/` | `hls-native-ui` | **按需启动的 Slint 原生桌面端（Presenter）**。承载主工作台、右侧非模态任务检视抽屉、新建/批量任务窗、确认/双模式进度/完成弹窗、设置中心与播放器界面。**严禁直接链接或读写 SQLite 与传输引擎**，一切状态读写均通过 Overlapped Named Pipe IPC 与 Core 通信。 |
| `crates/protocol/` | `hls-protocol` | 版本化 v7 IPC 协议契约、任务/抓站数据结构与 4 字节小端帧 JSON 编解码（仅依赖 `serde` 系列，保持极简依赖图）。 |
| `crates/client/` | `hls-client` | 共享轻量客户端基础设施：命名管道传输与重连、Core 按需拉起、单实例锁、配置与便携版路径解析、Windows DPAPI 凭据加密保险库（`credential_vault.rs`）、窗口与剪贴板/OLE 辅助。 |
| `native_clients/` | `hls-native-clients` | 轻量原生客户端入口：浏览器 `Native Messaging Host`、命令行工具（`HLSDownloaderCli`）、自动更新校验器（Updater）与启动器。生产依赖图不得引入 `hls-native-shell`、`rusqlite` 或 `russh`。 |
| `extension/` | `hls-downloader-extension` | 基于 **WXT** 构建的 Chromium（Chrome / Edge）与 Firefox **Manifest V3** 浏览器扩展，负责网页流媒体识别、普通 GET 下载直连接管、一次性 `POST`/`Blob` 响应安全保全与按站点显式授权的 Cookie 移交。 |
| `scripts/` | — | Windows 构建、媒体工具准备、安装包制作与跨模块自动化验证门禁脚本（详见 [scripts/README.md](scripts/README.md)）。 |

更多架构细节请参阅 [运行架构](docs/architecture/v7-architecture.md) 与 [Workspace 与 IPC 契约](docs/architecture/native-workspace.md)。

---

## 2. 开发环境与固定工具链

本项目面向 **Windows 10 / Windows 11 x64** 原生构建（无需安装 Java / JDK）。本地工具链请与 [`.github/workflows/ci.yml`](.github/workflows/ci.yml) 保持一致：

- **操作系统**：Windows 10 / 11（64 位 `x64`），安装 Visual Studio C++ Build Tools（MSVC 工具集与 Windows SDK）
- **Rust 工具链**：`1.98.1`（`x86_64-pc-windows-msvc`，需包含 `rustfmt` 与 `clippy` 组件）
  ```powershell
  rustup toolchain install 1.98.1 --component rustfmt,clippy
  rustup default 1.98.1
  ```
- **Node.js**：`24.20.0`
- **包管理器**：`pnpm@11.7.0`（通过 Corepack 激活）
  ```powershell
  corepack enable
  corepack prepare pnpm@11.7.0 --activate
  pnpm --version # 应输出 11.7.0
  ```
- **脚本环境**：Windows PowerShell 5.1（`powershell.exe`）、PowerShell 7+（`pwsh.exe`）与 Python 3（用于自动化契约与回归测试脚本）

---

## 3. 本地构建与测试命令

所有 CI 门禁均可在仓库根目录或对应子目录下本地复现。你也可以使用统一入口 `.\scripts\build-v7.ps1 -Task test` 执行集成检查，或用 `.\scripts\run-v7-local.ps1` 启动本地开发工作台。

### 3.1 Rust Core 与原生客户端（对应 CI `rust_core` 任务）

```powershell
Push-Location native_shell
try {
    cargo fmt --all -- --check
    # 播放器相关测试会切换进程级环境变量钩子，需串行执行避免交叉污染
    cargo test --locked --lib --tests -- --test-threads=1
    cargo test --locked -p hls-protocol -p hls-client -p hls-native-clients -- --test-threads=1
    cargo build --locked -p hls-native-clients --bins
    ../scripts/validation/verify-native-boundaries.ps1
    cargo clippy --locked --all-targets -- -D warnings -A clippy::large_enum_variant -A clippy::too_many_arguments -A clippy::type_complexity
    cargo clippy --locked -p hls-protocol -p hls-client -p hls-native-clients --all-targets -- -D warnings
} finally {
    Pop-Location
}
```

### 3.2 Slint 原生桌面工作台与弹窗（对应 CI `presenter` 任务）

```powershell
Push-Location native_ui
try {
    cargo fmt --all -- --check
    cargo test --locked
    cargo build --locked --bin hls-downloader-presenter
    cargo clippy --locked --bin hls-downloader-presenter -- -D warnings -A clippy::while_let_loop
} finally {
    Pop-Location
}
```

### 3.3 Chromium / Firefox 浏览器扩展（对应 CI `extension` 任务）

```powershell
Push-Location extension
try {
    pnpm install --frozen-lockfile
    pnpm test
    pnpm run build
} finally {
    Pop-Location
}
```

### 3.4 PowerShell 与 Python 仓库契约门禁（对应 CI `validate` 任务）

在仓库根目录执行：

```powershell
# PowerShell 5.1 / 7 语法、脚本卫生、版本契约与功能映射校验
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/validation/validate-powershell.ps1
pwsh.exe -NoProfile -ExecutionPolicy Bypass -File scripts/validation/validate-powershell.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/release/verify-v7-feature-parity.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/release/verify-v7-version-contract.ps1

# 打包、升级与媒体推送脚本回归检查
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/validation/test-v7-release-gap-contract.ps1 -Shell powershell.exe
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/validation/test-v7-received-msi.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/validation/test-v7-media-push-wrapper.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/validation/test-v7-portable-upgrade.ps1 -Shell powershell.exe
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/validation/test-v7-script-hygiene.ps1 -Shell powershell.exe

# Python 验证工具语法与单元测试
Get-ChildItem -Path scripts -Filter *.py -File -Recurse | ForEach-Object {
    python.exe -m py_compile $_.FullName
}
python.exe -m unittest discover -s scripts/validation -p test_v7_core_ipc.py
python.exe -m unittest discover -s scripts/validation -p test_v7_received_regression.py
python.exe -m unittest discover -s scripts/validation -p test_native_soak_resources.py
python.exe -m unittest discover -s scripts/validation -p test_native_tvbox_ui.py
python.exe -m unittest discover -s scripts/validation -p test_native_workbench_ui.py
```

---

## 4. 工程质量与安全红线

提交到本仓库的代码必须满足以下硬性规范：

1. **Clippy 零警告（`-D warnings`）与 `rustfmt` 一致性**
   - 所有新增与修改的 Rust 代码必须通过 `cargo fmt --all -- --check`，并在上述 CI Clippy 规则下保持零警告，不得随意添加宽泛的 `#[allow(...)]` 掩盖问题。
2. **生产 `unsafe` 契约强制注释**
   - 生产 Crate 启用 `#![cfg_attr(not(test), warn(clippy::undocumented_unsafe_blocks))]` 门禁。
   - 每一处生产代码中的 `unsafe { ... }` 块与 `unsafe impl` 都**必须**紧邻编写准确、可核查的 `// SAFETY:` 注释，明确说明指针有效性、所有权生命周期、Win32 句柄状态或并发同步不变量。
3. **生产路径零裸 `.unwrap()`**
   - 严禁在非测试的生产代码路径中对外部输入、网络响应、IPC 消息、数据库查询、文件路径或系统调用使用未受保护的 `.unwrap()` / `.expect()`。
   - 标准库互斥锁应使用抗毒化恢复模式（`lock().unwrap_or_else(|e| e.into_inner())`）；可失败操作应通过 `Result`、`?`、显式降级回退或可诊断错误向上返回。
4. **安全与架构隔离边界**
   - **代理路由严格 Fail-Closed**：HTTP CONNECT、SOCKS4/5 及系统代理握手或鉴权失败时必须立即中止连接并报错，严禁静默回退为直连从而泄露客户端真实网络出口（详见 [代理路由策略](docs/architecture/proxy-routing.md)）。
   - **DPAPI 凭据加密与日志脱敏**：Cookie、Authorization、代理密码、SFTP 凭据与云端字幕 API Key 必须通过 `crates/client/src/credential_vault.rs`（Windows DPAPI）加密落盘，UI 与日志绝不能回显或打印明文凭据。
   - **浏览器扩展按站点显式授权 Cookie**：扩展仅在用户明确对目标站点点击授权后才可读取并随任务移交该站点作用域内的 Cookie；严禁实现任何绕过 DRM（Widevine / PlayReady / EME / SAMPLE-AES）或规避访问控制的逻辑。
   - **客户端依赖图隔离**：`hls-protocol`、`hls-client`、`hls-native-clients` 与 `hls-native-ui` 的生产依赖中严禁混入 `hls-native-shell`、`rusqlite`、`libsqlite3-sys`、`russh` 或 `russh-sftp`（由 `scripts/validation/verify-native-boundaries.ps1` 强制校验）。
   - **脚本与路径卫生**：`scripts/` 下任何脚本严禁硬编码开发者个人用户目录路径；构建缓存应尊重 `$env:HLS_V7_BUILD_CACHE`（默认位于工作区 `.tool-cache`）。跨模块版本号变更须统一通过 `scripts/release/bump-v7-version.ps1` 同步，切勿手工只改单个清单文件。

---

## 5. Pull Request 与协作流程

1. **确认范围**：先检查已有 Issues / PRs。若涉及安全漏洞（如本地 IPC 越权、Native Messaging 注入、凭据泄露等），**切勿创建公开 Issue 或公开 PR**，请按 [SECURITY.md](SECURITY.md) 通过 GitHub **Security → Report a vulnerability** 私下报告。
2. **创建分支**：从最新 `main` 创建语义清晰的分支（如 `fix/dash-segment-retry`、`feat/inspector-filter`、`docs/update-guide`），保持单个 PR 职责单一、变更面可控。
3. **补充测试与验证**：
   - 修复 Bug 或调整协议/状态机时，请在对应模块补充可离线复现的单元测试或回归测试。
   - 如修改了 `config/release/feature-parity.json` 引用的测试函数名，请同步更新并运行 `verify-v7-feature-parity.ps1` 确保测试证据引用全部可解析。
4. **提交 PR**：按 [Pull Request 模板](.github/pull_request_template.md) 完整填写变更摘要、影响模块、本地验证记录与安全自检清单，确保所有 GitHub Actions CI 检查绿灯通过。
