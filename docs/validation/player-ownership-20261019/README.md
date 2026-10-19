# 播放器归属和插件生命周期修复

实现提交：`b832d2a`。本轮只修改插件和验收脚本，复用原 Core、桌面 UI、播放器与运行库；未在宿主安装或启动产品，未调用字幕云端。

## 根因与修改

- 悬浮按钮采用播放器的 SourceBuffer 归属证据，展开面板却重新按整页候选排序；两个播放器可能展开同一个高码率资源。面板现在优先沿用播放器匹配结果，只有缺少匹配时才显示待人工核对的候选。
- 同一个播放器缓冲后再次触发 `play/playing` 会清空选择状态。现在只有切换当前播放器才重置该状态。
- 部分文档/窗口监听、MutationObserver、ResizeObserver、PerformanceObserver 和 runtime 消息监听没有随内容脚本失效释放。现在使用现有 WXT 上下文管理监听与动画帧，显式释放观察器；旧的异步资源读取结果也不能更新已失效界面。下载意图脚本也随上下文释放监听。
- DOM 观察只处理添加节点，没有处理删除播放器，ResizeObserver 还保留旧元素。现在删除播放器会刷新入口并取消观察，重新挂载时恢复观察。
- 面板拖动只处理 pointerup，未处理 pointercancel/lostpointercapture。现在三条结束路径统一释放拖动监听和捕获。

未增加依赖、后台服务或播放引擎。没有通过重放 POST 来冒充无损接管。

## 实际验证

- 插件类型检查和 50 文件 / 386 项现有测试通过；Chromium、Firefox 构建完成。Firefox 扩展校验错误、警告、提示均为零。
- Firefox 158.0 / Windows Sandbox 的 23 个媒体场景通过，包含直接媒体、iframe、Shadow DOM、SPA、HLS/DASH、低延迟 HLS、双播放器和 Worker MSE。见 `media-initial.json`。无归属的场景保留识别入口，不冒充一键匹配成功。
- 针对性复测实际验证播放器移除/重新挂载、临时扩展替换后刷新页面只有一个入口，以及两个 DASH 播放器分别展开自己的清单。见 `media-targeted-final.json`；这是浏览器自动化检查，没有称为原生桌面按钮点测，也没有证明已打开网页无需刷新即可热更新。
- 下载接管 39 条记录通过，包含用户提供的公开 GitHub EXE、文件哈希、确认前后零 DownloadItem 创建事件、取消、Core 失败、关闭接管、站点排除和 NativeHost 不可用后的浏览器回退。见 `takeover.json`；接受/拒绝通过 Core 协议完成。
- 初次断连验收失败保留在 `takeover-initial-failed.json`。旧夹具只停止 WebDriver 父进程链中的 Host，不能可靠证明所有本轮连接都已断开。修正为沙盒内精确 runtime 路径的全部 Host，并临时移开本轮 Host 可执行文件；点击前必须先确认 ping 失败，结束后还原。没有修改宿主 Host 或浏览器登记。
- 最终 MSI 正常安装、自选含空格路径、内嵌插件/Core 哈希、源码溯源和卸载通过。MSI 安装/卸载均退出 0，所查产品/NativeHost 登记和主程序文件移除。见 `package-result.json`。
- MSI 同为 7.0.3 的覆盖升级通过：旧登记移除、沿用原路径、内嵌新插件；使用独立测试数据目录创建的暂停任务在升级后续传为完整 32 MiB 文件，SHA-256 一致。见 `upgrade-result.json`、`before-upgrade.json`、`after-upgrade.json`。本项不代表所有旧版迁移或真实用户任务都已认证。
- MSI 为 203,436,032 字节，与上一份桌面 MSI 相同；安装载荷为 469,462,823 字节，比上一轮增加 400 字节。未重新测量 CPU，不能把生命周期修复描述为已测出的 CPU 降幅。

## 交付与边界

桌面只保留 `HLSDownloader/最新-b832d2a` 下的 MSI、Firefox 未签名插件、Chromium 插件和 Firefox 审核源码 ZIP。哈希见 `desktop-artifacts.json`，安装包内两份插件与外部 ZIP 相同，审核源码中的修改文件与仓库逐字节相同。扩展版本仍为 7.0.3，已加载的旧插件需要换用新包并刷新网页。

Firefox POST、Blob/Data 以及 Chromium 仍未达到所有下载都在浏览器创建下载项之前接管的标准；本轮没有把这些路径标为完成。用户提供的网站此前被验证页阻挡，本轮也未证明其播放器识别已解决。字幕云端质量、全部网站、实体投屏/TVBox与所有播放器操作没有在本轮重新验收。没有发布新的 GitHub Release。

AB 的公开实现同样区分 Firefox 响应阻断与 Chromium 后置取消，并跳过 Blob/Data、限制直接接管的请求方法，不能据其普通下载体验推断它覆盖所有类型。参考：[AB 拦截实现](https://github.com/amir1376/ab-download-manager-browser-integration/blob/master/src/linkgrabber/DownloadLinkInterceptor.ts)、[MV3 路径](https://github.com/amir1376/ab-download-manager-browser-integration/blob/master/src/linkgrabber/Manifest3DownloadLinkInterceptor.ts)。IDM 的闭源内部实现未得到核实，没有臆测其使用何种底层机制。

此前四处构建缓存的删除已被自动审批拒绝，只有 `blocked by policy`，未提供原因。本轮没有重试同一删除效果或换工具绕过，缓存仍在。`finish-cleanup.ps1` 更新为核对本轮四份交付哈希，仍只处理固定清单；未执行。
