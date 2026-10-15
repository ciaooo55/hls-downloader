# QwenCloud 实时字幕翻译

插件字幕设置中选择“QwenCloud 实时翻译”，输入 QwenCloud API Key 并保存。QwenCloud 使用官方实时翻译地址 `wss://maas.qwencloudapi.com/api-ws/v1/realtime?model=qwen3.5-livetranslate-flash-realtime`，不需要填写百炼业务空间 ID 或地域。

`https://maas.qwencloudapi.com/compatible-mode/v1` 是 HTTP 兼容接口地址；本项目的实时音频翻译会自动连接上面的 WebSocket 地址。接口协议见 [QwenCloud 实时翻译文档](https://docs.qwencloud.com/developer-guides/speech/realtime-translation)。

API Key 通过桌面端 Core 配置，并由桌面端使用 Windows DPAPI 加密保存；插件不把明文 Key 写入扩展存储。不要将 Key 粘贴到日志、源码或文档中。

本机的个人 Key 可另存为 `outputs/private-user-backup/qwencloud-api-key.dpapi`，该目录已被 Git 忽略。启动配套新版桌面端后，在项目目录执行 `./scripts/configure-qwencloud.ps1`，即可将这份加密 Key 导入 Core。若文件不存在，脚本会通过密码输入框读取 Key 并加密保存；`-SaveOnly` 只保存个人加密备份。DPAPI 文件仅能由原 Windows 用户在原电脑解密，在其他电脑请重新输入 Key。

此功能需要配套的新版 HLS 桌面端 Core。启动桌面端后打开浏览器插件，在支持的真实网页视频上开启字幕翻译并允许网页音频采集；开始翻译时，采集到的音频会发送给所选服务进行实时翻译。预翻译会提前读取当前音轨。

确认播放器已加载且有声音后，先暂停并将播放位置归零，再在插件中开启“全局开启”，选择原文语种和译文语种（例如英语 → 中文），选中网页播放器并点击“开始翻译”，随后播放媒体。从头翻译时不要在开始翻译后再拖回开头，以免触发会话重置。可开启原文显示并调整字幕时间偏移；“停止翻译”会结束当前采集和云端会话。Chromium 若提示标签页采集权限，请通过浏览器工具栏打开插件后再开始；直接打开插件页面不等同于工具栏授权。

2026-10-12 的实际 Wikimedia Commons “Original file” 验收中，Edge 通过工具栏授权后使用标签页音频采集（`capture=tab`），Firefox 使用播放器元素采集（`capture=element`）；两者均取得 Core 英文和中文最终字幕，并有字幕实际可见时保存的截图。该云端验收使用约 10 秒的公开音频，并在媒体自然结束后实际发送生产 stop 以冲洗最终字幕。长视频 seek/reset 仍仅由本地 fixture 覆盖，不代表已完成长视频的真实云端验收。
