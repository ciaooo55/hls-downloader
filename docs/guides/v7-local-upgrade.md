# 安装、升级与用户目录

当前源码产品版本由 `config/release/feature-parity.json` 决定；最近公开包为 7.0.3-r12 未签名预发布。包的身份和实测范围见 [r12 说明](../releases/release-v7-r12.md)，不由本机是否安装决定。

## 普通 MSI 安装

- 安装向导允许选择安装位置；升级保留用户已明确选择的位置。
- 默认下载位置通过 Windows Known Folder 获取当前用户的“下载”文件夹，不使用开发机盘符或用户名。用户可在设置中修改。
- 常规安装的数据默认位于 `%LOCALAPPDATA%\HLS Downloader\v7`，数据库为其中的 `data.db`。数据目录与程序安装目录、下载成品目录分离；显式配置 `HLS_V7_DATA_DIR` 时以配置为准。
- Portable 使用自身根目录下的 `data` 和 `downloads`，不与普通安装共享状态。
- 安装镜像包含运行所需 JRE、Core、Native Host、Presenter、FFmpeg 和 libmpv。生产包不包含验收专用 ffprobe，详见 [包体积](package-slimming.md)。

未签名包通过手动运行 MSI 更新。自动更新的签名和项目签名者信任检查保持启用，不以关闭检查替代签名。

## 升级与卸载

升级前结束活动下载，备份任务数据库及个人设置；密钥备份不要提交到源码。退出 UI 不一定结束驻留 Core，应等待安装器完成进程协调后再替换文件，避免数据库或程序被占用。

使用 Windows 正常卸载流程移除产品。卸载程序与删除下载成品是不同操作；不应清空“下载”文件夹、共享浏览器配置、工具链缓存或浏览器注册表父键。清理 Native Messaging 时只处理确属本产品且已失效的叶子登记。

## 专用维护脚本

`scripts/install/install-v7-local.ps1` 是发布维护环境的事务安装脚本，**硬限定 `E:\h`**；这不是普通 MSI 的默认路径或用户安装限制，也不用于描述当前电脑的安装状态。

该脚本读取候选/正式 manifest，校验版本、来源及哈希，暂存完整镜像后切换并注册 Host；失败时恢复旧镜像。它会读取 [升级提示](../releases/v7-desktop-upgrade-note.md)，因此该文件必须保留。执行前应阅读脚本的路径、权限和状态备份要求，不直接套用于其他安装目录。

候选构建用于收集证据；正式构建另有 canonical completeness、readiness、同提交证据和干净工作树门禁，见 [发布 runner](../releases/v7-release-runner.md)。构建成功不能替代安装验收。

## 历史数据

首次创建普通 v7 数据库时，Core 可对精确识别的 v6 数据库建立 SQLite 一致快照并校验后迁移；已有 v7 数据库、Portable 或显式数据目录不触发整库覆盖。迁移失败保留源数据。

旧版行为通过同一仓库的 Git 历史查看，无需另存源码或创建 worktree。
