# 2026-10-24 硬件解码、Chromium 接管与安装验收

产品源码为 `be7f646e2e4439560b0d3531c96d859f945dfd76`。本轮用户已授权恢复依赖、构建、沙盒验证、未签名交付、同步与清理；没有在宿主安装或运行产品，也没有调用收费云端或签名。

## 改动与实际结果

播放器设置 `hwdec=auto-safe`，由 mpv 选择安全的硬件解码方式，不支持时回退。没有新增依赖或更换默认 `gpu-next` 渲染器。另收紧公开网站接管验收：Chromium 确认前必须暂停，接管后匹配下载项必须为空，不能用 `exists=false` 代替。

| 本轮范围 | 实际结果 | 证据 |
| --- | --- | --- |
| Core release 构建 | 成功，锁定依赖 | `results/core-build.log` |
| 播放器 / HTTP 引擎回归 | 分别 3 / 59 项通过，0 失败 | `results/gpu/core-*-tests.stdout.log` |
| HTTP 边界、真实断流与重启续传 | 13 项通过，成品 SHA 匹配 | `results/gpu/http-boundaries.json` |
| GPU / 无 GPU 边下边播与清理 | 两种环境各 27 项通过 | `results/{gpu,software-verified}/download-playback-cleanup.json` |
| Edge 真实 Native Host/Core 接管 | 36 项通过：接受、取消、失败、通信中断、POST、multipart、Blob/Data、禁用与排除等 | `results/browser/takeover.json` |
| 7-Zip 官网真实下载 | 6 项通过：确认前暂停、一个 Core 成品 SHA 匹配、接管后浏览器下载项为空 | `results/browser/public-takeover.json` |
| 新 MSI 安装和卸载 | 9 项通过：自选带空格路径、组件哈希、系统 Downloads、MDN 下载、UI 连接、卸载注销等 | `results/browser/installer.json` |

媒体矩阵包括 HTTP、HLS TS/fMP4/AES、外挂音轨和 DASH，暂停保留断点、拖动优先下载、独立缩略图、同窗口切成品、完成清缓存、删除未完成任务清缓存并保留无关文件、关闭播放不重开和保留临时文件设置。逐项边界以 JSON 为准。

Edge 为 156.0.4314.42，浏览器自动化以 headless 模式运行，确认按钮操作实际桌面 Presenter。安装后的主界面真实可见且连接 Core；截图保留于 `results/browser/installer.png`。没有把这些结果描述为完整人工目视验收或所有浏览器全量复跑。

## 性能及未解决限制

同一 640×360、25 fps、20 秒测试视频，初始化后采样 5 秒，沙盒 8 逻辑 CPU：

| 环境 | 旧 Core | 新 Core |
| --- | --- | --- |
| GPU 开启 | `hwdec=no`，整机 CPU 0.234%，RSS 120.85 MiB | 实际 `d3d11va`，整机 CPU 0.273%，RSS 116.52 MiB，采样掉帧 0 |
| GPU 禁用 | 软件解码与渲染 | `hwdec-current=no`，正常回退；整机 CPU 62.266%，采样掉帧 97 |

硬件解码生效有播放器自身诊断支持；小视频 CPU 差异不足以证明性能提升。无 GPU 软件渲染仍高占用，**尚未解决**。实验使用另一内置 `vo=gpu` 渲染器仅有限改善，未用于生产；完整对照保留，未通过降低画质掩盖问题。不能保证所有驱动、格式与机器表现，也没有 IDM/AB 同机基准。

新 MSI 203468800 字节（194.04 MiB），比上一交付增加 40960 字节。安装目录 469511994 字节（447.76 MiB），222 文件，比上一交付增加 25055 字节。主要是现有 FFmpeg、libmpv、裁剪 Java runtime；完整大小在安装报告。UI 可见连接后的 5 秒空闲样本整机 CPU 0.078%，产品进程 RSS 合计 206.90 MiB，不能作为长期或跨电脑保证。

Chromium 普通 MV3 接管路径仍是在下载项创建后暂停，不能承诺浏览器从未创建下载项或完全没有下载 UI 反应。本轮证实确认前暂停、接管完成清除匹配项、拒绝/失败/通信中断后正确回退。Firefox 的响应前阻断沿用上一轮结果，本轮没有重新执行 Firefox 矩阵。收费字幕、实体投屏/TVBox、所有网站及 DRM 不在本轮范围。

## 交付来源

Engine SHA256：`39bc4856824e31610cb478409d423031f647a1a03f66c41b08d3a4e1c5fddc48`。

MSI SHA256：`ecabc6cb6facbf55fed6aa4c6ee1a4256fdc7ebb6b809703fa22225aa3cefb01`。

复用上一轮验收的 UI、Host、Updater、FFmpeg、libmpv 和插件，仅替换新构建的 Engine 与来源文件，并应用既有安装生命周期顺序。安装报告逐个核对组件哈希。插件源码未变，三个交付 ZIP 哈希保持不变，未重新下载 Node 依赖或冒充重新编译。完整插件源码另从本轮交付提交导出。

包内 `BUILD-PROVENANCE.json` 如实保留打包时 `candidate`、`git_worktree_clean=false`、`release_ready=false` 和“待验证”的历史状态，不事后改写已验收二进制。外部交付清单记录本次验收结果；仍为未签名预发布，非正式签名发行。

## 验收夹具问题及证据保存

早期出现 PowerShell 进程退出码读取、Expand-Archive 模块、嵌入 Python 搜索路径、可选 WMI 查询和浏览器路径空格引号问题。修正的是临时验收环境，不算产品缺陷。GPU 首轮矩阵的浏览器 argparse 失败保留在 `results/gpu/summary.json`，通过的播放与 HTTP 阶段保持原结果；随后 `results/browser/summary.json` 从接管阶段续跑并全部通过。无 GPU 完整结果为 `results/software-verified/summary.json`。未改写旧失败为通过。

保留本轮使用的沙盒 runner、配置和关键日志/JSON；runner 依赖临时准备好的输入目录，不是单独可运行的发行脚本。不提交依赖、媒体、安装包或构建缓存。公开网站报告中的短期签名下载 URL 查询参数已脱敏，文件大小和 SHA 未改动。源码静态差异检查和播放器 rustfmt 检查已执行通过。

本轮结束后仅保留一个 `main`、一个 worktree，桌面成品在 `C:\Users\lee\Desktop\HLSDownloader\最新-20261024`。交付来源、清理结果和 GitHub 发布身份分别见 `delivery.json`、`cleanup.json` 与 `publication.json`。新 `v7.0.3-r13` 未签名预发布的九项资产均已核对远端 SHA；旧 `v7.0.3-r12` 发布与标签已删除，其他主版本未操作。

首轮发布上传返回 HTTP 404，未删除旧发布。随后先建立草稿，逐项上传时确认 GitHub 去除了资产文件名中的中文，使两个 Firefox 包名称冲突；已按哈希识别并改为不同的英文资产名，内容未变。远端 SHA256SUMS 使用远端英文名称，桌面保留中文名称及自己的校验表。

构建缓存、临时验收目录和旧桌面包合计约 2.71 GiB 已清除。本轮上传另产生的 `.tool-cache/release-upload` 仍含一个 Firefox 插件副本与校验表，合计 330317 字节；最终清理命令被自动审批以 `blocked by policy` 拒绝，未给原因，未换方式绕过。因此不能宣称工作目录已完全清空生成物。原有用户备份 `outputs/private-user-backup` 保留。两个旧 `.hls-tasks/task-1/control` 共 8 字节此前同样被拒绝删除，仍保留。
