# MSI 图形安装 2819 修复

此前交付的 MSI 静默安装通过，但图形向导从许可页跳转到目录页时发生 2819。用户本机原始日志明确记录 `InstallDirDlg, Folder`；只测 `/qn` 绕过了出错的界面，属于验收遗漏。

## 原因及修复

实际 MSI 的目录框是 `Property=WIXUI_INSTALLDIR` 的间接控件，但 Property 表中没有 `WIXUI_INSTALLDIR`。许可框仍包含 WiX 默认的 `Lorem ipsum`。这与 [Microsoft 的 2819 定义](https://learn.microsoft.com/en-us/windows/win32/msi/windows-installer-error-messages)和 [WiX InstallDir 绑定要求](https://docs.firegiant.com/wix3/wixui/dialog_reference/wixui_installdir/)一致。

`dark.exe` 会将标准向导还原为 `UIRef WixUI_InstallDir`，同时保留 jpackage 自定义的 `InstallDirNotEmptyDlg`。旧重打包脚本只在“完全没有 Dialog”时补属性和许可证；由于该自定义对话框存在，两个必要设置都被跳过。

修复 `scripts/build/repack-v7-msi.ps1`：始终确保目录属性绑定到 `INSTALLDIR`，按实际 WiX 表示设置项目许可证；完成链接后打开最终 MSI，检查目录属性并逐字比较完整 RTF，任一缺失就终止交付。没有修改产品运行二进制或浏览器插件，也没有增加产品依赖。

## 实际图形验收

在 Windows Sandbox 观察到完整 MIT 许可证、正常目录页，实际打开“Change”输入 `C:\QAWork\Wizard Path\HLSDownloader\`，确认后进入安装确认页。最终观察到安装完成页；原始 [wizard.log](wizard.log) 记录 `INSTALLDIR` 为上述含空格路径，安装返回 0，无 2819。见 [许可证](license.png)、[自选目录](custom-directory.png)、[安装确认](custom-directory-ready.png)和[安装完成](wizard-completed.png)。

验收期间检测到人工输入、前台窗口切换；不将整条点击链宣称为代理独立完成。[wizard-harness-result.json](wizard-harness-result.json) 的 `passed=false` 来自夹具预期“取消后返回 1602”，实际用户继续安装返回 0。该文件原样保留，不改写为成功；产品图形安装结果由 Windows Installer 原始日志和完成页证明。

另一个全新沙盒执行最终包的静默安装、12 份组件哈希核对及卸载，结果单独保存在 `result.json`。此轮只验证安装修复，不代表重新执行所有产品功能测试。本机未由代理安装或运行产品。

复测结果：安装和卸载均返回 0，12 个组件哈希全部匹配，卸载后产品登记与安装程序文件均不存在。桌面同名 MSI 已替换并核对 SHA256 `8da9e4073075afaa7a176d75736262f01c6bf22681908ccea5a9050a21db8e8d`；大小仍为 203,005,952 字节，两份插件文件不变。两轮沙盒已关闭。

本轮临时目录 `.tool-cache/installer-ui-fix` 的删除再次被自动审批拒绝，仅返回 `blocked by policy`，未提供具体原因；删除命令没有执行。其余此前九处缓存已由用户清理。当前仍可手动运行 `docs/validation/delivery-20261019/finish-cleanup.ps1`，该脚本使用的桌面哈希清单已更新，先检查交付包，再清理固定的项目内生成目录。不要将此状态理解为本轮临时缓存已清空。
