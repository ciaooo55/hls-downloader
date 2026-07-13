# HLS 下载器

本地 HLS/m3u8 视频下载工具，Web 界面管理。

## 快速启动

```
双击 start.cmd
```

自动安装依赖、启动后端、打开浏览器。

## 功能

- 粘贴 m3u8 链接一键下载
- 分段多线程下载（IDM 风格），支持断点续传
- 油猴脚本自动嗅探网页中的 m3u8 资源
- 实时进度：分片数、速度、ETA、线程状态、连接状态
- 合并和转封装阶段也有进度显示，不再卡在 100% 看不出状态
- 文件名冲突自动处理
- 批量下载、暂停/恢复/取消/重试
- 下载目录可浏览选择
- 完成后可直接打开文件或所在目录
- 自动清理临时文件
- 点播 HLS：支持多层主清单、AES-128、BYTERANGE、fMP4 init map、discontinuity

## 支持范围

当前阶段只支持点播 HLS。会明确拒绝直播清单、SAMPLE-AES/DRM 和独立音轨；外部字幕会跳过并记录提示。

## 依赖

- Python 3.10+
- ffmpeg / ffprobe 已放在项目 `bin/` 目录

安装依赖：
```
pip install -r requirements.txt
```

项目只内置运行必需的 ffmpeg 工具；Python 环境依赖写在文本清单里：

- 根目录 `requirements.txt`：入口依赖清单
- `backend/requirements.txt`：后端实际 Python 包列表
- `requirements-dev.txt`：运行 Python 测试所需依赖
- 前端依赖使用 `frontend/pnpm-lock.yaml` 固定版本

## 油猴脚本

安装版启动后会自动打开 `http://127.0.0.1:8765/help` 使用教程，CLI 窗口也会打印教程、管理器和脚本安装地址。

1. 安装 [Tampermonkey](https://www.tampermonkey.net/)
2. 在使用教程中点击“安装油猴脚本”，或手动导入 `userscript/m3u8-sniffer.user.js`
3. 打开含视频的网页，右上角自动弹出资源提示
4. 点"发送下载"，可在浮窗内看到实时进度
5. 完成后点"打开文件"或"打开目录"

教程页会显示脚本最近是否向本地下载器报到。受浏览器安全限制，它不能直接读取 Tampermonkey 的脚本安装列表；安装后打开任意 HTTPS 页面，通常几秒内就会显示“已检测到油猴脚本运行”。

## 配置

编辑 `config.json`：

| 字段 | 说明 | 默认值 |
|------|------|--------|
| token | 认证 token | 55555 |
| download_dir | 下载目录 | downloads |
| default_concurrency | 默认并发（分片数） | 4 |
| max_concurrent_tasks | 最大同时任务数 | 2 |
| ffmpeg_path | ffmpeg 路径，相对项目根目录解析 | bin\ffmpeg.exe |

## 项目结构

```
hls-downloader/
├── start.cmd              # 一键启动
├── requirements.txt       # Python 依赖入口
├── config.json            # 配置
├── bin/                   # ffmpeg + ffprobe
├── backend/
│   ├── app/
│   │   ├── main.py        # FastAPI 入口
│   │   ├── api.py         # API 路由
│   │   ├── config.py      # 配置管理
│   │   ├── database.py    # SQLite
│   │   ├── models.py      # 数据模型
│   │   ├── schemas.py     # Pydantic 模型
│   │   ├── utils.py       # 工具函数
│   │   └── downloader/
│   │       ├── hls.py     # 下载引擎
│   │       ├── merge.py   # 合并+转封装
│   │       ├── parser.py  # m3u8 解析
│   │       ├── progress.py# 速度/ETA
│   │       └── task_manager.py
│   └── requirements.txt
├── frontend/              # Web UI (React)
│   ├── dist/              # 构建产物
│   └── src/
├── userscript/
│   └── m3u8-sniffer.user.js
└── README.md
```

## 开发模式

```powershell
# 后端
.\run_backend.ps1

# 前端 (另一个终端)
.\run_frontend.ps1

# 打开 http://localhost:5173
```

验证：

```powershell
pip install -r requirements-dev.txt
python -m pytest -q
cd frontend
pnpm test
pnpm run build
```

## 生产模式

```powershell
# 构建前端
.\build_frontend.ps1

# 启动
.\start.cmd
# 打开 http://127.0.0.1:8765/ui
```

## 打包安装包

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build_installer.ps1
```

输出文件：

```text
release\HLSDownloaderSetup.exe
```

安装包默认安装到 `%LOCALAPPDATA%\Programs\HLS Downloader`，安装向导里可以修改目录。安装内容包含程序、ffmpeg/ffprobe、前端和油猴脚本。卸载会移除这些程序文件和快捷方式；`config.json` 与 `downloads` 会保留，避免误删下载记录。
