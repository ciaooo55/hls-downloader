# 云端独立复核与并行开发回执 (`docs/sync/CLOUD_REPORT.md`)

> **写入权限**：本文件建档合入 `main` 后由 **GitHub 云端 AI（Cloud）独占写入**；**本地执行之手（Local）与大脑（Brain）只读不改**。
> 请在每次开始复核或并行开发前，先拉取最新 `origin/main` 并通读根目录 [`SYNC.md`](../../SYNC.md)（关注元信息、领地锁表与 `[Brain -> Local & Cloud]` 指导）。

## 元信息（云端每次更新必填）
- **最后更新时间（UTC+8）**：`2026-11-28 21:50:57 +08:00`
- **所审 `origin/main` SHA**：`305b8b76`（已同步 PR #163 合入后的主线；PR #160、#161 的独立复核与源码行号仍以实际复核基线 `529fcbf4` 为准）
- **对应 `SYNC.md` 同步序号**：`#4`
- **云端当前状态**：PR #160、#161 独立复核已完成，核销与新缺陷见第二节第 5、6 小节；本次只更新复核回执，不认领业务代码。
- **活跃 `cloud/<topic>` 分支 / PR**：`cloud/review-pr160-pr161`（本复核回执）；`cloud/fix-p1-5-media-regression-gate`（[PR #163](https://github.com/ciaooo55/hls-downloader/pull/163) 已合入，`305b8b76`）。

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
- **修复状态**：已由 [PR #163](https://github.com/ciaooo55/hls-downloader/pull/163) 收口（合入提交 `305b8b76`）。
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

### 5. PR #160 运行时修复复核（`16324a8f`）

- 对照 [PR #160](https://github.com/ciaooo55/hls-downloader/pull/160) 合并前后的真实变更，在 `origin/main@529fcbf4` 复核调用链与错误处理；后续提交未改动这 17 个变更文件。[PR CI](https://github.com/ciaooo55/hls-downloader/pull/160/checks)：`7 passed, 0 failed, 0 pending, 0 skipped`。
- **17 项：14 项已修复、3 项部分修复、0 项未修复；确认 3 条新 P2（P2-18～P2-20），无新增 P0/P1。** 下列核销更新第一节对应的历史记录；同一直播字幕回归同时影响 CR-P2-9 与 PRA-P2-1，只计为一条新缺陷。
- 本次两份复核均不含 CR-P1-5；该项由 [PR #163](https://github.com/ciaooo55/hls-downloader/pull/163) 单独处理，未在此核销。发布门禁仍为 `release_ready: false`。

#### 逐项核销

| 编号 | 判定 | 当前主线证据与边界 |
|---|---|---|
| CR-P1-1 | 已修复 | `native_shell/src/media/hls.rs:566–629,828–843,865–875,931–935`：VOD/直播共同入口按选中变体的 `AUDIO`、`SUBTITLES` 组绑定；空组无外轨；无 URI 的音频保留为带内；音频仍可按名称/语言选择。 |
| CR-P1-2 | 已修复 | `extension/entrypoints/background.ts:651–675,696–741,1115–1149,1202–1262,2330–2368`；`extension/lib/pausedHandoffFollowups.ts:44–98`：浏览器重启后须按开始时间和 URL 摘要唯一匹配，供完成态文件名与大小辅助约束，身份不足或歧义只丢记录，不删除/恢复；删除前重新查项并校验。 |
| CR-P2-1 | 已修复 | `native_shell/src/download_worker.rs:5389–5406`：DRM 和网络策略分支在泛化的无效 URL 之前；`native_shell/src/download_worker/tests.rs:13656–13687` 覆盖多种代理地址错误。 |
| CR-P2-2 | 已修复 | `native_shell/src/media/preview.rs:324–433,549–603`：`reset_ready` 不再每片写索引，刷新用逐任务锁与 generation 快照，在全局锁外写盘；`native_shell/src/media/hls.rs:1012–1025`、`native_shell/src/media/dash.rs:2792–2807,3393–3404` 接回批量和单点刷盘。原写放大已收口；容量边界的新回归见 P2-18。 |
| CR-P2-3 | 已修复 | `native_ui/src/system_tools.rs:1885–1897,1926–1966`：主解包失败后先清除并重建普通 staging 目录，拒绝链接及重解析点，再进入 Windows PowerShell 路径。 |
| CR-P2-4 | 已修复 | `native_ui/src/system_tools.rs:1645–1661,1695–1707,1726–1745`：EOCD 条目计数、中央目录条目磁盘号检查；包含 Darwin 的 POSIX 链接类型、VFAT 重解析位以及本地头加密位被拒绝，保留路径和本地/中央头文件名校验。这里只核销原报告中明确列出的缺口，不等同 ZIP 安全认证。 |
| CR-P2-5 | 已修复 | `native_ui/src/system_tools.rs:1803–1822,1866–1882,2076–2168`：主路径与回退均检查目标类型；覆盖路径递归清理旧条目，清理或覆盖报错则不成功；发布后复检落盘树及 manifest，再写 `.archive-stamp`。 |
| CR-P2-6 | 已修复 | `native_ui/src/system_tools.rs:476–498,765–805`：无签名或无可用 MSI、且合法 release URL 不存在时，接入默认官方 Releases URL；按钮和摘要读取同一有效 URL，没有打开自动安装。 |
| CR-P2-7 | 已修复 | `extension/entrypoints/background.ts:2498–2563,3510–3527,2181–2207`：直接 HTTP(S) 菜单入口先查同 tab/frame/页面及 URL（含变体）；只有唯一匹配的双轨证据才传入 `audioUrl` 触发“先下载合并”，不继承被查资源的凭据。无匹配/歧义未被扩大成全站拒绝。 |
| CR-P2-8 | 已修复 | `native_shell/src/media/hls.rs:2724–2760,3120–3148,3175–3197,3293–3310,3389–3442`；`native_shell/src/media/subtitles.rs:55–85`：两条路径均拉取、缓存并使用对应初始化文件的 `X-TIMESTAMP-MAP`，并处理 MAP 切换；沿用原范围、密钥和代理通路。实现的内存回归见 P2-20。 |
| CR-P2-9 | 部分修复 | `native_shell/src/media/subtitles.rs:49–85,110–118`：同一时钟纪元内能展开 33 位回绕，已保存的 discontinuity 会清空展开状态；但直播清单滚动时未保存的边界可丢失，真实重置仍会被当作回绕，见 P2-19（同 PRA-P2-1）。 |
| CR-P2-10 | 部分修复 | `native_shell/src/media/dash.rs:4386–4456,4462–4489,9564–9632`：无结束边界、窄 DVR 的典型长直播已经能跳过过期段并推进 `$Number$/$Time$`，URL 预算仍生效；精确上限有一处分支仍先算被裁的边界段。以 `t=0,d=1,timescale=1,startNumber=1,live_end=1,500,000,DVR=1,000,000` 为例，理论保留 1,000,000 段，但 `skip=500,000-1` 后 cap 看到 1,000,001 个候选而拒绝，尚未运行下一层 `seg_end <= cutoff`。建议在预算计数前按同一裁剪谓词验证保守保留的边界候选，并测 999,999 / 1,000,000 / 1,000,001；不放宽 cap 及 URL 字节预算。这是旧拒绝行为的残余，不另计新增项。 |
| CR-P2-11 | 已修复 | `native_clients/src/native_host.rs:97–111,1015–1025,1092–1101,1147–1172`：新请求与已有 pending 的重试均到 `prompt_media_push_ui`；只有 `open_workbench()` 返回 `Ok` 才设请求 ID，同会话的 Core/启动失败仍可重试并记日志。这里只证明原“同步错误吞掉后还缓存”已修复，不把进程 spawn 成功视为窗口最终呈现实测。 |
| CR-P2-12 | 已修复 | `native_ui/src/activation.rs:93–151`、`crates/client/src/core_ipc.rs:620–689,1563–1634,1688–1721,345–370`：连接、写帧、读帧和退避在同一绝对总期限内，且取消仍等待 overlapped 操作收尾，未弱化管道属主及响应 ID 校验。Windows 专用运行路径本机未跑。 |
| CR-P2-13 | 已修复 | `native_ui/src/workbench.rs:681` → `native_ui/src/native_window.rs:1202–1264,1267–1335,1362–1414`：关闭最大化窗口优先读有效普通态矩形，换算工作区偏移和 DPI，查询失败/矩形无效才退回磁盘。不把计算单测等同于 Windows 多屏实测。 |
| CR-P2-14 | 已修复 | `extension/lib/nativeBridge.ts:170–190` → `extension/entrypoints/background.ts:2095–2120,2248–2272,2923–2947,3453`：队满在投递前以明确错误退出，新 offer 清 journal 并让原下载继续；已 journal 的恢复和可能已投递的超时保留不确定状态，不把 `handoff: null` 当成拒绝。 |
| PRA-P2-1 | 部分修复 | `native_shell/src/media/hls.rs:3274–3354,8132–8249`：同一次 poll 内，首片失败后若后续有成功片，边界正确随首个成功片保存；下一轮 poll 或暂停恢复没有待定边界，且未解析 discontinuity sequence（`hls.rs:143–159`）。见 P2-19。 |

#### 确认的新问题

##### P2-18｜预览缓存达到 256 项时，更新最老的当前任务会丢失尚未刷盘的就绪分片

- **定位**：`native_shell/src/media/preview.rs:452–496,506–544`（淘汰、重载、跳过旧快照）；生产读端 `native_shell/src/playback.rs:434–480`。
- **调用链和实证**：HLS 直播不走 VOD 分片 checkpoint（`native_shell/src/media/hls.rs:1166–1178,2520–2545,2588–2603`）；就绪读端的冷加载只读索引、旧标记及 VOD checkpoint，不读直播 `live_state.json`（`preview.rs:287–299`）。当前任务 A 的第 3 片已 `mark_ready` 但尚未成批刷盘。其后 255 个预览根可由 `is_ready` 的冷读进入缓存（`preview.rs:606–639`），A 成为最旧项。再 `mark_ready(A, 第4片)` 时，代码先选牺牲项再查找 A，遂移除 A、从旧索引重建 A、加入第 4 片。晚到的淘汰刷盘只见 A 键仍存在，便丢弃旧快照中的第 3 片。基线代码在移除时立即刷旧索引（`16324a8f^1:native_shell/src/media/preview.rs:339–355,378–386`），再装入 A 时会读到第 3 片。
- **验证/影响**：Linux 仓库外探针调用真实 `mark_ready/is_ready/flush_ready`，确认第 3 片原本可读，更新并显式刷盘后只剩第 4 片：`segment 3 before capacity: true; after marking segment 4 and flushing: false; persisted: false; segment 4 ready: true`。实际文件未丢失、任务最终合并不依赖该标记；边播边下的 HTTP 预览却会对已落盘文件返回缩略图 503 或等待直至超时。
- **建议**：只在插入新根且容量不足时淘汰；不淘汰当前更新根。让真正淘汰的旧根在同根重载完成移交前保持可读取的最新快照，或者让重载与淘汰写串行于同一个稳定的根锁；保留全局锁外 I/O。
- **建议测试**：在 `preview.rs` 用无 VOD checkpoint 的 HLS 直播式目录、明确的 LRU 顺序复现上述容量场景，断言第 3/4 片均就绪且显式 flush 后都在盘；另用 barrier 验证淘汰根与同根并发重载不会回退索引。

##### P2-19｜字幕 discontinuity 在直播窗口跨 poll 滑出后，被新的 33 位展开逻辑错判为回绕

- **定位**：`native_shell/src/media/hls.rs:143–159,2980–2991,3080–3108,3279–3307,3337–3353`；`native_shell/src/media/subtitles.rs:55–85`。
- **调用链和最小两轮场景**：启用字幕的直播每次迭代调用 `LiveSubSession::capture`（`hls.rs:931–958`），后者有自己的字幕清单拉取，执行完保存 track（`3064–3078,3240–3277`）。第一轮成功写 A（映射 `2^33−90000`），看到 `#EXT-X-DISCONTINUITY` 后 B 下载失败或标作 gap，且本轮没有下一片成功。第二轮播放列表合法滑窗，只剩新片 C，原边界已由上游 `#EXT-X-DISCONTINUITY-SEQUENCE` 表示；解析器只检查精确 `#EXT-X-DISCONTINUITY`，没有读序号，`pending_discontinuity` 又在每次调用重新置 `false`，持久文件只存已成功分片的布尔值。C 成功以 `false` 写存；结束时 `publish` 把 A、C 送入 `merge_webvtt_entries`（`1424–1426,3124–3148`）。C 的 raw `90000` 因此被展开为 `2^33+90000`，cue 从该实现期望的 `00:00:01.000` 变为 `26:30:44.718`。若错误边界和下一成功片仍在同一轮，现有 `hls.rs:8132–8249` 可通过，不能覆盖上述路径。基线独立按 raw tick 算偏移，不会对这个真实重置新加一圈。
- **影响**：合法滑窗加 gap/短时不可用即可触发，无须用户重启。字幕边车错误；有字幕的最终混流也读取该产物（`hls.rs:1463–1473`）。不会导致媒体分片丢失。
- **建议**：在字幕轨上按 media sequence 持久记录尚未消费的边界和已消费的 discontinuity sequence；解析并比较 `EXT-X-DISCONTINUITY-SEQUENCE`，直到首个在新纪元的有效分片提交才消费，不用任意见过的旧片清掉边界；保留同纪元连续帧的正常回绕。
- **建议测试**：沿用本地 HTTP 字幕夹具做双 poll，第一轮尾部有边界但无可用后片，第二轮携带上升的 discontinuity sequence 并让第一个后片成功，断言 cue 为 `00:00:01.000`，无 `26:30:44.718`；在两轮之间 reload `live_subtitles.json` 加一组复测。

##### P2-20｜初始化 MAP 虽只读一次，合并却给每个分片深拷贝一份完整头

- **定位**：`native_shell/src/media/hls.rs:3122–3148,3187–3197,3393–3414,3435–3442`；`native_shell/src/media/subtitles.rs:9–15,55–67`。
- **调用链**：开启字幕的常规 VOD 下载（`hls.rs:1428–1444`）或直播结束（`:1424–1426`）走各自的聚合。对同一个 `EXT-X-MAP`，缓存命中返回 `String::clone()`，每个 `WebVttSegment` 持有独立的 `Option<String>`；所有副本留到整轨 `merge_webvtt_entries`。合并只需从头读 `X-TIMESTAMP-MAP`，不需持有每片完整头。`read_subtitle_text` 的 8 MiB 阈值只限制单文件（`native_shell/src/media/mod.rs:16`），不限制克隆后的合计，也无流式收集或 MAP 头短路。改动前不会为分片分配这部分内存。
- **影响**：额外常驻文本为 `分片数 × 复用头大小`。例如共享 64 KiB 初始化头的 10,000 个短分片，仅副本就约 625 MiB，不含分片和 cue；压力在单一 Rust Core 中，可能使并行任务/UI 状态查询一并变慢。这里确认的是可计算的分配放大，没有宣称在 Linux 人为跑到 OOM。
- **建议**：按 MAP 缓存已解析、可复制的小型 timestamp 元数据；若需保留完整头则共享 `Arc<str>`，避免逐片深拷贝，同时保留分片自己的合法 timestamp-map 优先及切 MAP 行为。
- **建议测试**：合成 1,000 个引用同一 MAP 的短分片，验证不同分片共享同一头分配（或同一 MAP 仅解析一次），并对照 MAP 切换与分片本地头覆盖的 cue 时间，不以堆分配失败作为判定条件。

#### 验证与限制

- Linux 扩展：`pnpm install --frozen-lockfile && pnpm test`（加载已安装的 Node v24.22.2，pnpm 11.7.0）→ `Test Files  65 passed (65)`、`Tests  778 passed (778)`，包含 TypeScript 检查及三项扩展回归覆盖。
- Linux 媒体：`cargo test --locked -p hls-native-shell --lib 'media::'` → `test result: ok. 187 passed; 0 failed; 3 ignored; 0 measured; 920 filtered out; finished in 21.44s`。
- Linux 下载 Worker：`cargo test --locked -p hls-native-shell --lib 'download_worker::'` 中，覆盖 CR-P2-1 的 `auto_retry_governance_skips_permanent_failures_while_retrying_transient_errors ... ok`；较宽的整组未全绿，日志有 Linux 路径长度 `File name too long (os error 36)` 和其他失败，且 `t2_r8_magnet_metadata_proxy_preserves_global_manual_and_system_and_fails_closed_on_empty_manual has been running for over 60 seconds`，已停止该扩展检查。未将未做基线复跑的失败归为 #160 新缺陷。
- Linux Native Host（canonical 源码由 Core 单测 harness 编入）：`cargo test --locked -p hls-native-shell --lib 'native_host::tests::'` → `test result: FAILED. 44 passed; 2 failed; 0 ignored; 0 measured; 1064 filtered out; finished in 1.08s`。失败的是 `media_push_retries_open_workbench_after_transient_failure_without_dropping_pending_request` 与 `stale_pipe_reconnects_and_autostarts_core_on_explicit_media_push_with_idempotent_request_id`；两者均为 `DPAPI credentials require Windows`，在进入工作台唤醒断言前就失败。凭据分支需 Windows 验证，不能算本机通过，也未将该平台限制列为产品缺陷。
- Linux Presenter：`cargo test --locked -p hls-native-ui --bin hls-downloader-presenter` 未运行测试。`yeslogic-fontconfig-sys` 构建报 `The pkg-config command could not be found.`；已查本机未安装 pkg-config 或对应 fontconfig 开发包，Windows CI 结果不代表本机跑过 UI。
- P2-18 仓库外 Linux 源码探针：复现输出见 P2-18；未修改仓库增加测试。P2-19、P2-20 及 CR-P2-10 临界值为源码调用链与确定性状态/算术推演，不宣称已新增或运行对应回归测试。
- 未做 Windows 真机/UI 视觉、Win32 IPC、真实浏览器重启与真实投屏/TVBox 设备测试。上述边界不算本地通过；不将 CI 绿等同于发布放行。未发现此次变更改写 `file_dialogs::open` 白名单，或移除现有媒体请求的 SSRF/私网、代理与凭据隔离检查；不将 `net_policy::acquire` 不进入既定 `fetch_range` 视为缺陷。

### 6. PR #161 Windows 打包修复复核（`74848fe3`）

- 对照 [PR #161](https://github.com/ciaooo55/hls-downloader/pull/161) 合并前后的真实变更，使用同一 `origin/main@529fcbf4` 基线；`74848fe3` 后仅刷新了 `SYNC.md`。
- **9 项：8 项已修复、1 项部分修复（CR-P2-15）、0 项未修复；确认 1 条新 P2（P2-21），无新增 P0/P1。** 本表更新第一节对应的历史记录；SBOM 已补上本单点名的 curl-impersonate 和 Vulkan loader，但 FFmpeg 声明仍未绑定实际打包二进制。
- 当前公开发布仍按既定策略无签名；本节签名链结论限于可选签名工具，不代表公开 Release 已执行签名验收。

#### 逐项核销

以下文件与行号均指向所审 `main`，不是历史报告行号。

| 项目 | 结论 | 证据及核销范围 |
|---|---|---|
| CR-P1-3 · 同提交 MSI ProductCode 冲突 | **已修复** | `scripts/build-v7.ps1:34,332,515–534` 传入 tier、源提交、版本、候选清单哈希；`scripts/build/V7MsiIdentity.ps1:20–41` 默认加每次构建的新 GUID nonce，显式固定 nonce 时结果可复现，tier 不同则输入不同；`scripts/release/finalize-v7-signed-package.ps1:108–125` 为签名重打包另生成 ProductCode。MSI UpgradeCode 不变，且 `scripts/build/build-native-v7-installer.ps1:108–115` 的升级范围包含同版本。 |
| CR-P1-4 · 先签载荷，再签 MSI/Burn | **已修复（签名路径）** | `scripts/release/finalize-v7-signed-package.ps1:75–125,128–143` 先签 7 个 EXE，用同一份签名 preflight 和签名 app-image 重建，再更新清单；`scripts/build/build-native-v7-installer.ps1:49–53,130–132,139–179` 把 helper 放入 Binary 表，数据库修改完成后先签 MSI，Burn 链内嵌已签 MSI，最后 `insignia` 分离、签引擎、重附加并签外层。验签覆盖见 PRB-P2-1。 |
| CR-P2-15 · 第三方 SBOM / 构建版本一致 | **部分修复** | `config/release/third-party-components.json:4–54` 与 `scripts/release/generate_sbom.py:62–96` 产出 FFmpeg、libmpv、Vulkan loader、curl-impersonate 四项；`scripts/build-v7.ps1:97–136,159–224,366–367` 使用相同清单中的 libmpv/Vulkan/curl 下载配置。**FFmpeg 是例外**：构建只检查 JSON 有该 ID，实际从环境变量、任意匹配缓存或 PATH 取 EXE；随后只验证 `-version` 进程正常退出，不核实版本或哈希（`scripts/build-v7.ps1:109–117,439–486`）；`scripts/release/stage-v7-release.ps1:84–88` 只把产品版本传给 SBOM，不传二进制身份。官方 Release bootstrap 有自己的固定归档配置（`scripts/build/bootstrap-v7-media-tools.ps1:17–25,38–64`、`.github/workflows/release-v7.yml:139–146`），所以不声称标准 CI 已发错版本。修复建议：共享锁定 FFmpeg 版本与来源，进入包前核对实际二进制身份，或从构建证据生成 SBOM；验收用不同版本的可运行 FFmpeg 验证拒绝/如实记载，再比较实际包中第三方二进制与 SBOM。 |
| CR-P2-16 · 清理缺目录时继续 | **已修复** | `.github/workflows/release-v7.yml:325–339` 保留 `always()`，逐目录 `Test-Path -LiteralPath ... -PathType Container` 后才调用，单目录抛异常时继续下一个。浏览器及性能门禁确实产出这两个路径（`scripts/validation/verify-v7-candidate-browser.ps1:15,29–30`、`scripts/validation/benchmark-v7.ps1:9–10,18–21`）。PowerShell stub 分别验证了首项缺失与首项抛错。 |
| CR-P2-17 · Checkpoint 内嵌错误 / DLL、curl 锁 | **已修复（原报告具体场景）** | `scripts/install/installer-preflight.rs:41–92,908–958` 解析帧、校验类型/请求号、拒绝内嵌错误，再等 Core 退出；实际 Core 通过 `native_shell/src/core_server.rs:639–656,684–697` 返回 Events 或顶层 Error。锁检查覆盖安装根、resources 及 curl 工具目录的 EXE/DLL（`scripts/install/installer-preflight.rs:960–990,1040–1066`）；Windows 独占句柄测试与帧测试在同文件 `:1216–1278`。新加入的扫描对普通目录存在误报，见 P2-21。 |
| PRB-P1-1 · MSI Binary 流必须按原始字节读取 | **已修复** | `scripts/release/verify-v7-authenticode.ps1:38–71,109–117` 用 `ReadStream(..., [int]1)`，检查完整长度和 SHA-256 再验签；微软定义 format 1 为每字符一原始字节。Windows 合约测试创建真实 COM Binary 记录、覆盖所有字节值，并调用实际导出函数（`scripts/validation/test-v7-packaging-contracts.ps1:83–139`）。 |
| PRB-P1-2 · 无 PyYAML 的 Windows CI 仍能检查第三方清单 | **已修复** | `scripts/release/generate_sbom.py:44–47,62–84` 把 `import yaml` 限于 npm lock 解析；`scripts/validation/test-v7-packaging-contracts.ps1:140–203` 无条件验证 stdlib 分支，有 PyYAML 才生成完整 SBOM。本次用 `python3.11 -S` 独立通过。 |
| PRB-P1-3 · 安装目录与便携文件分开验 | **已修复** | `scripts/release/V7ReleaseScope.psm1:42–54` 分为六个 MSI 落地 EXE 和七个便携 EXE；`scripts/validation/verify-v7-exe-startup.ps1:156–177` 在真实 Bundle 安装后拒绝清单额外键，仅逐一校验六个安装 EXE 的必填 SHA-256；`scripts/validation/verify-v7-msi-lifecycle.ps1:121–137` 的失败诊断也只检查这六项，不把 Binary 表内的 helper 当作落地文件。 |
| PRB-P2-1 · Preflight 身份与签名内部产物验证 | **已修复（脚本契约）** | `scripts/build-v7.ps1:324–353,517–539,559–564` 候选只编译一次 helper，两种容器使用同一文件，候选提升要求该哈希存在且匹配；`scripts/release/verify-v7-authenticode.ps1:75–145` 校验签名后的 ZIP、MSI 行政解包 EXE、Binary 表 helper，以及有 WiX 时的 Burn 引擎与内嵌 MSI 签名和 MSI SHA-256。缺 WiX 时默认明确回报 `burn_inner_verified=false` 但仍可能 `passed=true`（`:144–145,176`）；做完整签名验收必须传 `-RequireBurnInnerVerification`，不能只消费 `passed`。 |

#### 新缺陷

##### P2-21｜普通 `.dll` 后缀目录被 preflight 误判为占用，升级停在替换之前

- **位置：** `scripts/install/installer-preflight.rs:960–985`（具体为 `:971–984`）。遍历得到 `DirEntry`，立刻丢掉类型信息，只按路径后缀判断；本 PR 把此前仅 `.exe` 扩展到 `.dll`。Windows Rust `OpenOptions::new().read(true).share_mode(0)` 对目录没有设置 `FILE_FLAG_BACKUP_SEMANTICS`；[CreateFileW 文档](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew#directories) 规定打开目录必须设置该标志。该打开错误不是文件消失，却被统一归为“程序文件仍被占用”。
- **复现与调用链：** 在完整、已退出的旧便携安装根下建一个普通空目录 `notes.dll`，不替代任何程序文件，不持有任何文件句柄。运行新便携包的升级脚本。旧目录的镜像检查不排斥这个额外目录（`scripts/upgrade-v7-portable.ps1:36–101,153–161`），`:165` 在移动目录之前调用 `:15–30` 启动新版 helper。helper 的 `prepare_install` 进入 Windows shutdown（`scripts/install/installer-preflight.rs:373–391`），无剩余进程时仍反复调用 `files_unlocked`（`:1043–1087`）。确定的目录打开失败最后清理超时并返回 1（`:27–37`），升级脚本抛错，不到达移动语句（`scripts/upgrade-v7-portable.ps1:165–169`）。MSI 对已有路径执行同一 helper，且 `Return="check"`（`scripts/build/build-native-v7-installer.ps1:130–132`），因此相同条件也会阻止覆盖安装／卸载。
- **影响：** 正常文件均未锁仍无法升级或卸载；会留下旧安装和用户数据，不是数据损坏。以前根目录的 `notes.dll` 不进入 EXE 锁检测；当前回归并不依赖伪造 IPC、签名或特殊权限。
- **建议改法：** 在锁探测前区分普通目录与文件，跳过普通目录；对元数据错误、重解析点和已知文件位上的异常类型保持现有防护，不要泛化忽略访问失败。
- **建议测试：** 在现有 Windows 锁测试中加一个名为 `notes.dll` 的普通目录，断言 `files_unlocked` 成功，然后在同一扫描范围持有实际 DLL 的独占句柄，仍断言失败。用带该目录的旧便携 fixture 走一次升级，验证交换目录成功且数据保留。
- **证据级别：** 由 Windows API 契约和可达的调用/错误传播链确认；本次 Linux 未运行该 Windows 复现。

#### 验证方法与局限

- GitHub [#161 checks](https://github.com/ciaooo55/hls-downloader/pull/161/checks)：`5 passed, 0 failed, 0 pending, 0 skipped`；含 Validate contracts、Rust Core、Native presenter、Browser extensions、test。Windows PowerShell 5.1 与 7 合约接入点是 `.github/workflows/ci.yml:65–66` 和 `scripts/validation/validate-powershell.ps1:115–118`。
- Linux PowerShell 7.4.13 实跑了 ProductCode 固定输入可复现、tier 分离、默认 nonce 重建分离与 MSI/Portable 6/7 项清单；输出 `Parsed 11 changed PowerShell scripts and modules.`。从工作流取出实际清理逻辑，以 stub 替代子脚本、测试目录替代真实路径；输出 `Cleanup YAML logic: second root processed after first root missing or throwing.`。未运行实际停进程动作。
- Python 3.11.18 隔离验证：`python3 -S` 导入输出 `stdlib-only: ['ffmpeg', 'libmpv', 'vulkan-loader', 'curl-impersonate']`；带 PyYAML 运行实际 `generate_sbom.py` 输出 `CycloneDX 1.6 1266`，断言四项第三方版本全部匹配。系统 Python 3.10 缺 `tomllib`，改用机器已装 3.11。**独立证实但非新回归**：包还始终复制 `vcruntime140.dll`（`scripts/build-v7.ps1:425–433`），共享清单及 SBOM 都未列出。若合规目标是全量清点，应一并从实际构建证据补齐；父提交也漏此项。
- 原样 Linux `rustc --edition 2021 --test scripts/install/installer-preflight.rs` 报 ``error[E0425]: cannot find type `PathBuf` in this scope``：`PathBuf` 导入在 `#[cfg(windows)]` 下而解析函数不受该条件限制（`scripts/install/installer-preflight.rs:7–8,310`），父提交已存在。通过 stdin 临时补导入、不改仓库源码后，非 Windows 测试输出 `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`；这不等价于 Windows 原生构建。
- 未执行 Windows MSI/Burn 安装、真实证书签名/时间戳、Windows 独占句柄、重解析点和加载映像的实机行为；未把源码中的路径规范化或显式句柄测试外推为这些场景均通过。若启用签名发布，建议在授权 Windows 环境用一次性测试签名证书，完整跑 `finalize → verify -RequireBurnInnerVerification → candidate→formal 同版本覆盖升级`，不触发 Release。

---

## 三、云端并行开发领地声明（动工前先登记）

> **防打架硬规则**：
> 1. 并行开发前必须核对 [`SYNC.md`](../../SYNC.md) 中的 `AVAILABLE_FOR_CLOUD`，**严禁**触碰 `LOCKED_LOCAL` 与 `NEXT_LOCAL_PLANNED` 中的任何文件，**严禁**修改根目录 `SYNC.md`。
> 2. 所有云端改动必须通过 `cloud/<topic>` 分支提 PR 到 `main`，不得直推 `main`，不得修改 `config/release/feature-parity.json` 的 `"release_ready": false`，不得打任何版本标签或触发 Release 工作流。

| 状态 | `cloud/<topic>` 分支 | 目标模块与任务说明 | 计划锁定文件清单（须完全位于 `AVAILABLE_FOR_CLOUD`） | PR 链接 & CI 状态 |
| --- | --- | --- | --- | --- |
| 已认领（开发中） | `cloud/protocol-wire-boundary-tests` | `AVAILABLE_FOR_CLOUD` 第 4 项：`crates/protocol` 帧编解码（4 MiB 上限、截断/长度不符/超限）、`CorePipeRequest`/`CorePipeResponse` 标签名与 `serde(default)` 向后兼容、`request_id` 映射、`Debug` 不泄露凭据、`CrawlSpec` `deny_unknown_fields`；`crates/client` `request_validation` 公共校验（保留设备名、HTTP 方法、URL 协议白名单、资源键归一化）边界回归。只新增集成测试，不改生产代码；若测试暴露真缺陷，写入本文件第一节而不改行为 | `crates/protocol/tests/wire_contract_boundaries.rs`（新增）、`crates/client/tests/request_validation_boundaries.rs`（新增） | 待提交 |
| 已合入 | `cloud/fix-p1-5-media-regression-gate` | `CR-P1-5`：校验浏览器媒体矩阵逐项通过，并将组件及扩展摘要绑定到候选包清单；认领基线 `origin/main@529fcbf4` / `SYNC.md #4`（UTC+8：2026-11-28 15:43） | `scripts/validation/verify_v7_received_regression.py`、`scripts/validation/smoke_extension_media.py`、`scripts/validation/test_v7_received_regression.py` | [PR #163](https://github.com/ciaooo55/hls-downloader/pull/163)（`305b8b76`） |
