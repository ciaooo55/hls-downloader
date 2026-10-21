# HLS Downloader 浏览器插件

版本：7.0.3。适用于 Firefox，以及 Chrome、Edge 等 Chromium 浏览器。插件通过 Native Messaging 连接 Windows 上的 HLS Downloader 桌面程序。

插件提供网页媒体识别、HLS/DASH 清晰度和音轨选择、普通下载接管、独立桌面确认、取消与失败回退、右键下载、选中链接批量发送，以及在线字幕的采集和控制。站点 Cookie 需要用户单独授权；在线字幕默认关闭，需要用户自行配置 QwenCloud 或百炼服务，见 [字幕配置](../docs/guides/qwencloud-subtitles.md)。

工具栏弹窗使用紧凑资源列表，高度随内容变化。每项直接提供下载和清晰度选择，复制链接、投屏与 TVBox 位于“更多”。字幕服务、API Key、全局开关、语言、同步模式、原文显示和默认偏移统一在主程序“设置 → 字幕翻译”中配置；插件只提供播放器选择、开始/停止和网页开关，并显示当前语言与模式。网页字幕的上一句、下一句和本页时间微调位于播放器上的“选项”，不修改主程序默认值。

Firefox 对已确定为下载的 HTTP(S) GET 导航，在浏览器创建下载记录前交给主程序；主程序无法接收时保留浏览器原请求。Chrome/Edge 的 Manifest V3 使用下载项创建后暂停、交接成功后取消的路径，不能保证浏览器完全没有下载事件。普通网页导航和内嵌播放器请求不会因此被取消。

下载项接管也覆盖图片、JSON 和无扩展名文件。POST 下载和网页生成的 Blob/data 导出先由浏览器保存，再自动交给桌面确认并导入任务；任务完成后才删除浏览器副本，拒绝或失败保留原文件。POST 下载不会因自动接管而重复提交。

## 开发与审核

[审核构建说明](AMO-BUILD.md) 包含工具版本、锁定依赖构建命令、权限说明和服务依赖。[插件隐私说明](PRIVACY.md) 描述数据传输和用户控制。

```sh
pnpm install --frozen-lockfile
pnpm test
pnpm run package:store
pnpm exec web-ext lint --source-dir .output/firefox-mv3
```

`package:store` 生成 Chromium 插件 ZIP、Firefox 未签名 ZIP 和 Firefox 审核源码 ZIP。打包不调用签名服务、不上传文件。Firefox 的 Gecko ID 和 Chromium 公钥保持既有商店身份。

## 本地加载

先安装并启动 HLS Downloader 桌面程序。Firefox 在 `about:debugging#/runtime/this-firefox` 使用“临时载入附加组件”，选择 `.output/firefox-mv3/manifest.json`。Chrome 或 Edge 在扩展管理页启用开发者模式，加载 `.output/chrome-mv3` 目录。

交给签名服务的是 Firefox 插件 ZIP；审核需要源码时，同时提交源码 ZIP。签名后的 XPI 由用户安装。临时加载用于功能验证。
