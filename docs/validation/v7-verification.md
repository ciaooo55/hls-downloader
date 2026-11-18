# 当前验证状态

当前正式版是原生 Windows x64 版 7.0.7，发布提交为 `4f7c3d45cb271d8e7181a78e107e6442f6f0f9a9`。[发布说明](../releases/v7.0.7.md)列出了安装方式和功能范围。[验收索引](current-evidence.json)提供同一提交的构建、浏览器、安装和发布记录。

EXE 在独立 Windows 环境通过默认安装、中文自定义路径安装和开始菜单快捷方式启动验收。真实投屏设备、付费云字幕质量和物理多显示器切换尚未完成实机验证。Firefox 正式版永久安装需要 Mozilla 签名。

当前正式版有两个已知 DASH 选轨问题。清单仅声明 `contentType="audio"` 时可能把音轨当作视频。多 Period 点播在有多种编码可选时，可能选中无法与上一段拼接的轨道。

[IDM 对应表](idm-source-audit.md)和[外部源码整合记录](../architecture/source-integration-20261117.md)记录实现依据。旧版验收记录可以通过 Git 标签查看。
