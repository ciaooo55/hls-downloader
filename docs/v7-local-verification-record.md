# v7 本机验证记录（2026-09-19，本地 `main`）

本文记录在**本机 Windows**（不使用 WSL）上对当前 `main` 实际执行的验证命令与真实结果，
并明确区分"本机已验证"与"仍需实机/操作者"的边界。未执行的项一律标注为未执行，
不得据此声称通过。

- 受测源码：`main` @ `2c634c6`（本轮验证前 HEAD）
- 受测产品版本：canonical `artifacts/v7-productization/feature-parity.json.product_version` = `7.0.2`
- canonical 状态：27 verified / 1 partial（`browser.media_push_device_selection`）/ `release_ready=false`
 - Rust 工具链：**先按仓库 pin 安装 Rust 1.98.1**（`.github/workflows/ci.yml:103`、
   `rust-security.yml:35` 均 pin `1.98.1`），下表的 Rust 命令均以 `cargo +1.98.1 ...` 运行，
   与 CI 保持一致；本机默认 stable 为 1.97.1，此前一轮曾用默认工具链跑通，结论相同。
 - 其他工具链：Node v24.14.0、pnpm 11.7.0（未在 PATH，改用 `extension\\node_modules\\.bin`）、
   Microsoft OpenJDK 21.0.12.1（scratch 内临时解压，`JAVA_HOME` 指向该目录）、
   Windows PowerShell 5.1、PowerShell 7.6.6

## 实机执行结果（本机 Windows）

| # | 命令 | 真实结果 |
 | --- | --- | --- |
 | 1 | `cargo +1.98.1 test --manifest-path native_shell/Cargo.toml --locked --lib` | `464 passed; 0 failed; 1 ignored`，退出码 0 |
 | 2 | `cargo +1.98.1 test --manifest-path presenter_ui/Cargo.toml --locked` | `9 passed; 0 failed`，退出码 0 |
 | 3 | `cargo +1.98.1 clippy --manifest-path native_shell/Cargo.toml --locked --all-targets -- -D warnings -A dead_code -A clippy::large_enum_variant -A clippy::too_many_arguments -A clippy::type_complexity`（与 `.github/workflows/ci.yml:129` 参数一致） | 无告警，退出码 0 |
 | 3b | `cargo +1.98.1 clippy --manifest-path presenter_ui/Cargo.toml --locked --bin hls-downloader-presenter -- -D warnings -A clippy::while_let_loop`（与 `ci.yml:164` 参数一致） | 无告警，退出码 0 |
 | 3c | `cargo +1.98.1 fmt --manifest-path native_shell/Cargo.toml --all -- --check` / `--manifest-path presenter_ui/...` | 均无 diff，退出码 0 |
| 4 | `desktop_ui\gradlew.bat test --no-daemon --rerun-tasks`（`JAVA_HOME=<scratch>\jdk21`） | `BUILD SUCCESSFUL in 1m 15s`，13 个测试类共 71 个用例，0 failures / 0 errors / 0 skipped（`--rerun-tasks` 强制真实执行，非缓存） |
| 5 | 扩展：`wxt prepare` → `tsc --noEmit` → `vitest run` → `wxt build -b chrome` → `wxt build -b firefox`（经 `extension\node_modules\.bin`，等价 `pnpm test` / `pnpm run build`） | 全部退出码 0；`vitest` 43 文件 / 301 用例通过；chrome-mv3 生成、249.58 kB；firefox-mv3 生成、249.35 kB |
| 6 | `validate-powershell.ps1`（PowerShell 5.1 与 PowerShell 7.6.6 各一次） | 均退出码 0：45 个脚本解析通过；三方 gate-id 契约通过；版本契约 `7.0.2 / release_ready=False / v7_0_2_iteration_in_progress` 同步，MSI 生命周期不一致检查 fail closed |
| 7 | gate-id 三方一致性（脚本抽取比对） | `invoke-v7-release-gates.ps1`、`record-v7-release-gate.ps1` 的 `ValidateSet`、`verify-v7-feature-parity.ps1` 的 `$requiredGateIds` 三者均为同一集合 `browser / performance / browser_media_push / installer / rollback`（5 项） |

第 4 项首次执行时 `:test` 为 `UP-TO-DATE`（Gradle 缓存），不足以作为"实际执行"证据，
故用 `--rerun-tasks` 强制重跑并以 `build/test-results/test/TEST-*.xml` 逐类核对用例数与失败数。

## 本轮未执行（不得视为通过）

- **正式门禁五项实机证据**：`browser`、`performance`、`browser_media_push`、`installer`、`rollback` 均未在本机执行。
- **正式打包 / 签名 / 发布**：未运行 `build-v7.ps1 -Task package`、`release-v7.yml`、任何 Authenticode 签名或 Draft Release 流程。
- **实机安装与真实浏览器 / 局域网 TVBox**：本机无 `E:\h` 安装根、无已安装 Edge/Firefox 的 Native Messaging 注册验证、无真实 TVBox 接收端，`browser.media_push_device_selection` 维持 `partial`。
- **四个 exact-SHA 前置工作流**（`v7 CI`、`v7 Candidate Package`、`Maintenance Security`、`Rust Security`）：本机无法触发，需在 GitHub 侧对冻结 `main` SHA 运行。

## 边界结论

本机可完成的部分（Rust 单元测试、clippy/fmt 静态检查、Compose 测试、扩展测试与双目标构建、
PowerShell 双版本校验、仓库与门禁契约核对）均已真实执行且通过，源码可编译、可测试、契约自洽。
正式发布仍被**本机之外**的条件卡住，见 `docs/v7-release-runbook-local.md` 与
`docs/v7-release-runner.md`：`release_ready=false` 保持不变，`browser.media_push_device_selection`
在本机无实机证据前不得标记为 verified。
