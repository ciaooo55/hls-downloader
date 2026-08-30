# HLS Downloader 7.0.0 验证状态

核验时间：2026-08-26

## 历史已通过

以下数字是核验日期对应提交的历史基线，不代表当前提交已经重新执行。当前源码状态以
`feature-parity.json`、当前 candidate provenance 和本轮门禁报告为准。

- 功能合同：唯一权威清单为 `artifacts/v7-productization/feature-parity.json`，当前 `24/28` verified、`4` partial。候选打包要求 canonical 清单、无 blocked 项且 Git 工作树干净，允许 partial 以便通过实机证据关闭；正式打包要求全部 `28/28` verified，并额外要求 `release_ready=true`。
- Rust Core：`334/334`，覆盖 IPC、数据库、HTTP/HLS/DASH、FTP/SFTP、BT、播放器、投屏、迁移和恶意输入。
- Core 恢复：pending media push 重启后可继续 resolve；named pipe 创建失败会在 Engine ready 前返回错误。
- HLS 候选证据：认证 VOD/Live 均覆盖未授权 `401`、Authorization 传递、暂停、checkpoint 和不重复分片恢复；Windows PowerShell 5.1 放大复跑 `10/10` VOD 与 `10/10` Live 通过。
- BT 候选证据：Core 经过本地 tracker/peer 完成传输中文件切换、in-flight Cancel、保留文件完成和选中输出物化；PS7 连续 3 次、PS5.1 1 次通过。
- HTTP 分段专项：`39/39`；96 MiB 实际 Range 下载 `76.82 MiB/s`，32 个分段无重叠，发布后额外网络字节为 `0`。
- Compose：协议、组件、选择、右键菜单、设置、敌对输入和性能测试通过；1000 任务模型 P95 `14.802ms`。
- 热确认窗口：可见 P95 `31.03ms`，Native Host 提交 P95 `6.65ms`，提交到可见 P95 `27.95ms`，门槛均为 `100ms`。
- 浏览器扩展：37 个测试文件、222 个测试通过；Chromium MV3 和 Firefox MV3 生产构建成功。
- Native Host：安装目录二进制冷启动首响应 `618.76ms`，双响应总耗时 `620.64ms`，低于 `1500ms` 门槛。
- Presenter 和播放器：单实例、强杀隔离、Core 重连与播放器子进程退出测试通过。
- TVBox：真实接收端完成 `Range: bytes=0-` 拉取，HTTP `206`，共 `42521` 字节。
- 挂机：Core 30 秒工作集 `11.809 -> 11.797 MiB`；1000 次 IPC P95 `0.376ms`，错误和句柄增长均为 `0`。
- 安装后验证：Compose、Engine、Presenter 均从 `E:\h` 运行；窗口 `1280x760`，版本 `7.0.0`，图标和未授权 `401` 门禁通过。
- 便携升级：配置、数据库和下载目录保留，升级后真实回滚通过。
- PowerShell：18 个用户/维护脚本在 Windows PowerShell 5.1 与 PowerShell 7.6 中均通过语法解析。

## 本机构建哈希

- EXE：`2A66C8A83508BB5C140884157731D74FB0A8AE7C645DA2D28FD430BEFAD5E06F`
- MSI：`3D2873E0F7215E9EE68293B3F0BB66E3FAEE17BD9952E6F7F17BF6C9ADC4161A`
- Portable ZIP：`0F9485BB60EBC10051C66B51321E3EE9A73A9D363205B4EEF07F93667409B406`

这些哈希描述 2026-08-24 的本机受测构建，不作为后续不同提交产物的固定哈希。

## 候选包与正式包

仓库中可能保留上一轮 Windows 验收的 candidate 压缩包和 provenance；它们只作历史证据，
必须以当前提交重新运行 candidate 构建后才能用于安装或冒烟，不能通过文件名推断其对应当前源码。

候选包用于外部 Windows 实机验收，不代表正式发布决定：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task candidate
```

候选 EXE、MSI 和 Portable ZIP 写入 `artifacts/v7-productization/candidate`，并在
`BUILD-PROVENANCE.json` 中标记 `package_tier=candidate`。候选门禁接受
canonical feature parity、`blocked=0` 和 clean Git worktree，允许当前
`partial` 项，因此可以在 `release_ready=false` 时生成用于实机验证的包。
实机/端点证据用于关闭 partial；全部 28 项 verified 后，正式包使用：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task package
```

正式产物继续写入既有的 `artifacts/v7-productization/package` 目录，并额外
要求 `release_ready=true`。

正式打包还必须提供本机 `artifacts/v7-productization/release-evidence.json`。该文件绑定当前
commit/tree 和 candidate `ARTIFACT-MANIFEST.json` 的 SHA-256，并且只包含 `browser`、
`performance`、`installer`、`rollback` 四项门禁。每项记录精确命令、输入、原样输出、
退出码、candidate manifest 哈希及报告路径/哈希。正式门禁会重算 candidate EXE/MSI/Portable
与所有报告哈希，并从 Portable 重新提取两种扩展，核对 ZIP digest 和 `manifest.version=7.0.0`，
因此只修改 parity 状态不会通过。
使用记录器实际运行每项门禁，避免手工拼接报告。`-Command` 是在新的 Windows PowerShell
进程中执行的精确命令文本，`-Input` 描述该命令实际使用的 candidate 路径、浏览器或机器：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\record-v7-release-gate.ps1 `
  -GateId browser `
  -Input 'candidate Portable 解压目录和 Chrome/Edge/Firefox' `
  -Command '<实际浏览器生产冒烟命令>'
```

同样记录 `performance`、`installer`、`rollback`。脚本直接捕获输出和退出码，生成
`artifacts/v7-productization/release-evidence/<gate>.json` 并原子更新
`release-evidence.json`；命令失败时仍记录 `failed`，正式门禁不会放行。无输出命令以空字符串
如实记录。每份报告本身必须是 schema 1 JSON 结果封套，且其 `gate_id`、`product_version`、
`source_commit`、`source_tree`、`candidate_artifact_manifest_sha256`、`command`、`input`、
`output`、`result`、`exit_status` 必须与 release evidence 一致。

```json
{
  "schema": 1,
  "product_version": "7.0.0",
  "source_commit": "<git rev-parse HEAD>",
  "source_tree": "<git rev-parse HEAD^{tree}>",
  "candidate_artifact_manifest": { "path": "artifacts/v7-productization/candidate/ARTIFACT-MANIFEST.json", "sha256": "<sha256>" },
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

`gates` 数组须同样包含其余三个固定 ID；每份 report 重用同一组结果字段。

浏览器生产冒烟（media、takeover、browsers）必须显式传入候选或正式产物解压后的
扩展目录；脚本不再默认读取工作树 `extension/.output`，避免把开发输出误当成交付证据。

## 正式标签前门槛

本地源码、运行、升级和浏览器桥接已经可用。创建公开 `v7.0.0` 标签前仍保留两项发布工程门槛：

1. 使用外部 Windows UI Automation 工具完成全部窗口、控件语义和键盘路径验收。
2. 在全新 Windows 虚拟机完成 MSI 安装、覆盖升级、重启、卸载和回滚矩阵。

这两项不影响当前 per-user 本地安装，但决定是否创建正式 GitHub Release。
