# 2026-10-24 软件渲染性能修复

本轮仅处理无 GPU 播放占用和性能验收缺口。沿用用户已经批准的构建、GPU / 无 GPU 沙盒验证、未签名交付与清理范围；未在宿主安装或运行产品，未调用收费云端，未签名。

## 根因与改动

Windows 沙盒关闭 vGPU 时，mpv 的 D3D11 WARP 在 CPU 上执行视频渲染。仅设置硬件解码自动选择不能降低这部分软件渲染成本。360p 的选项隔离显示瓶颈在缩放而非抖动；1080p 进一步证实，双线性缩放仍受默认扩大的缩小滤波范围影响。

Core 通过系统 DXGI 1.1 枚举显示适配器，仅确认所有适配器都是软件设备时设置 `scale=bilinear`、`dscale=bilinear`、`correct-downscaling=no`。存在硬件设备、枚举失败或无法加载系统接口时保留原默认设置。没有新增依赖、播放引擎、后台线程或轮询。

这改变纯软件设备上的缩放质量取舍，可能使放大细节更柔和、缩小图像出现更多锯齿；不是像素无损优化。硬件设备保持原缩放、HDR 和抖动设置。DXGI ABI 已对照微软 [SDK 头文件](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/shared/dxgi.h) 核对；软件适配器判断沿用该版本 mpv 的 SOFTWARE / Basic Render 规则。缩放行为参考打包版本的 [mpv 选项文档](https://github.com/mpv-player/mpv/blob/7b8915bc1d/DOCS/man/options.rst)。

## 验收缺口与失败记录

旧脚本只要求播放时间前进，不能证明实时或流畅。新版保留原有实际子进程测试，固定 960×540 窗口，预热 5 秒、采样 10 秒，保存前后掉帧差值与实际播放速度。候选要求采样掉帧比例不超过 5%，播放速度至少达到请求速度的 90%。未指定门槛的基线及探索报告仅代表可播放。

- `results/first-candidate`：第一版只改双线性缩放的 360p 样本，CPU 从约 74% 降到 2.34%，但 5 秒掉 9 帧，未通过。
- `results/long-sample`：延长样本后，360p 为 2.15% CPU、0 掉帧；1080p 仍为 77.11% CPU、143 掉帧，未通过。旧基线的 1080p 在 10 秒内仅前进 1.64 秒，不能因采样掉帧为零称为流畅。下载 / 播放 / 清理 27 项实际检查通过。
- `results/1080-isolation`：在同一第一版 Core 上额外设置 `correct-downscaling=no` 后，1080p 为 1.875% CPU、0 掉帧、播放速度比 1.0；关闭抖动仍为 78.75% CPU、125 掉帧。整体 `fast` 配置也有效，但会同时改变其他画质参数，未整体用于生产。

以上均为 8 逻辑 CPU 沙盒的短时样本，CPU 百分比按整机计算。第一版 Engine 为 `4b08540b70da85aa596fade031414e91e6fe16b74f765f69aa924db4e0acea26`；旧 r13 基线为 `39bc4856824e31610cb478409d423031f647a1a03f66c41b08d3a4e1c5fddc48`。失败报告原样保留，不能作为最终候选通过证据。

## 最终候选

最终产品源码提交为 `4de9a58`，Engine SHA256 为 `57e07e0f2bd993d96ca701dad41257deb68f43ee5b2cadd276c36097b12591b2`。release 构建成功，证据为 `results/final-core-build.log`。

| 关闭 vGPU，同一视频 / 窗口 | r13 CPU | 最终 Core CPU | 最终采样掉帧 | 最终播放速度比 |
| --- | --- | --- | --- | --- |
| 640×360 / 25 fps | 70.99% | 1.58% | 0 | 1.00 |
| 1920×1080 / 25 fps | 87.20% | 1.84% | 0 | 1.00 |

r13 的 1080p 仅以请求速度的约 17.6% 播放，故其低采样掉帧不代表流畅。最终候选两个分辨率均达到新增门槛，实际诊断确认自动选择了 bilinear / 禁用扩展缩小滤波，并保持原 `gpu-next`、D3D11、HDR 和抖动设置。`results/software` 中另有最终 Core 的 27 项下载、播放和清理检查，全部通过。

开启 vGPU 后，最终候选的 360p / 1080p 整机 CPU 分别为 0.70% / 0.45%，采样掉帧均为 0，播放速度比均为 1.00。实际诊断仍为 `lanczos`、`correct-downscaling=true`、`d3d11va`，确认没有把软件路径的质量取舍应用到硬件设备。基线为 0.33% / 0.86%，这些短时差异不能证明硬件路径性能提升。

有 GPU 环境的 27 项下载、播放和清理检查也全部通过。覆盖 HTTP、HLS TS/fMP4/AES/外挂音轨、DASH；逐项检查及成品 SHA 见 `results/{software,gpu}/download-playback-cleanup.json`。

## 安装与交付

新 MSI SHA256 为 `fa0d637cd6fc4cb1f23374a69042f0f86eca5ad58d51ce7c3fc4468a2aa5e6bf`，203468800 字节，与 r13 安装包同样大小。Engine 增加 14336 字节；安装目录 469526377 字节、222 文件，比 r13 增加 14383 字节。没有通过增加依赖换取性能。

新 MSI 在沙盒完成 9 项实际验收：自选带空格路径安装、220 个已有镜像文件的哈希一致、插件 ZIP 完整性、Windows Known Folder 的 Downloads、MDN 公开媒体下载与清缓存、UI 可见连接、空闲测量、卸载移除产品注册。结果、安装日志和截图在 `results/gpu/installer.*`。UI 空闲 5 秒样本为整机 CPU 0%、产品进程 RSS 合计约 226 MiB，仅是短时样本，不能代表长期或所有机器。

桌面成品已更新在 `C:\Users\lee\Desktop\HLSDownloader\最新-20261024`，包括 MSI、Chromium / Firefox 插件、插件完整源码和 Firefox 审核源码。插件生产代码没有改变（与 r13 产品源码的 extension 差异为空），复用原包与源码 ZIP，未冒充重新编译或重测。

包内来源文件保留打包时 `candidate`、`release_ready=false` 和 GPU / MSI 待验证的历史状态；最终验收见本报告及 `delivery.json`。本轮未重跑 Firefox、收费字幕、实体接收设备、全部网站或完整按钮矩阵；Chromium 通用 MV3 接管仍先创建下载项后暂停。以上限制不能表述为本轮已经修复。

沙盒测试完成后已关闭。一次正常关闭等待 20 秒未退出，核对唯一沙盒为本任务 `accept-gpu.wsb` 后结束了该任务的客户端与启动进程，未操作其他虚拟机服务。
