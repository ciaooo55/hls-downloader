# QwenCloud 实时字幕翻译

插件字幕设置中选择“QwenCloud 实时翻译”，输入 QwenCloud API Key 并保存。QwenCloud 使用官方实时翻译地址 `wss://maas.qwencloudapi.com/api-ws/v1/realtime?model=qwen3.5-livetranslate-flash-realtime`，不需要填写百炼业务空间 ID 或地域。

`https://maas.qwencloudapi.com/compatible-mode/v1` 是 HTTP 兼容接口地址；本项目的实时音频翻译会自动连接上面的 WebSocket 地址。接口协议见 [QwenCloud 实时翻译文档](https://docs.qwencloud.com/developer-guides/speech/realtime-translation)。

API Key 通过桌面端 Core 配置，并由桌面端使用 Windows DPAPI 加密保存；插件不把明文 Key 写入扩展存储。不要将 Key 粘贴到日志、源码或文档中。

本机的个人 Key 可另存为 `outputs/private-user-backup/qwencloud-api-key.dpapi`，该目录已被 Git 忽略。启动配套新版桌面端后，在项目目录执行 `./scripts/configure-qwencloud.ps1`，即可将这份加密 Key 导入 Core。若文件不存在，脚本会通过密码输入框读取 Key 并加密保存；`-SaveOnly` 只保存个人加密备份。DPAPI 文件仅能由原 Windows 用户在原电脑解密，在其他电脑请重新输入 Key。

此功能需要配套的新版 HLS 桌面端 Core。启动桌面端后打开浏览器插件，在支持的真实网页视频上开启字幕翻译并允许网页音频采集；开始翻译时，采集到的音频会发送给所选服务进行实时翻译。预翻译会提前读取当前音轨。

确认播放器已加载且有声音后，在插件中开启“全局开启”，选择原文语种和译文语种（例如英语 → 中文），选中网页播放器并点击“开始翻译”。可开启原文显示并调整字幕时间偏移；“停止翻译”会结束当前采集和云端会话。Chromium 实时采集若提示标签页权限，请通过浏览器工具栏打开插件后再开始；直接打开插件页面不等同于工具栏授权。

## 提前翻译

开启“提前翻译音轨”后，从当前播放时间开始准备，无需手动归零。插件复用当前播放器的资源匹配结果；MSE/blob 播放器只有在 SourceBuffer 证据唯一对应真实媒体清单时才允许预取，多个音轨无法确认时明确提示使用实时采集。

Core 使用随程序提供的 FFmpeg 将 HLS、DASH 或普通媒体的有限时间窗口解码为原始 16kHz 单声道 PCM。单次最多解码 12 秒，音频直接通过管道进入内存，不创建下载任务、临时文件或 `.hls-tasks`。分片、初始化段、解密和准确定位交由媒体解复用器处理；不按分片个数推算时间，也不通过倍速播放改变送入模型的语音。

插件按正常语速发送音频，优先在静音处提交窗口（至少 2 秒，连续声音最多 8 秒，媒体末尾可以更短），使用云端 Manual 模式。一轮仅有一个已提交但未完成的窗口；收到匹配的 `response.done` 且状态为 `completed`，才将该窗口标记为就绪，确认提交或收到原文都不代表译文已完成。云端窗口文本按该窗口时间显示，尚未提供逐字对齐。协议依据：[客户端事件](https://docs.qwencloud.com/api-reference/speech-translation/livetranslate-realtime/client-events)、[服务端事件](https://docs.qwencloud.com/api-reference/speech-translation/livetranslate-realtime/server-events)。

开始时暂停视频，通常准备约 8 秒已完成字幕再继续，倍速时最多等待 16 秒字幕；播放追上字幕时重新准备。原本暂停的视频保持暂停，准备期间“保持暂停”可取消自动续播。原始音频最多提前发送约 30 秒，已完成区间必须连续，中间空洞不能被后面的结果掩盖。会话内最多缓存 500 个完成窗口，回看这些区间复用结果；拖动到未翻译区间会取消旧音频、重置云端连接并重新准备。更换音源或音轨时停止，避免混入上一视频的字幕。

当前独立解码支持公开或 URL 签名鉴权的点播链接，以及 Referer、User-Agent、Origin。需要 Cookie、Authorization 或其他专用请求头的媒体须使用实时采集，不能把浏览器凭据作为全局 FFmpeg 头跨域转发。直播没有尚未产生的音频可供提前获取，受保护媒体也不进入预取。准备失败会显示原因，不会把实时采集冒充提前翻译。

网络、识别和翻译速度不足以跟上播放时，播放器会暂停补齐字幕；倍速播放更容易触发等待，不能保证永不缓冲。预取提前量不是逐字时间精度，也不能消除模型翻译错误。

## 2026-10-16 验证结果

本轮已完成插件类型检查、全部 380 项现有插件测试，以及修正后的 10 项字幕定向测试；Chromium、Firefox 生产构建和 Core release 构建成功。最终 Core 的 11 项字幕服务测试和 1 项凭据边界测试在 Windows 沙盒通过。

Windows 沙盒中，实际生产 Core/FFmpeg 对 M4A、MP4、HLS、DASH 分别在 1.25、5.5、12 秒处解码 2.5 秒窗口，共 12 个窗口的长度、音调和能量与参考音频匹配。验收覆盖了 HLS 分片前滚和 DASH 时间戳空隙；阻塞音源可经独立连接在 2 秒内取消，取消后无残留 FFmpeg 子进程，整个预取过程没有创建下载任务或 `.hls-tasks`。

Edge 的实际插件、Native Host 和模拟翻译服务通过了预取控制验收：等待已完成译文才续播、原本暂停保持暂停、播放追上字幕自动暂停、准备时保持暂停、播放中拖动后按原意图续播、缓存回看、变速、时间偏移、失败重试、切换语言和停止清理。模拟服务用于验证时序和控制，不验证云端翻译质量；播放器实时采集和标签页实时采集也分别通过沙盒回归。

真实 QwenCloud 验收使用 [Wikimedia 公有领域短音频](https://upload.wikimedia.org/wikipedia/commons/2/2e/En-us-Wikipedia.ogg)（约 0.906 秒）。沙盒从本轮保存的未修改副本解码，按正常语速发送并手动提交，实际取得原文 `Wikipedia.`、译文“维基百科”和完成区间；随后重建云端连接，并在存在未提交音频时停止，会话正常结束。未上传用户视频。这项结果验证了短音频的实际服务链路，不代表长句质量、长视频连续使用或所有网站已经验收。

可复验脚本为 `scripts/smoke_subtitle_prefetch_core.py` 和 `scripts/smoke_extension_subtitles.py`，应在验收虚拟机运行；前者强制检查 `HLS_VM_ACCEPTANCE=1`。本轮已生成包含这些改动的未签名 `7.0.3-r4` 安装包及浏览器插件，并完成沙盒安装卸载和浏览器补测；桌面交付文件已更新。未进行本机安装或 GitHub 发布。

## 此前实时采集验收

2026-10-12 的实际 Wikimedia Commons “Original file” 验收中，Edge 通过工具栏授权后使用标签页音频采集（`capture=tab`），Firefox 使用播放器元素采集（`capture=element`）；两者均取得 Core 英文和中文最终字幕，并有字幕实际可见时保存的截图。该云端验收使用约 10 秒的公开音频，并在媒体自然结束后实际发送生产 stop 以冲洗最终字幕。长视频 seek/reset 仍仅由本地 fixture 覆盖，不代表已完成长视频的真实云端验收。
