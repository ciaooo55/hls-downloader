# HLS Downloader v7 验证状态

当前源码状态核验：2026-09-08。活动 canonical 产品版本为 `7.0.2`，`release_ready=false`。

本文同时保存较早 v7.0.1 阶段的实测基线。**历史数字只证明当时对应提交/产物，不自动证明当前 `main`。** 当前提交是否可进入正式发布，必须以 canonical feature contract、同一 SHA 的四个 prerequisite workflows、当前 candidate provenance 与重新生成的 release evidence 为准。

## 当前 canonical 状态

- 唯一权威功能/版本合同：`artifacts/v7-productization/feature-parity.json`。
- 当前 `product_version=7.0.2`、`release_ready=false`、`audit_state=v7_0_2_iteration_in_progress`。
- candidate 可以在 `release_ready=false` 时构建以收集实机证据；formal `package` 仍要求 canonical completeness、clean worktree、当前 release evidence 和 `release_ready=true`。
- 正式 GitHub 发布还必须由 `.github/workflows/release-v7.yml` 在专用 self-hosted Windows x64 `hls-release` runner 上，对冻结 `main` SHA 执行 exact-SHA prerequisite、真实浏览器/性能/MSI/rollback、签名/时间戳、Draft asset digest 与显式 publish 授权。
- 最新公开可下载测试包仍为历史 `v7.0.1-candidate.1`；其文件名、manifest、provenance 和 SHA-256 不可当作当前 7.0.2 SHA 的证据。

## 历史已通过（v7.0.1 阶段基线）

以下数字来自 2026-08-24 至 2026-08-26 附近的受测提交/产物，保留用于回归参考，不代表当前提交已经重新执行：

- 功能合同：历史快照曾达到 `28/28 verified`、`0 partial`，并在当时发布准备阶段出现过 `release_ready=true`；**当前 canonical contract 已进入 7.0.2 iteration 且为 `release_ready=false`**。
- Rust Core：历史基线 `334/334`，覆盖 IPC、数据库、HTTP/HLS/DASH、FTP/SFTP、BT、播放器、投屏、迁移和恶意输入。后续安全任务已经继续增加测试，因此不要把 `334` 当作当前固定测试总数。
- Core 恢复：pending media push 重启后可继续 resolve；named pipe 创建失败会在 Engine ready 前返回错误。
- HLS 候选证据：认证 VOD/Live 均覆盖未授权 `401`、Authorization 传递、暂停、checkpoint 和不重复分片恢复；Windows PowerShell 5.1 放大复跑 `10/10` VOD 与 `10/10` Live 通过。
- BT 候选证据：Core 经过本地 tracker/peer 完成传输中文件切换、in-flight Cancel、保留文件完成和选中输出物化；PS7 连续 3 次、PS5.1 1 次通过。
- HTTP 分段专项：`39/39`；96 MiB 实际 Range 下载 `76.82 MiB/s`，32 个分段无重叠，发布后额外网络字节为 `0`。
- Compose：协议、组件、选择、右键菜单、设置、敌对输入和性能测试通过；1000 任务模型 P95 `14.802ms`。
- 热确认窗口：可见 P95 `31.03ms`，Native Host 提交 P95 `6.65ms`，提交到可见 P95 `27.95ms`，当时门槛均为 `100ms`。
- 浏览器扩展：历史基线 37 个测试文件、222 个测试通过；Chromium MV3 和 Firefox MV3 生产构建成功。
- Native Host：安装目录二进制冷启动首响应 `618.76ms`，双响应总耗时 `620.64ms`，低于当时 `1500ms` 门槛。
- Presenter 和播放器：单实例、强杀隔离、Core 重连与播放器子进程退出测试通过。
- TVBox：真实接收端完成 `Range: bytes=0-` 拉取，HTTP `206`，共 `42521` 字节。
- 挂机：Core 30 秒工作集 `11.809 -> 11.797 MiB`；1000 次 IPC P95 `0.376ms`，错误和句柄增长均为 `0`。
- 安装后验证：历史受测安装中的 Compose、Engine、Presenter 均从 `E:\h` 运行；窗口 `1280x760`、图标和未授权 `401` 门禁通过。该历史条目中的产品版本属于当时产物，不能用于判断当前 7.0.2。
- 便携升级：配置、数据库和下载目录保留，升级后真实回滚通过。
- PowerShell：历史用户/维护脚本集合在 Windows PowerShell 5.1 与 PowerShell 7.6 中通过语法解析；当前脚本集合仍由 CI 的 PowerShell contract validation 负责重新验证。

## 历史本机构建哈希

- EXE：`2A66C8A83508BB5C140884157731D74FB0A8AE7C645DA2D28FD430BEFAD5E06F`
- MSI：`3D2873E0F7215E9EE68293B3F0BB66E3FAEE17BD9952E6F7F17BF6C9ADC4161A`
- Portable ZIP：`0F9485BB60EBC10051C66B51321E3EE9A73A9D363205B4EEF07F93667409B406`

这些哈希只描述 2026-08-24 的历史本机受测构建，不是 7.0.2 或任何后续提交产物的固定哈希。

