# 安装、升级与用户目录

当前源码产品版本由 `config/release/feature-parity.json` 决定。包的身份和实测范围见[验证状态](../validation/v7-verification.md)。

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

## 构建与验收

候选构建用于收集证据；正式构建另有 canonical completeness、readiness、同提交证据和干净工作树门禁，见 [发布 runner](../releases/v7-release-runner.md)。构建成功不能替代安装验收。

## 历史数据

Core 只打开当前版本数据库，不自动导入旧版本设置、任务或缓存。旧数据不会被覆盖或删除。

旧版行为通过同一仓库的 Git 历史查看，无需另存源码或创建 worktree。
