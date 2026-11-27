# 云端独立复核与并行开发回执 (`docs/sync/CLOUD_REPORT.md`)

> **写入权限**：本文件建档合入 `main` 后由 **GitHub 云端 AI（Cloud）独占写入**；**本地执行之手（Local）与大脑（Brain）只读不改**。
> 请在每次开始复核或并行开发前，先拉取最新 `origin/main` 并通读根目录 [`SYNC.md`](../../SYNC.md)（关注元信息、领地锁表与 `[Brain -> Local & Cloud]` 指导）。

## 元信息（云端每次更新必填）
- **最后更新时间（UTC+8）**：`2026-11-27 13:30:00 +08:00`
- **所审 `origin/main` SHA**：`41fbb8e3`（[PR #156](https://github.com/ciaooo55/hls-downloader/pull/156) 回填 PR #154 复核结论后的 `main`；代码基线仍为 `34ed0501`）
- **对应 `SYNC.md` 同步序号**：`#2`
- **云端当前状态**：`PR #154 复核结论已在第一节回填；认领 AVAILABLE_FOR_CLOUD 第 4 项（协议与客户端边界测试），仅新增测试文件，不改生产代码与既有 wire 契约`
- **活跃 `cloud/<topic>` 分支 / PR**：`cloud/claim-protocol-wire-tests`（本领地声明 PR）；`cloud/protocol-wire-boundary-tests`（实现 PR，声明合入后提交）

---

## 一、独立真代码复核结论（严守零误报原则）

> **填报规范**：
> - 严禁仅凭猜测或文档印象提缺陷；每条缺陷均已对照当前 `origin/main@6968c41a`（代码基线 `34ed0501`）真实源码与调用链逐条核实。
> - 按 `P0`（安全破坏 / 数据损坏 / 死锁崩溃）、`P1`（高影响功能缺陷 / 状态机竞态 / 严重性能退化）、`P2`（边界瑕疵 / 错误提示偏差 / 可维护性隐患）严格分级。
> - 本次汇总包含 **PR #154（`617033dd..34ed0501`）增量复核**（0 P0 / 0 P1 / 7 P2）与 **617033dd 五模块基线复核中在当前 `main` 仍存续的缺陷**（0 P0 / 5 P1 / 10 P2；原 617033dd 的 `native_ui` 离线 Core 不自愈 P1 已由 PR #154 W2 修复，已核销并记录在第二节）。
> - **总计**：**0 个 P0、5 个 P1、17 个 P2**。

### P0（阻塞级）
- *无*

### P1（高优先级，共 5 条，均为 617033dd 基线存量且在当前 `main@6968c41a` 确认仍存在）

#### P1-1｜HLS 变体未声明外部音轨/字幕组时，仍会误选主清单中其他组的音轨或字幕
- **文件路径与行号**：`native_shell/src/media/hls.rs:554-596, 795-806`；`native_shell/src/media/merge.rs:289-295, 492-503`
- **调用链与触发条件**：`download_worker.rs` 启动 HLS 任务 → `hls.rs:795-806` 选中变体后调用 `select_audio_track(master, variant, ...)` 与 `select_subtitles(master)`。`select_audio_track`（`hls.rs:564`）将 `variant.audio_group.is_empty()` 视为通配（`variant.audio_group.is_empty() || item.group_id == variant.audio_group`）；`select_subtitles`（`hls.rs:587-596`）无条件收集主清单中所有 `SUBTITLES` 渲染组，不绑定选中变体的 `SUBTITLES` 属性。主清单若同时包含“声明了外部 `AUDIO`/`SUBTITLES` 的纯视频变体 A”与“自带内嵌音频、未声明外部组的变体 B”，选中 B 时仍会拉取 A 的外部音轨/字幕，混流阶段 `merge.rs` 存在外部音轨即映射 `1:a:0` 而丢弃 B 自身音轨。
- **全局架构/跨模块影响**：选中带内音频变体时，成品会替换为其他组的音轨（错语言/错轨）；若其他组音轨不可达则导致本可完成的 VOD 在音轨准备阶段失败；字幕开启时会误嵌无关组的字幕。
- **建议修复方向**：以选中变体声明的 `AUDIO` / `SUBTITLES` `GROUP-ID` 为唯一候选范围；变体未声明 `AUDIO` 时保留带内音轨，未声明 `SUBTITLES` 时不拉取其他组字幕；保留组内语言/名称/默认排序与无 URI 即带内的现有语义。
- **建议补充测试**：在 `native_shell/src/media/hls.rs` 新增主清单回归测试（共存带内音频变体与两个外部 `AUDIO`/`SUBTITLES` 组），覆盖 VOD 与直播选择逻辑，断言未声明外部组的变体返回 `None` 音轨与空字幕列表。

#### P1-2｜Firefox 重启后，待清理记录仅按数字 `downloadId` 匹配，可能误删另一条已完成下载的成品
- **文件路径与行号**：`extension/entrypoints/background.ts:1020-1030, 1055-1071, 2804, 2828`；`extension/lib/pausedHandoffFollowups.ts:14-23, 134-153, 168-175`
- **调用链与触发条件**：Firefox 接管进入清理阶段写入 `phase: 'cleanup'` 记录；若首次清理失败，记录滞留在 `storage.local`，其唯一浏览器标识为数字 `downloadId`。Firefox 重启时会从 0 按当前历史列表重新分配下载编号；若原下载历史在下次清理前已消失，另一条无关的已完成下载 B 可能复用该数字编号。启动补扫或 alarm 触发 `continuePausedHandoffFollowups()`（`background.ts:1066-1070`）→ `removeFollowUpDownload(record.downloadId)`（`1020-1030`）→ `removeBrowserDownload(current)`，全程未校验会话、开始时间或来源 URL，直接 `removeFile` + `erase`。
- **全局架构/跨模块影响**：可能不可逆地删除从未交接给桌面端的用户本地下载成品，并抹除浏览器下载历史（仓库在本地导入链路已有 Firefox ID 重映射防护，但该清理链路未复用）。
- **建议修复方向**：在持久化的暂停/清理随访记录中保存最小稳定身份（开始时间、摘要化来源及确认稳定后的文件名），跨会话执行暂停/恢复/取消/删除前强制核对唯一匹配；无足够证据或存在歧义时只丢弃随访记录、绝不按裸数字 ID 删除源文件。
- **建议补充测试**：在 `extension/lib/downloadOwnership.test.ts` 补充 Firefox 重启后数字 `downloadId` 被另一条不同来源/开始时间的已完成下载复用时的 `phase: 'cleanup'` 补扫测试，断言不调用 `removeFile`/`erase`。

#### P1-3｜同提交候选包与正式包共用 `ProductCode` 且生成不同 `PackageCode`，导致覆盖升级触发 MSI `1638`
- **文件路径与行号**：`scripts/build-v7.ps1:488-493`；`scripts/build/set-v7-msi-rollback-order.ps1:152-153, 185-187`
- **调用链与触发条件**：`build-v7.ps1 -Task candidate` 与 `build-v7.ps1 -Task package -CandidateManifestPath ...` 基于同一 `HEAD` commit 均直接截取 `$sourceCommit` 前 32 位作为 `ProductCode`（`build-v7.ps1:489-490`），而 `set-v7-msi-rollback-order.ps1` 每次写入新的随机 `PackageCode`。
- **全局架构/跨模块影响**：已安装同提交候选包的机器覆盖安装正式包（或同提交重打包）时，相同 `ProductCode` + 不同 `PackageCode` 会被 `msiexec /i` 直接以 `1638` 拒绝，无法触发 Major Upgrade。
- **建议修复方向**：将 `package_tier` 与构建清单指纹纳入 `ProductCode` 派生（或每次打包生成新 GUID），与 `Upgrade` 表同版本升级属性对齐。
- **建议补充测试**：在打包校验脚本中补充断言，验证同提交生成的 candidate 与 release MSI 的 `ProductCode` 互不相同。

#### P1-4｜正式包 Authenticode 签名链漏签 MSI 内嵌 EXE、Burn `.exe` 内嵌 MSI 及 `HLSDownloaderPreflight.exe`
- **文件路径与行号**：`scripts/release/finalize-v7-signed-package.ps1:38-75`；`scripts/release/verify-v7-authenticode.ps1:21-38`；`scripts/release/V7ReleaseScope.psm1:42-47`；`scripts/build/build-native-v7-installer.ps1:122-124`
- **调用链与触发条件**：`build-v7.ps1 -Task package` 先用未签名二进制构建 MSI 与 Burn `.exe`；随后 `finalize-v7-signed-package.ps1:52-69` 仅签名 `Portable.zip` 内 `Get-V7PortableExecutables` 返回的 6 个 EXE 与外层 `.msi`/`.exe`，并把签名后的 Portable EXE 哈希写回 `ARTIFACT-MANIFEST.json` 的 `components`。
- **全局架构/跨模块影响**：启用正式签名发布时，MSI 安装落盘的第一方二进制仍为未签名文件且与 `ARTIFACT-MANIFEST.json` 哈希不一致；Burn `.exe` 内嵌仍是未签名 MSI 且未用 WiX `insignia` 签名 Burn 引擎；`HLSDownloaderPreflight.exe` 不在 `Get-V7PortableExecutables` 列表内而始终未签名。
- **建议修复方向**：将签名步骤前置到打包链（先签全部第一方 EXE 含 `HLSDownloaderPreflight.exe` → 构 MSI 并签 MSI → 构 Burn `.exe` 并用 `insignia` + `signtool` 签引擎与外层），并在 `verify-v7-authenticode.ps1` 抽检 MSI 展开后的 EXE 签名与清单哈希一致性。
- **建议补充测试**：在签名验收脚本中补充对 MSI 解包 EXE、`HLSDownloaderPreflight.exe` 及 Burn 引擎签名的断言。

#### P1-5｜`verify_v7_received_regression.py` 未校验 `media-{browser}.json` 的通过状态与组件哈希
- **文件路径与行号**：`scripts/validation/verify_v7_received_regression.py:42-56`
- **调用链与触发条件**：`release-v7.yml:153, 164` → `scripts/release/receive-v7-candidate.ps1:88` / `scripts/release/invoke-v7-release-gates.ps1:77` → `verify_v7_received_regression.py --kind browser`。第 `44-46` 行仅校验 `isinstance(media, list) and len(media) >= 24`，未检查列表条目的通过状态，也未校验对应组件/扩展 SHA-256。
- **全局架构/跨模块影响**：浏览器媒体矩阵存在失败项或旧构建证据时，`browser` 发布门禁仍会放行，削弱发布质量防线。
- **建议修复方向**：让 `scripts/validation/smoke_extension_media.py` 输出显式 `passed` 与组件/扩展摘要，并在 `verify_v7_received_regression.py` 强制校验全部条目通过且匹配 `manifest`。
- **建议补充测试**：在校验脚本单测中构造含失败条目的 `media-edge.json` / `media-firefox.json` 与哈希不匹配样例，断言门禁拒绝通过。

---

### P2（中低优先级，共 17 条：PR #154 本轮 7 条 + 617033dd 基线存量 10 条）

#### PR #154 本轮复核（7 条）

#### P2-1｜`task_failure_from_error` 中 `"地址无效"` 分支遮蔽 `"代理地址无效"`，导致代理配置错误被误判为 `INVALID_URL`
- **文件路径与行号**：`native_shell/src/download_worker.rs:5389-5406, 5443-5467`（关键行：`5391` 与 `5402, 5451`）
- **调用链与触发条件**：代理配置错误来源（`proxy_tunnel.rs:160,176` `"代理地址无效"`、`proxy_tunnel/system.rs:197` `"Windows 系统代理地址无效"`、`http_engine.rs:4481`、`site_rules.rs:289` `"站点规则代理地址无效"`、`download_worker.rs:3032`、`download_worker/settings.rs:650`）→ Worker 失败收口调用 `task_failure_from_error` → 第 `5391` 行 `lower.contains("地址无效")` 优先命中，直接返回 `INVALID_URL`，导致第 `5402` 行 `is_network_policy_error`（第 `5451` 行 `raw.contains("代理地址无效")`）不可达。
- **全局架构/跨模块影响**：所有代理地址格式错误均被误报为 `INVALID_URL`，向用户展示“检查下载地址格式，或回到来源页面重新识别”而非 `NETWORK_POLICY` 的代理配置提示，误导排障（因 `INVALID_URL` 已禁止自动重试，不会引发重试死循环）。
- **建议修复方向**：在 `task_failure_from_error` 中将 `is_network_policy_error(&lower, error)`（以及 `is_drm_protection_error(&lower, error)`）判定提前至 `INVALID_URL` 分支之前。
- **建议补充测试**：在 `native_shell/src/download_worker/tests.rs` 的 `auto_retry_governance_skips_permanent_failures_while_retrying_transient_errors` 补充 `"代理地址无效"`、`"Windows 系统代理地址无效"`、`"站点规则代理地址无效"` 断言 `NETWORK_POLICY`，并保留 `"下载地址无效或不安全"` → `INVALID_URL`。

#### P2-2｜`preview::reset_ready` 在全局 `ready_segments()` 互斥锁内逐片同步重写 `index.tsv`，批量失效续传退化为 O(M×N) 写放大与全局锁竞争
- **文件路径与行号**：`native_shell/src/media/preview.rs:439-465`；`native_shell/src/media/hls.rs:980-990`；`native_shell/src/media/dash.rs:2781-2807`；`native_shell/src/atomic_replace.rs:42-57, 110-139`
- **调用链与触发条件**：已下载大量分片的 HLS/DASH VOD 任务在画质切换、清单变更或签名身份变化后续传 → `hls.rs:982` / `dash.rs:2798` 在每分片循环内调用 `reset_ready`（循环外已紧跟 `flush_ready`）→ `preview.rs:453-464` 在持有进程级全局 `ready_segments()` 互斥锁期间每次 `removed_segment == true` 都全量序列化并同步执行 `flush_ready_index`（`fs::write` + Windows `MoveFileExW(..., MOVEFILE_WRITE_THROUGH)` 及最多 20×10ms = 200ms 杀软重试）。
- **全局架构/跨模块影响**：批量失效场景下重新引入逐片 NTFS/杀软写风暴，抵消 W3 合并索引的减压目标；且持全局锁执行磁盘 I/O 与重试休眠会直接阻塞其他并发任务的 `playback.rs` `is_ready` 热路径与 `mark_ready` 写入。
- **建议修复方向**：为批量失效循环提供延迟刷盘路径（`reset_ready` 更新内存与 `dirty_ops`，由循环末尾已有的 `flush_ready` 统一落盘；单点调用如 `dash.rs:3402` 保留即时刷盘或紧跟 `flush_ready`）；并进一步将序列化后的磁盘写与原子替换移出全局 `ready_segments()` 锁（配合每任务刷盘锁）。
- **建议补充测试**：在 `native_shell/src/media/preview.rs` 补充批量失效回归测试，验证连续 `reset_ready` N 片 + `flush_ready` 后内存立即可见 `is_ready == false`、重启重载仍不可用且刷盘次数为常数级。

#### P2-3｜`extract_zip_archive` 的 PowerShell 回退未清理 `tar.exe` 失败后的部分残留
- **文件路径与行号**：`native_ui/src/system_tools.rs:1832-1877`
- **调用链与触发条件**：`SystemTools::action("chromium")` → `open_chromium_onboarding` → `resolve_chromium_extension` → `unpack_chromium_extension_archive` → `extract_zip_archive`（`System32\tar.exe` 解压中途失败后回退 `powershell.exe` `[System.IO.Compression.ZipFile]::ExtractToDirectory`）。
- **全局架构/跨模块影响**：Windows PowerShell 5.1（.NET Framework 4.x）的双参数 `ExtractToDirectory` 不支持覆盖已存在文件；若 `tar.exe` 已写出部分文件再失败，PowerShell 回退必然抛 `IOException` 导致自动解压自救失败。
- **建议修复方向**：进入 PowerShell 回退分支前清空并重建临时 `dest_dir`（或在调用方为回退解压准备全新 staging 目录）。
- **建议补充测试**：在 `native_ui/src/system_tools.rs` 补充单测模拟 `dest_dir` 存在部分残留文件时触发回退解压，断言先清空残余再完整解出。

#### P2-4｜ZIP 预检的符号链接/重解析点与 EOCD/LFH 一致性校验存在纵深防御缺口
- **文件路径与行号**：`native_ui/src/system_tools.rs:1639-1653, 1670-1728`（重点 `1689-1695`）
- **调用链与触发条件**：`resolve_chromium_extension` → `validate_extension_zip_file` → `validate_extension_zip_entries`。
- **全局架构/跨模块影响**：当前仅拦截 `creator_os == 3`（UNIX）的 `S_IFLNK (0o120000)` 与 `creator_os in {0, 10}` 的重解析点位 `0x0400`，未覆盖 `creator_os == 19`（macOS/Darwin，Windows `tar.exe`/libarchive 同样按 POSIX mode 解析符号链接）与 `creator_os == 14`（VFAT）；且 EOCD 未校验 `entries_on_disk == total_entries`，LFH（`1714-1728`）未校验加密标志位 `flags & 0x0001`。在开启 Windows 开发者模式时，先解压的 symlink 可能在后置 `verify_extracted_tree` 前被后续条目穿透写入。
- **建议修复方向**：符号链接校验纳入 `creator_os in {3, 19}`（或无条件拦截 `(external_attrs >> 16) & 0o170000 == 0o120000`），重解析点校验纳入 `creator_os == 14`；补充校验 EOCD `entries_on_disk == total_entries` 与 LFH `flags & 0x0001 == 0`。
- **建议补充测试**：在 `extension_zip_validator_rejects_*` 中新增 `creator_os = 19` 符号链接、`creator_os = 14` 重解析点及 LFH 加密位不一致用例。

#### P2-5｜`publish_extracted_directory` 走覆盖复制回退时不清理旧文件且未在发布后复检 `dest_dir`
- **文件路径与行号**：`native_ui/src/system_tools.rs:1796-1803, 1927-1957`
- **调用链与触发条件**：`unpack_chromium_extension_archive` → `publish_extracted_directory`（`remove_dir_all`/`rename` 因浏览器或资源管理器占用失败）→ `copy_directory_contents`。
- **全局架构/跨模块影响**：回退覆盖复制不会清理旧版本遗留的多余文件，且未校验 `dest_dir` 本身非符号链接；发布后（`1797-1801`）也未对 `dest_dir` 再执行 `verify_extracted_tree`。
- **建议修复方向**：回退复制前校验 `dest_dir` 非符号链接并尽力清理不在 `staging_dir` 内的旧文件；在 `publish_extracted_directory` 成功后、写入 `.archive-stamp` 前调用 `verify_extracted_tree(dest_dir)?`。
- **建议补充测试**：单测在 `dest_dir` 预置旧文件并强制走回退复制路径，断言旧文件被清理且发布后执行 `verify_extracted_tree(dest_dir)`。

#### P2-6｜`CoreEvent::UpdateAvailable` 在无签名构建/无 MSI 资产且 `release_url` 缺失时未回退 `DEFAULT_GITHUB_RELEASES_URL`
- **文件路径与行号**：`native_ui/src/system_tools.rs:765-797, 1302-1315`；`native_ui/ui/system.slint:94`
- **调用链与触发条件**：`SystemTools::event(CoreEvent::UpdateAvailable)` → `self.set_can_open_release(valid_release_url.is_some())`（`767`）→ `format_update_available_summary`（`1302-1311`）→ `system.slint:94`（`if root.page == "updates" && root.can-open-release`）。
- **全局架构/跨模块影响**：`apply_signature_failure_guidance` 会调用 `ensure_fallback_release_url()` 兜底官方 Releases URL 并显示按钮，但 `UpdateAvailable` 在 `valid_release_url` 为 `None` 时隐藏了「打开发布页」按钮，而摘要文案（`1304, 1310`）仍指示用户“请点击下方「打开发布页」”，文案与界面控件矛盾。
- **建议修复方向**：在 `UpdateAvailable` 分支中，当 `!signing_ready || !asset_ready` 且 `valid_release_url.is_none()` 时调用 `self.ensure_fallback_release_url()`。
- **建议补充测试**：单测无签名构建收到空 `release_url` 的 `UpdateAvailable`，断言 `can_open_release == true` 且触发 `open_release` 打开 `DEFAULT_GITHUB_RELEASES_URL`。

#### P2-7｜已识别的音画分离资源走浏览器右键菜单直接 `video.srcUrl` 入口时丢失 `audioUrl`，绕过 W4 双轨投屏拦截
- **文件路径与行号**：`extension/entrypoints/background.ts:2011-2039, 3230-3268`；`extension/lib/contextMenuActions.ts:37-55`；`extension/lib/resources.ts:168-172, 339-353`
- **调用链与触发条件**：站点解析已为双轨资源（如 Bilibili）识别并上报 `audioUrl`（Popup 与页面面板通过 `resources.ts:168-172` 隐藏投送按钮）；当浏览器右键事件提供直接 HTTP(S) `video.srcUrl`（非 `blob:` MSE）时，`background.ts:3230-3267` 不查询该 tab/frame 的已识别资源，仅用点击信息重建无 `audioUrl` 的合成对象，从而绕过 `pushToTv`/`castToDevice`（`2012, 2027`）的双轨拦截，直接向 Native Host 下发单 URL `media_push`（注：此入口在基线即未拦截，PR #154 补了失败提示但未补齐该入口拦截，非新增回归）。
- **全局架构/跨模块影响**：同一已知音画分离视频在面板不可投，通过右键却会进入桌面投屏设备选择，投送源缺少独立音轨导致接收端无声或拒播，且不展示“先下载合并”提示。
- **建议修复方向**：在 `background.ts:3230-3267` 按同 tab、同 frame、当前页面与目标 URL（含已知变体）查找已识别资源，仅复用可唯一关联的双轨证据并走现有拦截提示，不跨站点继承凭据、不影响普通单轨/下载。
- **建议补充测试**：在 `extension/lib/downloadOwnership.test.ts` 新增 `contextMenus.onClicked` 回归用例：先录入已知双轨，再对同页直接视频 `srcUrl` 触发 Cast/TVBox，断言零 `media_push` 且提示合并；另覆盖普通单轨、`blob:`、不同 frame 不误用双轨记录。

#### 617033dd 基线存量（在当前 `origin/main@6968c41a` 确认仍存在，10 条）

#### P2-8｜HLS WebVTT 的 `EXT-X-MAP` 初始化头未进入字幕合并
- **文件路径与行号**：`native_shell/src/media/hls.rs:174-200, 346-355, 3034-3043, 3110-3139, 3213-3241`；`native_shell/src/media/subtitles.rs:15-22, 86-127`
- **调用链与触发条件**：解析器已将 `EXT-X-MAP` 挂到字幕分片，但 VOD 字幕下载（`hls.rs:3221-3241`）与直播字幕捕获/合并（`3034-3043, 3110-3139`）仅下载并拼接分片本体，未读取初始化 MAP；`subtitles.rs:86-127` 因而读不到位于初始化头中的 `X-TIMESTAMP-MAP`。
- **全局架构/跨模块影响**：符合 RFC 8216 §3.5、通过 `EXT-X-MAP` 携带 `X-TIMESTAMP-MAP` 的 HLS WebVTT 字幕会出现固定时间偏移（VTT/SRT 边车及可选内嵌字幕均受影响）。
- **建议修复方向**：VOD 与直播共用字幕 MAP 拉取/缓存（沿用现有受限媒体请求、范围与解密语义），解析 cue 前合并对应初始化头并按 MAP 变更更新。
- **建议补充测试**：在 `native_shell/src/media/hls.rs` / `subtitles.rs` 补充带 `EXT-X-MAP` 非零 `X-TIMESTAMP-MAP` 与 MAP 切换的 WebVTT 合并测试。

#### P2-9｜HLS WebVTT 跨 MPEG-TS 33 位回绕时字幕时间倒退约 26.5 小时
- **文件路径与行号**：`native_shell/src/media/hls.rs:3043, 3241`；`native_shell/src/media/subtitles.rs:15-22, 39-48, 112-127`
- **调用链与触发条件**：`hls.rs` 将有序字幕分片交给 `merge_webvtt_segments`；`subtitles.rs:18-22, 112-126` 独立计算每片 `MPEGTS / 90000 - LOCAL` 后按绝对时间排序，未展开跨片 33 位（`2^33`）回绕。
- **全局架构/跨模块影响**：字幕分片跨越 33 位 PTS 回绕点（含上游初始 PTS 临近回绕的短录制）时，后片时间戳会跳回 `00:00` 附近并与前片倒置（相差约 26.5 小时）。
- **建议修复方向**：在有序分片合并上下文中展开 33 位 MPEG-TS 回绕，并结合媒体时间轴/`EXT-X-DISCONTINUITY` 区分时钟回绕与真实重置。
- **建议补充测试**：在 `native_shell/src/media/subtitles.rs` 新增跨 `2^33` 回绕与 `DISCONTINUITY` 对照回归测试。

#### P2-10｜DASH 直播 `r=-1` 在应用 DVR 窗口裁剪前即触发百万分片上限
- **文件路径与行号**：`native_shell/src/media/dash.rs:4365-4414`（关键行：`4383-4396` 与 `4402-4404`）
- **调用链与触发条件**：动态 MPD 中无 Period 结束边界且末项 `<S ... r="-1"/>` 走 `live_clip` 分支（`dash.rs:4383-4396`）计算从 `time` 到 `live_end` 的总 `repeat`，紧接着在 `4402-4404` 检查 `repeat >= 1_000_000` 并报错 `DASH timeline exceeds segment limit`，早于 `4407+` 的 `timeShiftBufferDepth` 窗口裁剪循环。
- **全局架构/跨模块影响**：长驻 24×7 DASH 直播源（例如 1 秒分片、DVR 仅 60 秒）在连续运行超过约 11.6 天（1,000,000 秒）后会在解析阶段直接失败，尽管实际可下载窗口始终很小。
- **建议修复方向**：先用防溢出的整数算术跳过早于 DVR cutoff 的重复分片并同步推进 `clock`/`number`，仅对窗口内将要输出的区间执行现有分片数与 URL 字节预算上限（不放宽安全上限）。
- **建议补充测试**：在 `native_shell/src/media/dash.rs` 新增 `live_end > 1_000_000s`、DVR 窗口 60 秒的 `r=-1` 动态 MPD 解析回归测试。

#### P2-11｜Native Host `prompt_media_push_ui` 吞掉 `open_workbench()` 失败并记住请求 ID，禁掉同会话重试
- **文件路径与行号**：`native_clients/src/native_host.rs:1124-1140`
- **调用链与触发条件**：浏览器发送 `media_push` → Core 持久化 `pending` → Host 调用 `prompt_media_push_ui(request_id)`（`1134-1140`），在调用 `let _ = self.core.open_workbench();` 前即写入 `self.media_push_ui_request_id = Some(request_id)` 并丢弃启动失败；当 Presenter 瞬时启动失败后，浏览器在同一 Host 会话内再次点击投送，Core 拒绝新 ID，Host 查回旧 `pending` ID 命中 `1135` 行去重直接返回，始终不再拉起工作台。
- **全局架构/跨模块影响**：瞬时启动失败恢复后，同 Host 会话内用户重试投送无法再唤起设备选择器，旧 `pending` 请求同时阻塞新投送。
- **建议修复方向**：区分“请求已提交”与“选择器已成功唤醒”；仅在 `open_workbench()` 成功时记录去重 ID，失败时记录日志并允许围绕旧 ID 有界重试（不重复提交、不删凭据）。
- **建议补充测试**：在 `native_clients/src/native_host.rs` 新增注入一次 `open_workbench` 失败的回归测试，断言同一 Host 下次触发可重新唤醒原 `pending` 请求。

#### P2-12｜唤醒常驻工作台的 10 秒时限不覆盖已连接的 Win32 IPC 请求
- **文件路径与行号**：`native_ui/src/activation.rs:93-121`；`crates/client/src/core_ipc.rs:1623-1655, 1931-1938`
- **调用链与触发条件**：二次启动 Presenter 命中单实例锁后调用 `activation::forward_workbench`（`98-120`）。10 秒 `deadline` 仅约束外层重试循环，连接管道成功后 `pipe.request(OpenMain)` 使用默认 15 秒写超时与 180 秒读超时。若对端管道已打开但进程挂起不回执，调用方将阻塞远超 10 秒。
- **全局架构/跨模块影响**：常驻 Presenter 进程异常卡死时，第二次双击启动会长时间挂起无法按 10 秒约定快速失败。
- **建议修复方向**：向连接、写入、读取及退避传递统一绝对 `deadline`（每阶段按剩余预算计算超时，保留 overlapped 取消等待完成后再释放栈缓冲区约束）。
- **建议补充测试**：在 `crates/client/src/core_ipc.rs` / `native_ui/src/activation.rs` 补充端到端总截止时间单测（对端接收连接但永不回复时按总期限返回）。

#### P2-13｜工作台最大化后关闭优先沿用磁盘旧坐标，丢失最近一次普通窗口位置和尺寸
- **文件路径与行号**：`native_ui/src/native_window.rs:1213-1245`
- **调用链与触发条件**：普通窗口关闭一次后磁盘已有坐标 A → 重开后拖放/缩放为 B → 不先关闭直接最大化再关闭 → `save_geometry` 的 `if maximized` 分支（`1214-1244`）读取旧值 A，因 `previous.x.is_some() && previous.y.is_some()` 跳过 `GetWindowPlacement`，仅把 `previous.maximized = true` 写回磁盘，丢失 B。
- **全局架构/跨模块影响**：重启应用并取消最大化时，窗口退回更早的旧布局 A，用户最近一次手动调整丢失。
- **建议修复方向**：最大化保存时优先采用当前有效的普通态窗口矩形（或持续追踪最后一次普通态矩形），仅在查询失败/无效时回退磁盘旧值，并注意工作区坐标与 DPI 换算一致。
- **建议补充测试**：在 `native_ui/src/native_window.rs` 补充普通态几何更新后最大化关闭的持久化单测。

#### P2-14｜Native Messaging 队列满（64 条）时明确未发送的接管被标为 `uncertainOffer`，原下载一直暂停
- **文件路径与行号**：`extension/lib/nativeBridge.ts:142-159`；`extension/entrypoints/background.ts:1956, 2041-2046, 2093-2096`；`extension/lib/pausedHandoffFollowups.ts:83-95`
- **调用链与触发条件**：浏览器接管先暂停原下载再发 offer；当 `NativeBridge.queue.length >= 64` 时，`nativeBridge.ts:148` 在入队与 `postMessage` 之前直接 `Promise.reject`；上层将此确定未发送的本地拒绝与真实传输超时/回执丢失混同为 `transport_error` → `uncertainOffer`（`background.ts:2041-2046`），持久化待定记录并阻止 `finally` 恢复原下载。后续补扫向 Host 查询从未收到的 ID 始终返回 `handoff: null`，每 60 秒续期且无自动恢复路径。
- **全局架构/跨模块影响**：桥接过载时既无桌面任务、原浏览器下载又永久停在暂停态需人工恢复（Firefox 请求前接管路径同样会在所有权“不明”时返回 `cancel: true`）。
- **建议修复方向**：在 `NativeBridge` 为队列准入拒绝等“确定未投递”错误提供显式错误码，新建 offer 命中该错误码时清理 journal 并回退恢复浏览器下载；对已投递或可能投递的错误继续保持 `uncertainOffer` 保守策略，**不得**把 `handoff: null` 当成接管失败。
- **建议补充测试**：在 `extension/lib/downloadOwnership.test.ts` 新增桥接队列满（64 条）时的接管集成测试，断言新 offer 走明确未发送回退并恢复浏览器下载，而真传输超时仍保持暂停保守策略。

#### P2-15｜SBOM 遗漏已随包分发的 `curl-impersonate.exe` 与 `vulkan-1.dll`
- **文件路径与行号**：`scripts/release/generate_sbom.py:62-68`（对比 `scripts/build-v7.ps1:102-111, 340-341`）
- **调用链与触发条件**：`scripts/release/stage-v7-release.ps1:87` → `generate_sbom.py::build_sbom` 仅静态写入 `ffmpeg 9.0.2` 与 `libmpv 20261110-git-a61f6c3abd`。
- **全局架构/跨模块影响**：CycloneDX SBOM 遗漏实际随包分发的 `vulkan-1.dll` 与 `curl-impersonate.exe`，且版本号分散硬编码易与构建脚本漂移。
- **建议修复方向**：补齐上述两项随包第三方组件，并抽离共享第三方版本锁定 JSON 统一驱动 `build-v7.ps1` 与 `generate_sbom.py`。
- **建议补充测试**：在发布契约校验中对比 `app/resources/` 随包第三方二进制与 SBOM 组件清单。

#### P2-16｜`.github/workflows/release-v7.yml` 末尾 `always()` 清理步骤未判空目录即调用 `shutdown-running.ps1`
- **文件路径与行号**：`.github/workflows/release-v7.yml:325-332`；`scripts/install/shutdown-running.ps1:10`
- **调用链与触发条件**：工作流末尾 `Stop release-owned processes` 遍历两个目录直接调用 `shutdown-running.ps1`，后者在 `$ErrorActionPreference = 'Stop'` 下第 10 行直接 `Resolve-Path`。
- **全局架构/跨模块影响**：前期步骤失败或首个目录尚未创建时抛错中断，导致第二个目录得不到清理且清理步骤标红。
- **建议修复方向**：在循环内调用前增加 `if (Test-Path -LiteralPath $dir -PathType Container)` 守卫。
- **建议补充测试**：本地 PowerShell 校验首个目录不存在、第二个目录存在时仍能完成第二个目录的进程清理且返回 0。

#### P2-17｜`scripts/install/installer-preflight.rs` 检查点回执未拦截内嵌 `error` 事件，且 `files_unlocked` 漏检 DLL 与 `curl-impersonate.exe`
- **文件路径与行号**：`scripts/install/installer-preflight.rs:679-681, 693-721`（对比 `scripts/install/shutdown-running.ps1:133-136`）
- **调用链与触发条件**：MSI `V7StopRunningApplications` / 便携升级调用 `HLSDownloaderPreflight.exe` → `checkpoint()`（`679-681`）仅校验 JSON 字节前缀，未拒绝 `events` 中内嵌的 `"kind":"error"`；`files_unlocked()`（`693-721`）仅检查 `root` 与 `app/resources` 顶层 `.exe`。
- **全局架构/跨模块影响**：若 Core 返回含 `"kind":"error"` 的 events 帧会被误判成功；若残留进程占用 `libmpv-2.dll`、`vulkan-1.dll` 或 `app/resources/tools/curl-impersonate/curl-impersonate.exe`，预检会误放行并延后至安装/目录替换阶段失败。
- **建议修复方向**：`checkpoint()` 拒绝包含 `"kind":"error"` 的回执；`files_unlocked()` 补充检查 `app/resources/*.dll` 与 `app/resources/tools/curl-impersonate/curl-impersonate.exe`。
- **建议补充测试**：在 `installer-preflight.rs` 补充解析含 `"kind":"error"` events 帧失败与占用 DLL/嵌套工具文件被拒绝的单元测试。

---

## 二、已核验通过 / 排除假阳性清单
> 记录本轮重点排查后确认无缺陷、已由 PR #154 修复或已由现有机制兜底的怀疑点（附源码证据），避免大脑与本地重复排查。

### 1. 已由 PR #154 修复并核验通过（原 617033dd 基线问题核销）
- **原 617033dd P1「引擎异常退出后，从桌面重新打开应用不会重启引擎」**：已由 PR #154 W2 Presenter 自愈修复（`native_ui/src/hot_main.rs:722-723, 944-967, 2688-2730, 8696`）。显式 `--workbench` 激活会清除 `user_shutdown`，并通过 `maybe_revive_offline_core`（`compare_exchange` + `ReviveGuard`）在后台安全拉起离线 Core；同时 `event_loop` 通过 `should_quit_for_user_shutdown(is_user_shutdown_active, core_revive_in_flight)` 双检，确保被动轮询仍不会误复活用户主动退出的引擎。

### 2. PR #154 重点排查已排除疑点
- **`native_shell` — W3-A4 锁外冷加载并发竞态（`preview.rs:278-302, 367-388, 427-449, 484-506`）**：锁外 `load_ready_state_from_disk` 后二次加锁均走 `Entry::Occupied` / `or_insert` 复用先入驻条目，不覆盖并发已写入状态；`is_ready` 持锁期间 0 系统调用、纯 O(1) 内存查询。
- **`native_shell` — HLS/DASH 分片命名与扩展名切换（`preview.rs:76-91`、`hls.rs:2184, 2199`）**：`{:06}.<ext>` 与 `hls_slot_key` 解析一致；续传同槽位换扩展名时旧文件已被删除，`is_ready` 末尾 `fs::metadata` 会返回 `false`。
- **`native_shell` — DASH 直播未调用 `flush_ready`（`dash.rs:1479-1509, 2340-2445`、`download_worker.rs:7149-7153`）**：DASH 直播仅结束时写 `local.m3u8`、不写 `preview.m3u8`，而 `playable_task_playlist` 仅检查 `preview.m3u8`，无暂停重启预览缺失问题。
- **`native_shell` — W3-A1 LL-HLS 签名回退与控制字优先级（`hls.rs:1918-1985`）**：先拦截 `"paused" | "canceled" | "finish"`，再在 `target != url` 失败时无休眠回退原始 `url`；本地网络策略拒绝时零网络零休眠中止。
- **`native_shell` — W3-A2/A3 代理瞬时重试与 DASH 终次真实错误保留（`hls.rs:1830-1873`、`dash.rs:2469-2542`）**：代理瞬时错误（CONNECT aborted / 502 / 503 / 握手超时）保留重试；DASH 终次尝试保留底层错误字符串；`finish` 先于 `EngineError::Pause` 检查并返回 `Ok(None)`。
- **`native_shell` — W3-A5 缺 FFmpeg 分类与 `stable_media_url` 清洗边界（`download_worker.rs:5343-5352, 5437-5441, 5519-5525`、`hls.rs:3470-3670`）**：`is_missing_ffmpeg_error(&lower)` 已提前且中英文大小写不敏感；`stable_media_url` 仅用于去重/checkpoint，保留关键分片标识参数且按 ASCII 分隔符切片无 UTF-8 越界，不影响 SSRF/白名单/代理安全边界。
- **`native_ui` / `native_clients` / `scripts` — 安全边界与打包对齐（6 项）**：
  - `file_dialogs::open` 白名单未弱化：`TERMS.md`/`PRIVACY.md` 通过 `SystemWindow` 内联读取展示，`open_release_page` 经 `sanitize_github_release_url` 严格限定 HTTPS `github.com`；
  - 未签名更新与 rollover 授权一致（`crates/client/src/update_install.rs:116-123` 与 `system_tools.rs:1211-1227` 均校验编译期主指纹）；
  - 异步磁盘探测 `ConfirmDiskSpaceProbe` 单 worker + 单调 `generation` 三重校验 + DropGuard，无嵌套锁死锁；
  - 冷启动极速下载 HUD 唤醒（W2-A1）失败仅记日志不回滚已受理任务，`--workbench` 显式屏蔽且由 `completed_notified` 去重；
  - `TERMS.md`/`PRIVACY.md` 已同步到 `scripts/build-v7.ps1:472-473`、MSI 与 Portable 整树打包；`config/release/feature-parity.json` 的 `release_ready: false` 未改动。
- **`extension` — W4 投屏主清单、长度限制与接管文案（3 项）**：
  - 质量选择后 `resources.ts:586-597` + `mediaPush.ts:5-12` 在投屏前恢复 HTTP(S) master URL，Core 用同一请求建立代理；
  - Native Messaging 4 MiB 入站 / 1,000,000 字节出站、URL/变体长度限制及 Core 同 ID 幂等 / 异 ID 排斥 pending 均保持不变；
  - 接管文案调整未改变被动唤醒判定、handoff 确认状态机或浏览器下载所有权清理路径。

### 3. 核心调度与网络引擎不变式（617033dd 基线，当前 `main` 均保持）
- **SSRF / 重定向 / 分段与断点 / 持久化**：每一跳重定向均重算代理、目标地址范围与 `origin_boundary` 并固定连接目标防 DNS rebinding；跨源重定向先剥离敏感头再套用 scoped 凭据；`RangeScheduler` 锁顺序 `pending -> active -> progress` 一致；`completed_ranges_from_checkpoint` 与 `write_progress_rows` 单调推进无旧状态覆盖风险。

### 4. 待设计确认与验证边界说明（非缺陷）
- **连接池常量语义确认（`native_shell/src/net_policy.rs:28-29`）**：`GLOBAL_CONNECTION_LIMIT = 128` / `PER_HOST_CONNECTION_LIMIT = 24` 目前仅在 `http_engine.rs` `run_job_once` 按“任务”取 1 个名额，分段 worker 不单独占名额，同 host 实际 TCP 连接数可能超过 24。若本意为按任务限流建议重命名常量/注释；若本意为按 TCP 连接限流则需在 `fetch_range` 内按连接取名额。
- **Linux 容器验证限制**：本轮全部复核在 Linux 容器执行（含 `extension` 65 个测试文件 / 764 用例全绿与 `native_shell` 媒体/回放/投屏单测）；`media_proxy::public_cast_manifest_blocks_ssrf_subroutes_and_redirects_while_preserving_lan_mounts` 在非 Windows 下因 `http_engine` 返回 `https/proxy needs WinHTTP` 报 502 属平台限制，非 Windows 投屏缺陷，需在 Windows 环境复跑。真实投屏/TVBox 硬件与 Windows 实机安装包尚未验证。

---

## 三、云端并行开发领地声明（动工前先登记）

> **防打架硬规则**：
> 1. 并行开发前必须核对 [`SYNC.md`](../../SYNC.md) 中的 `AVAILABLE_FOR_CLOUD`，**严禁**触碰 `LOCKED_LOCAL` 与 `NEXT_LOCAL_PLANNED` 中的任何文件，**严禁**修改根目录 `SYNC.md`。
> 2. 所有云端改动必须通过 `cloud/<topic>` 分支提 PR 到 `main`，不得直推 `main`，不得修改 `config/release/feature-parity.json` 的 `"release_ready": false`，不得打任何版本标签或触发 Release 工作流。

| 状态 | `cloud/<topic>` 分支 | 目标模块与任务说明 | 计划锁定文件清单（须完全位于 `AVAILABLE_FOR_CLOUD`） | PR 链接 & CI 状态 |
| --- | --- | --- | --- | --- |
| 已认领（开发中） | `cloud/protocol-wire-boundary-tests` | `AVAILABLE_FOR_CLOUD` 第 4 项：`crates/protocol` 帧编解码（4 MiB 上限、截断/长度不符/超限）、`CorePipeRequest`/`CorePipeResponse` 标签名与 `serde(default)` 向后兼容、`request_id` 映射、`Debug` 不泄露凭据、`CrawlSpec` `deny_unknown_fields`；`crates/client` `request_validation` 公共校验（保留设备名、HTTP 方法、URL 协议白名单、资源键归一化）边界回归。只新增集成测试，不改生产代码；若测试暴露真缺陷，写入本文件第一节而不改行为 | `crates/protocol/tests/wire_contract_boundaries.rs`（新增）、`crates/client/tests/request_validation_boundaries.rs`（新增） | 待提交 |
