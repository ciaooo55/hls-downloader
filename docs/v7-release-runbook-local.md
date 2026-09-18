# v7 正式发布运行手册（`hls-release` 机器）

本文是给**操作者**照着执行的步骤清单，机器为携带 `hls-release` 标签的自托管 Windows x64 runner。
本机（开发机）无法完成这些步骤：它们需要签名私钥、已安装的真实浏览器、可交互桌面、真实局域网 TVBox
接收端、`E:\h` 安装根和不可变 `v7.0.0` MSI 基线。

前置环境与 runner 契约以 `docs/v7-release-runner.md` 为准，本文只给执行顺序与放行判据。
当前 canonical `release_ready=false`，因此**本手册不授权创建正式 tag/Release**；
只有在第 3 步完成且 `release_ready=true` 被显式授权后，第 4 步才可用。

## 0. 操作者需先备好

- runner 以**发布用户桌面会话**运行（非 Session-0 服务），桌面保持已登录且不锁屏。
- 环境 `v7-release` 内配置：secret `HLS_V7_SIGN_CERT_THUMBPRINT`；变量 `HLS_V7_SIGN_CERT_STORE`、
  `HLS_V7_TIMESTAMP_URL`、`HLS_V7_TVBOX_EXPECTED_HOST`（真实 TVBox 的**私有 IPv4**）。
- runner 上：Windows SDK `signtool.exe`（或 `HLS_V7_SIGNTOOL`）、可信代码签名证书+私钥、
  已安装 Edge 与 Firefox、Git / `gh` / Python(PyYAML) / Go 在 PATH、可访问 `E:` 卷。
- 该 LAN 上的真实 TVBox 接收服务已开启，且能拉取 runner 提供的确定性媒体 URL。

## 1. 冻结 `main` 并触发四个 exact-SHA 前置工作流

在本地 `main` 推送完成后（**推送需操作者显式授权**，非 fast-forward 被拒时如实报告，不强推）：

```powershell
# 记录冻结 SHA
git rev-parse HEAD
```

对**同一** `main` SHA 确保以下四个工作流成功（由 `scripts/assert-v7-exact-main-workflows.ps1` 绑定名字/路径/事件/SHA）：

1. `v7 CI`（`ci.yml`，`push`）
2. `v7 Candidate Package`（`package-v7-candidate.yml`，`main` 上**手动** `workflow_dispatch`）
3. `Maintenance Security`（`maintenance-security.yml`，`push`）
4. `Rust Security`（`rust-security.yml`，`push`）

候选包工作流只手动触发；先冻结 `main`，再显式跑一次候选包，并确认三个 push 工作流已成功。

## 2. 收集实机 media-push 证据（`release_ready` 仍为 false）

`.github/workflows/v7-media-push-readiness.yml` 是当前唯一被批准的、用于补齐
`browser.media_push_device_selection` 实机证据的通道：

```powershell
gh workflow run v7-media-push-readiness.yml --ref main
```

该工作流会：拒绝非 `main`/过期源码；要求 `release_ready=false`；要求 `HLS_V7_TVBOX_EXPECTED_HOST`；
构建该精确提交的新 candidate；安装 candidate MSI；校验已安装 Edge/Firefox 的 Native Messaging 注册；
在两种浏览器里驱动 生产浏览器 → Native Host → Core → Compose DevicePicker 全链路；要求配置的
TVBox 接收端拉取到确定性媒体 fixture；再次确认远端 `main` 未移动；上传 source/candidate/browser/receiver
绑定的 attestation。

放行判据：attestation 中真实浏览器与真实 TVBox 全链路通过。缺失/非私有/不可发现/错误主机/拉取失败
均判失败，无 skip 回退。

## 3. 提升 canonical 元数据（需显式授权）

`v7-media-push-readiness` 的 attestation **仅**作为窄范围的元数据提升依据，它本身不设置
`release_ready=true`、不建 tag、不签名。

在操作者显式授权后，将 `artifacts/v7-productization/feature-parity.json` 中
`browser.media_push_device_selection.status` 由 `partial` 改为 `verified`，
并把 `release_ready` 改为 `true`；同步 `audit_state`。提交该元数据变更并冻结到同一 `main` SHA。

> 注意：只改 parity 状态或只改文档不会通过正式门禁；正式门禁会重算产物与报告哈希。

## 4. 触发正式发布（需显式授权 + 上述全部通过）

```powershell
gh workflow run release-v7.yml --ref main -f publish=false   # 先不发布
```

工作流会按 `docs/v7-release-runner.md` 顺序：校验四前置工作流 → 校验 runner 并引导工具链 →
构建新 candidate → 记录**五项** candidate 绑定门禁（`browser`、`performance`、`browser_media_push`、
`installer`、`rollback`）→ 复查 `main` 未移动 → `build-v7.ps1 -Task package` 构建正式包 →
Authenticode 签名+时间戳 → 生成 Firefox 源包 / SBOM / 证据包 / `SHA256SUMS.txt` → 上传 → 建/续
annotated tag 与 **Draft** Release → 逐项核对资产字节数与 SHA-256 → 仅在 `publish=true` 时发布 Latest。

放行判据：全部门禁 `passed` 且 `exit_status=0`，签名验签通过，上传 digest 一致。
任一缺失/重命名/路径不符/事件不符/分支不符/SHA 不符/失败前置、`release_ready=false`、残留功能缺口、
缺浏览器、非交互桌面、TVBox 缺失或拉取失败、性能不达标、MSI 生命周期回归、缺证书、签名无效或缺时间戳、
`main` 变动、上传 digest 不符——任一命中即停止，**无回退把失败变成公开发布**。

## 5. 已发布后

- 仅当 `publish=true` 才发布 Latest；核对 GitHub Release 资产与本地暂存文件。
- 保留证据包与 `SHA256SUMS.txt` 供审计。

## 本机侧进度

本机可执行的部分已全部完成并通过，见 `docs/v7-local-verification-record.md`。
本机未执行：五项实机门禁、正式打包/签名/发布、实机安装与真实浏览器/TVBox 证据。
