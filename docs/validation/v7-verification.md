# 当前验证状态

当前正式版是原生 Windows x64 版 7.0.9，发布提交为 `d2fdc0688e02b5bad9cdeec919add2485de524d0`。[发布说明](../releases/v7.0.9.md)列出了本版修复和安装方式。[验收索引](current-evidence.json)记录了同一提交的构建、安装、浏览器、原生功能与发布检查。

EXE 在独立 Windows 环境通过默认目录安装、中文自定义路径安装和开始菜单快捷方式启动。MSI 验证了从 7.0.8 升级、失败回滚及两种情况下的断点恢复。正式发布后，Edge 和 Firefox 分别安装公开 EXE 并加载配套插件，验证了原先没有主程序进程时可从插件打开工作台。真实投屏设备、云字幕质量和物理多显示器切换尚未完成实机验证。Windows 安装包未签名，Firefox 正式版永久安装需要 Mozilla 签名。

[IDM 对应表](idm-source-audit.md)和[外部源码整合记录](../architecture/source-integration-20261117.md)记录实现依据。旧版验收记录可以通过 Git 标签查看。
