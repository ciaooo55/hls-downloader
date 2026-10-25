# HTTP 无效范围响应的错误分类与重试

## 根因和改动

HTTP 引擎已经拒绝缺少或不匹配 Content-Range 的响应，避免拼接不可信数据。但任务层直接把 EngineError 转为字符串，丢失 RangeUnsupported 分类，调度器把它当作普通失败进行整任务自动重试。

复用现有自动重试与队列回归，增加返回 206、ETag 匹配但缺少 Content-Range 的真实本地 HTTP 场景。旧代码实际失败：任务尝试三次，错误码为 DOWNLOAD_FAILED，而不是 RANGE_UNSUPPORTED。见 results/range-hint-red.log.txt。

修复在现有错误转换位置保留范围拒绝分类，调度停止这种无法安全续传的自动重试，任务提供明确错误及重取地址提示。资源身份变化仍使用 RESOURCE_CHANGED。没有忽略范围校验、拼接错误字节或修改续传缓存格式；手动重试入口保留，普通暂时性 HTTP 错误继续重试。

## 已执行

宿主离线 test/release 编译通过，产品仅在 Windows Sandbox 运行。定向场景先红后绿；同一回归还核对 401/403/404、503 重试、资源变化、后续队列继续及不得发布错误成品。634 项常规 Core、4 项 curl、4 项 WinHTTP 回归通过，共 642 个不同回归；另 4 项 ignored 未运行。定向回归已经包含在 634 项中，不重复计数。

现有安装验收的 HTTP 故障组也增加了相同无效范围场景。最终安装包实测该任务尝试一次、后续任务完成、测试任务/成品/缓存移除；不是只测 debug 二进制。

## 浏览器组合验收的夹具失败

补验前批桌面 MSI 时两次 msiexec 超时，均未进入浏览器场景；沙盒诊断没有安装登记、产品进程或安装日志。第二次已使用本地 MSI/日志，因此不能认定共享日志路径是唯一根因。新脚本还遗漏了现有安装夹具对 INSTALLDIR 含空格值的特殊引号处理，复用相同命令构造后，同一 MSI 成功安装并核对五组件哈希。

随后 Edge 在第一个场景前因默认管道不存在而失败。前一个分组 runner 在同一 PowerShell 进程留下自定义 HLS_V7_PIPE，而浏览器夹具 Pipe 使用默认管道；修正后续 runner，明确移除测试 IPC 环境变量，不改生产程序来容忍夹具状态。失败报告和两次安装诊断均保留在 results/newcore-browser-*；这些失败不计为通过。

## 最终包与安装后验收

产品源码树为 de039b6，Engine SHA-256 为 b72a5e83edb87a9c5e1e754d0c48489658edd467cc6c236bced7ad7f9008c283，Native Host 为 bbcc0dd0fba23334b7955cb2f9dd65e99e1f870884e24f4258e1d0f0e15bc5a7。MSI SHA-256 为 88dfbf2f968f31d3d418f8a0e9b4b1aa1be8c1bbb6d545269eb71e2c022000cc，203444224 字节，与前一桌面 MSI 大小一致。UI/JRE/播放器/Updater/Presenter 和插件沿用原有构建，没有新增依赖。

同一沙盒先安装桌面旧包 f1e92a17，再覆盖为最终 MSI。安装前及首次 Host 启动前默认任务库均不存在。最终包通过 11 项安装验收：自选含空格路径、五组件及双插件 ZIP 哈希、正常默认管道四 Host 并发唯一 Core、系统 Downloads、公开 MDN 文件字节及 SHA-256、四组 HTTP 和六组 HLS 故障恢复/清理、闲置采样和卸载。旧产品登记、旧程序以及最终产品登记均移除。见 results/range-hint-installer.json 和 results/range-hint-upgrade.json。

四 Host 首响应最大值为首次新库 131.02ms、常驻 71.88ms、正常关闭后重启 121.69ms；仍使用 1500ms 门槛。单次新库样本不证明历史慢点已解决。安装占用 469604258 字节、222 文件，比上一包增加 529 字节，不能将 Core 闲置样本解释为整个 UI 的性能。

再次从同一 MSI 安装并核对组件后，真实 Edge 和 Firefox 各 18 项接管场景通过，报告记录实际 Engine/Host 与插件 background 哈希。覆盖明确链接、GitHub EXE、取消、不同 MIME 附件、脚本/重定向/表单/新标签、POST/Blob 导入、禁用、排除与离线回退。确认决策使用 Core IPC；未验收原生确认按钮与浏览器下载栏视觉。Chromium 通用附件创建后取消、POST/Blob 经浏览器保存导入的现有限制仍存在，不能据此声称所有下载都在浏览器零反应。

浏览器 suite 完成后卸载成功，产品进程为空、产品登记不存在、可执行文件移除，见 results/range-hint-browser-suite.json。桌面同一目录的安装包、双插件、完整插件源码和 Firefox 审核源码已更新；七项资产及 SHA256SUMS 核对通过，完整源码 ZIP 的 124 个文件逐一匹配 HEAD Git blob。见 results/desktop-delivery.json 和 results/delivery-audit.json。仍为 release_ready=false 候选，未更新 GitHub Release。

本批未重新执行 UI/插件构建或 400 项历史插件测试；真实浏览器已重新运行最终 Core/Host 的组合，不能把历史插件测试重复计数。本机只读检查未发现产品进程、已知产品卸载登记或已检查的浏览器 Native Host 项，未进行注册表写入；核查范围见 results/host-readonly-audit.json，不能解释为扫描过整个系统的每一处历史痕迹。

## 限制

本轮不重建未修改的 UI/插件，不调用云端，不在宿主安装，不更新 GitHub Release。指定直播站点 403、原始 HLS 卡死现场、新库初始化历史慢点、Chromium 通用附件零下载项及 POST/Blob 边界、完整 UI/实体设备/长期负载与 IDM/AB 全面对等均未因此获得证明。

历史构建缓存删除被自动审批拒绝，本轮未换通道重试；生成物仍集中在项目 .tool-cache，不能称工作目录只剩源码。继续保留 main 单分支、单 worktree。

最终沙盒核对三批产品登记均不存在、产品进程及已检查的 Native Host 项为空；随后按本轮保存的 PID 与创建时间关闭沙盒及其客户端，最终两个进程均不存在。见 results/vm-product-handoff.json 和 results/vm-closed.json。未删除用户下载、备份、密钥或其他项目。

## 用户再次要求清理后的状态

2026-10-26 用户再次要求清理后，确认只有 main、一个 worktree，源码无未提交；桌面七项成品资产哈希一致。发现仍有 .tool-cache、桌面 Gradle/Kotlin、插件构建/依赖及 Python 缓存，另有 outputs/private-user-backup 用户备份，未将备份列为删除目标。

在路径范围、目标非重解析点及 Git 不跟踪目标的检查命令后尝试直接删除上述生成目录，整个命令在启动前再次被自动审批以 blocked by policy 拒绝，未给出具体原因，没有任何删除发生，也未换通道重试。

已更新现有的 [手动清理脚本](../live-presenter-20261025/finish-cleanup.ps1)，没有增加新的收尾脚本。脚本核对当前桌面安装包哈希、项目范围、全部目标的 Git 跟踪情况和链接边界后才删除明确列出的生成目录，保留用户备份、桌面成品和全部源码。代理仅完成静态语法和差异检查，未执行此脚本；用户手动执行前不能将清理记录为完成。

