# 网页下载处理器与插件接管顺序

## 确认的根因和修复

旧 Chromium 插件在 window 捕获阶段取消链接点击，并调用 stopImmediatePropagation。网站的目标处理器和 document 委托处理器因此无法生成实际导出地址；插件直接把原始 href 交给桌面。它可能是占位地址、旧签名或另一份文件。

使用当前桌面旧插件、真实 Edge、已安装的 Native Host/Core 复现：点击 href 为 /stale.zip 的按钮，页面本应取消默认导航并访问 /scripted-export，但桌面收到 /stale.zip。见 [失败证据](results/edge-red.json)。此失败发生在地址断言，没有接受错误地址并把下载成功当作通过。

修复保留 pointerdown/click 捕获阶段的意图记录，将普通链接的实际拦截移至 window 冒泡阶段，尊重 defaultPrevented，并移除 stopImmediatePropagation。网页已接手的点击交给现有请求/附件接管路径，普通链接继续直接交接。未新增生产依赖、配置或接管分支。

回归加入按钮自身处理器与 document 委托处理器两种场景。两种场景都必须交接真实导出 URL，两个旧 href 的请求数必须为零；接受后校验 Core 成品的全部字节并删除沙盒测试任务和文件。

## 实际执行的验证

产品源码提交 ced9ef5。类型检查与 400 项现有插件测试通过，见 [测试日志](results/extension-tests.log.txt)。Chromium、Firefox 构建及 ZIP/Firefox 审核源码打包成功；Firefox web-ext lint 为零错误、零警告、零提示，见 [校验结果](results/firefox-lint.json)。未重编译或重跑未修改的 Core/UI 单元测试。

产品只在 Windows Sandbox 安装、运行。真实 Edge 156.0.4314.42 与 Firefox 158.0.1 各 20 项接管场景通过，见 [Edge](results/edge-green.json)、[Firefox](results/firefox-green.json)。确认决策使用 Core IPC，浏览器输入分别使用 CDP 鼠标和 WebDriver 点击；没有把这些结果称为原生确认按钮或下载栏视觉验收。

场景包括普通明确链接、GitHub EXE 取消、无扩展名附件、两种网页脚本处理器、多种 MIME 附件、脚本导航、重定向、GET 表单、新标签、POST/Blob 导入、禁用、排除和离线回退。报告记录浏览器下载创建事件；Chromium 通用附件发生创建后取消，Firefox 的相应导航附件场景没有创建事件。POST/Blob 在两种浏览器中均经浏览器保存后导入，必须只有一次原始请求、一个 Core 成品，随后移除浏览器源文件。

本轮 Engine/Native Host 与基线完全相同。仅 background.js 哈希不能证明这次修复已进入插件：实际改动位于 content-scripts/takeover.js。已额外保存 [双插件 ZIP、接管脚本及基线脚本哈希](results/artifact-identities.json)，并逐文件核对验收所用解压目录与构建 ZIP 一致。后续同类验收脚本也记录接管脚本哈希。

## 夹具失败与修正

第一次安装从沙盒映射路径传入 MSI，返回 1619；复制到虚拟机本地后安装成功。PowerShell Start-Process -Wait 创建的进程树 Job 阻止 Core 使用 breakaway，改用 PassThru 和 WaitForExit 后，同一 Native Host/Core 正常启动。没有为夹具修改产品启动行为。

首次绿色 Edge 场景在 GitHub 链接前中断：新增链接使文本换行，包围框中心落在不可点击空白处，既没有点击意图也没有桌面交接。改用滚动后首个实际 content quad 定位，随后 20 项通过。该失败保留在 [夹具点击失败](results/edge-fixture-missed-click.json)。另修正早期失败诊断过早引用局部 events 函数、导致错误报告丢失的问题。

首次升级验收将读取字段 speed_limit_kib 误作保存键，升级前并未设置所需限速值；正式 UI 使用 download_speed_limit_kib。修正夹具并增加升级前读回断言后重新验收。失败不计入通过，也不据此认定产品升级丢失设置。见 [夹具问题记录](results/harness-failures.json)。