## 当前 candidate 与 formal package 合同

仓库/本机可能存在上一轮 Windows 验收的 candidate 压缩包和 provenance；它们只作历史证据，必须对当前提交重新构建后才能用于当前安装/冒烟，不能通过文件名推断其对应当前源码。

当前 candidate 构建：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
```

candidate EXE、MSI、Portable ZIP 和浏览器包写入 `artifacts/v7-productization/candidate`，其 manifest/provenance 的产品版本必须等于 canonical `feature-parity.json.product_version`（当前为 `7.0.2`）。candidate gate 要求 canonical contract、`blocked=0` 和 clean Git worktree，允许验证尚未完全结束，因此可以在 `release_ready=false` 时生成用于实机验证的包。

formal package 构建：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task package
```

formal package 写入 `artifacts/v7-productization/package`，额外要求 canonical completeness、clean worktree、当前 release evidence 和 `release_ready=true`。当前 canonical `release_ready=false`，所以正式打包必须保持 fail closed。

## release evidence 合同

formal package 必须提供当前 `artifacts/v7-productization/release-evidence.json`。该文件绑定同一个 source commit/tree 和 candidate `ARTIFACT-MANIFEST.json` SHA-256，并且只接受固定的 `browser`、`performance`、`installer`、`rollback` 四项门禁。每项记录精确命令、输入、原样输出、退出码、candidate manifest 哈希及报告路径/哈希。

正式门禁会重算 candidate EXE/MSI/Portable 与所有报告哈希，并从 Portable 重新提取两种扩展，核对 ZIP digest 和扩展 `manifest.version` **等于当前 canonical product version**；不再把 `7.0.1` 当作可执行常量。只修改 parity 状态或只改文档不能通过这些门禁。

使用记录器实际运行每项门禁，避免手工拼接报告：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\record-v7-release-gate.ps1 `
  -GateId browser `
  -Input 'candidate Portable 解压目录和 Chrome/Edge/Firefox' `
  -Command '<实际浏览器生产冒烟命令>'
```

同样记录 `performance`、`installer`、`rollback`。脚本捕获输出和退出码，生成 `artifacts/v7-productization/release-evidence/<gate>.json` 并原子更新 `release-evidence.json`；命令失败时仍记录 `failed`，formal gate 不会放行。每份 report 的 `gate_id`、`product_version`、`source_commit`、`source_tree`、`candidate_artifact_manifest_sha256`、`command`、`input`、`output`、`result`、`exit_status` 必须与 release evidence 一致。

版本字段示例应跟随 canonical contract，而不是复制旧版本号：

```json
{
  "schema": 1,
  "product_version": "<feature-parity.json product_version>",
  "source_commit": "<git rev-parse HEAD>",
  "source_tree": "<git rev-parse HEAD^{tree}>",
  "candidate_artifact_manifest": {
    "path": "artifacts/v7-productization/candidate/ARTIFACT-MANIFEST.json",
    "sha256": "<sha256>"
  },
  "gates": [{
    "id": "browser",
    "command": "<exact command>",
    "input": "<exact input>",
    "output": "<literal output>",
    "result": "passed",
    "exit_status": 0,
    "candidate_artifact_manifest_sha256": "<same sha256>",
    "report": { "path": "<repository-relative JSON report>", "sha256": "<sha256>" }
  }]
}
```

`gates` 数组须包含其余三个固定 ID；每份 report 重用同一组结果字段。浏览器生产冒烟（media、takeover、browsers）必须显式传入 candidate/formal 产物解压后的扩展目录；脚本不得默认读取工作树 `extension/.output`，避免把开发输出误当成交付证据。

## 当前正式发布前门槛

当前 `7.0.2` source line 的本地功能可继续开发/验证，但 `release_ready=false` 明确表示**尚未进入正式发布授权状态**。在任何公开 `v7.0.2` formal tag/Release 前，至少仍必须满足：

1. 同一冻结 main SHA 的 `v7 CI`、`v7 Candidate Package`、`Maintenance Security`、`Rust Security` 四个 workflow（三个 push，候选包为 main 上手动 workflow_dispatch） 均成功。
2. Edge 与 Firefox 对该 SHA 重新构建的 candidate 完成真实 browser 门禁；不得复用历史 7.0.1 扩展包。
3. 当前 candidate Engine、Native Host、Compose 满足 formal performance thresholds。
4. 公开 `v7.0.0` MSI 作为不可变升级基线，在固定 `E:\h` 生命周期环境升级到**当前 canonical candidate version**并验证应用进程恢复。
5. 注入失败的 candidate MSI 副本证明 rollback 不破坏旧 ProductCode、Engine、数据与注册。
6. formal Windows 资产由项目受信 signer 完成 Authenticode + RFC3161 timestamp，并验证 signer identity/trust。
7. formal workflow 在上传 Draft Release 后逐项核对 GitHub asset size 和 SHA-256 digest，并且只有显式 `publish=true` 才发布 Latest。

版本标签由 formal workflow 从 canonical `product_version` 动态得到；当前 canonical 是 `7.0.2`，但本文不授权创建该 tag/Release，也不授权把 `release_ready` 改成 true。
