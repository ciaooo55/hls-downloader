# main 源码与本地测试交付（2026-10-19）

本轮将此前未提交的产品修改、回归代码和验收证据整理到 `main`。产品源码提交为 `7c42c864fe5d3d04a5fc0106f0441bec4fd61fa5`；之后的提交仅补充安装验收脚本、记录和交付说明，不改变产品二进制。未推送或创建 GitHub 发布，未签名，未在宿主机安装或运行产品。

## 桌面交付

`桌面/HLSDownloader` 只放三份文件，具体绝对路径、字节数和 SHA256 见 [desktop-artifacts.json](desktop-artifacts.json)：

- `HLSDownloader-7.0.3-main-7c42c86-未签名.msi`：约 193.6 MiB，安装时可选择目录；默认下载目录由 Windows Known Folder 获取，不是固定盘符。
- `HLSDownloader-7.0.3-Chromium.zip`：Edge/Chrome 解压后，在扩展管理页面打开开发者模式，选择“加载解压缩的扩展”；保留解压目录。程序内也有插件安装入口。
- `HLSDownloader-7.0.3-Firefox-未签名.zip`：Firefox `about:debugging#/runtime/this-firefox` 中“临时载入附加组件”；未签名包不能当成正式签名商店扩展永久安装，重启浏览器后需重新载入。

插件源码保留在仓库 `extension/`，无需再存一份重复源码压缩包。MSI/EXE/Portable 的构建原始清单保留用于溯源；本次仅交付 MSI，清理掉其余可重建产物。

## 实际执行的验证

- 当前插件类型检查、381 项测试、Chromium/Firefox 构建成功；Firefox lint 为 0 error / 0 warning / 0 notice。见 [extension-tests.log](extension-tests.log)、[firefox-lint.json](firefox-lint.json) 和 [package.log](package.log)。
- Core、Native Host、Updater、Presenter release 及桌面 JAR、MSI/EXE/Portable 构建完成。上游锁定的播放库归档失效；构建脚本改为可复用同版、固定 SHA256 的已验证 DLL 缓存，没有更换播放引擎或放宽校验。原始失败日志与最终构建日志均保留。
- 核对 82 个源码文件与此前逐模块验收证据，全部匹配，见 [source-evidence-match.json](source-evidence-match.json)。这不是本轮重新运行全部 Core/UI 测试。
- 最终 MSI 在全新 Windows Sandbox 中通过：含空格的自选安装路径、12 份组件/插件 ZIP 与构建输出的哈希一致、安装中不含 ffprobe、默认系统下载目录、真实 MDN WebM 字节一致、下载完成缓存清理、删除成品、界面可见并连接 Core、卸载及移除 MSI 产品登记。见 [installer.json](installer.json)、[界面截图](installer.png) 和安装/卸载原始日志。
- 安装占用 469,405,456 字节、222 个文件；5 秒闲置样本的进程 RSS 合计约 220 MiB，8 逻辑 CPU 的整机 CPU 约 0.078%。仅为本次沙盒样本，不代表跨电脑或播放/下载负载下的保证。

## 首次失败及范围

[first-attempt/installer.json](first-attempt/installer.json) 保留首轮失败：旧夹具要求安装根目录必须有预解压插件，而当前程序从安装 ZIP 按需解压。修正夹具后，严格核对 ZIP 哈希、CRC、版本和入口；若存在预解压副本仍逐文件比较。没有往 MSI 再塞一套重复插件。本轮没有实际点击“首次解压插件”入口，相关实现与此前测试不等于本轮入口实测。

本轮未调用真实字幕云端、未验证实体投屏/TVBox、未逐一访问所有网站，也未测试真实系统关机/睡眠或旧版升级数据迁移。功能合同中仍有两项 partial；不能称为所有功能、所有细节已完全认证。用户测试包与此目录的清单哈希对应。

## 收尾状态

桌面三份交付包复制后已再次核对哈希，验收沙盒已关闭。本机定向查询未发现本产品 MSI 卸载登记、浏览器专用 Native Messaging 子键或 HLSDownloader 进程；没有操作其他软件的注册表项。

九个项目内可重建目录的删除被自动审批拒绝，仅返回 `blocked by policy`，没有具体原因；命令没有执行，因此缓存尚在。已核实这些目录均不含 tracked 源码。用户可审阅后在 PowerShell 手动执行本目录的 `finish-cleanup.ps1`：脚本先核对桌面交付包哈希、项目边界及 tracked 文件，再只删除固定清单，保留源码、Git 和下载文件。代理未通过其他工具重试被拒绝的删除操作。后续是否实际删除以脚本执行结果为准。