## 最终包与交付

最终 MSI SHA-256 为 7cc0f1e14ee234bfe6de5e6dce7858b4755b412938de3c2100aac10ac55aebca。包大小 203444224 字节，与上一桌面包相同；安装后 222 个文件、469604467 字节，比基线增加 209 字节。没有将这一差值解释为运行内存或 CPU 性能改善。

从桌面旧 MSI 安装后，在 Core 运行时执行同版本覆盖升级，旧 ProductCode 移除，新 ProductCode 登记成功。先在旧程序写入并读回设置，实际完成一个字节已知的下载；升级后同一任务、成品全部字节及设置保留。新包安装后的五个程序组件、构建来源记录和双插件 ZIP 哈希均与预期一致。删除沙盒 canary 任务、文件和缓存后卸载，最终产品登记和可执行文件移除。见 [升级及卸载](results/installer-upgrade.json)，首次失败单独保存在 [错误保存键夹具](results/installer-upgrade-wrong-setting-key.json)。本批没有重跑首次安装向导或原生窗口按钮。

桌面沿用 C:/Users/lee/Desktop/HLSDownloader/最新-20261025 目录及安装包文件名，更新其安装包、双浏览器插件、完整插件源码、Firefox 审核源码和交付记录。完整插件源码 ZIP 的文件集合与 Git 中的 123 个受控文件完全相同，每个文件逐字节匹配 ced9ef5 的 Git blob；Firefox 审核包也核对本次修改文件及构建配置。最初 git archive 子树受到本机 core.autocrlf 影响而转换了行尾，核对失败后重新以 core.autocrlf=false 导出；只有核对通过的包才覆盖桌面。见 [交付审计](results/desktop-delivery.json)。

沙盒卸载后再次核对：新旧产品登记不存在、产品进程为空、已检查的 HKCU/HKLM 32/64 位 Native Host 项为空，见 [沙盒收尾](results/vm-product-handoff.json)。按本轮保存的 PID 和创建时间关闭沙盒及其客户端，两者均退出，见 [进程关闭记录](results/vm-closed.json)；没有打印沙盒客户端命令行。

更新现有清理脚本的成品哈希及本轮 pnpm 项目索引位置后，已实际执行成功。随后复核 .tool-cache、插件依赖/构建输出及列明的其他生成目录均不存在；宿主三个产品窗口状态文件及空目录已删除，用户备份和下载保留。桌面七项资产再次核对一致，本轮没有在宿主安装产品或写注册表，保留 main 单分支、一个 worktree。见 [清理结果](results/cleanup.json)。历史报告中的清理阻断不代表本轮现状。

## 对照依据及边界

实际读取 AB 浏览器插件提交 658a0c70188d78033f478ba51c102a273322166f 的 [DownloadLinkInterceptor](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/DownloadLinkInterceptor.ts) 和 [Manifest3DownloadLinkInterceptor](https://github.com/amir1376/ab-download-manager-browser-integration/blob/658a0c70188d78033f478ba51c102a273322166f/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)。它在 MV3 中明确不能阻断响应，使用下载事件取消/移除浏览器项。对照其请求及下载阶段的处理，修复本项目过早抢占网页点击的问题；没有复制其代码。

[Chrome 官方 webRequest 文档](https://developer.chrome.com/docs/extensions/reference/api/webRequest)说明普通 MV3 扩展没有 webRequestBlocking 权限。AB 的现有实现也不能证明用户要求的“任何 Chromium 下载都不创建下载项”。IDM 闭源，本轮没有声称读过其内部实现。

这次修复证明目标处理器和 document 委托处理器场景可用，不证明任意站点的任意脚本行为、全部浏览器或所有下载入口都已覆盖。Chromium 通用附件零下载项、POST/Blob 无浏览器保存中间态、指定直播站点 403、原始 HLS 卡死现场、历史新库启动慢点，以及完整 UI/云端/实体设备/长期负载仍未闭环。因此交付保持未签名候选，release_ready=false，不更新 GitHub Release。
