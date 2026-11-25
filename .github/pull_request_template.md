## 变更摘要

<!-- 简要说明本 PR 解决的问题、设计取舍及关联的 Issue（例如：Closes #123） -->

- 

## 影响模块

- [ ] `native_shell`（Rust Core 守护引擎、SQLite、传输协议、FFmpeg 混流、播放/投屏代理）
- [ ] `native_ui`（Slint 桌面工作台、右侧任务检视抽屉、确认/进度/完成弹窗、设置中心）
- [ ] `crates/protocol` · `crates/client` · `native_clients`（IPC 协议、DPAPI 凭据、Native Host、CLI、Updater）
- [ ] `extension`（WXT Chromium / Firefox Manifest V3 浏览器扩展）
- [ ] `scripts` · `config` · `docs`（构建/验证脚本、功能契约、文档与模板）

## 验证记录

<!-- 请勾选已在本地执行并通过的检查项，必要时贴出关键测试输出 -->

- [ ] `cargo fmt --all -- --check`（`native_shell` / `native_ui`）
- [ ] `cargo clippy ... -- -D warnings`（与 `.github/workflows/ci.yml` 规则一致，零新增警告）
- [ ] `cargo test`（`native_shell`、`hls-protocol`、`hls-client`、`hls-native-clients`、`native_ui`）
- [ ] `pnpm --dir extension test` 与 `pnpm --dir extension run build`（如涉及 `extension/`）
- [ ] 仓库契约门禁（`validate-powershell.ps1`、`verify-native-boundaries.ps1`、`verify-v7-feature-parity.ps1`、`verify-v7-version-contract.ps1`）

## 安全与边界自检

- [ ] **Unsafe 契约**：所有新增或修改的生产 `unsafe` 块与 `unsafe impl` 均附带准确、可核查的 `// SAFETY:` 不变量说明（符合 `clippy::undocumented_unsafe_blocks` 门禁）。
- [ ] **无生产裸 `.unwrap()`**：生产代码路径未引入未受保护的 `.unwrap()` / `.expect()`（互斥锁采用 poison-safe 处理，外部输入与 I/O 显式处理 `Result` 或回退）。
- [ ] **凭据、代理与依赖边界**：未在代码、测试夹具、日志或 PR 描述中泄露真实 `Cookie` / Token / 密钥，未硬编码个人用户目录路径，且遵守代理 fail-closed、DPAPI 加密存储与 `verify-native-boundaries.ps1` 客户端依赖隔离契约。
