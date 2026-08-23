# v7 Compose UI 测试 API

该接口只用于本机开发和视觉验收。默认不启动；启用后仅监听 `127.0.0.1`，每个请求都必须携带测试令牌。

## 启动

```powershell
$env:HLS_UI_TEST_API = '1'
$env:HLS_UI_TEST_TOKEN = 'replace-with-a-local-token-at-least-16-chars'
$env:HLS_UI_TEST_PORT = '19739'
$env:HLS_UI_AUDIT_WIDTH = '1024'
$env:HLS_UI_AUDIT_HEIGHT = '600'
.\desktop_ui\gradlew.bat -p desktop_ui run --console=plain
```

也可以将 `HLS_UI_TEST_PORT` 设为 `0`，并通过 `HLS_UI_TEST_PORT_FILE` 获取系统分配的实际端口。

## 接口

- `GET /health`：版本和存活状态。
- `GET /window`：窗口位置、尺寸、活动、显示状态和应用图标尺寸。
- `GET /state`：当前任务选择数量和稳定排序后的任务 ID，用于验证 Ctrl、Shift 与拖选。
- `GET /screenshot`：当前真实窗口的 PNG 截图。
- `GET /screen`：当前主屏幕的 PNG 截图，用于核对任务栏、托盘和系统弹窗。
- `POST /action`：执行窗口相对坐标操作。

所有请求必须包含：

```text
X-HLS-Test-Token: <HLS_UI_TEST_TOKEN>
```

动作示例：

```json
{"type":"activate"}
{"type":"click","x":954,"y":60}
{"type":"right_click","x":520,"y":180}
{"type":"drag","x":520,"y":420,"to_x":520,"to_y":180}
{"type":"click","x":520,"y":180,"modifiers":["CTRL"]}
{"type":"select_task","index":8,"modifiers":["SHIFT"]}
{"type":"key","key":"A","modifiers":["CTRL"]}
{"type":"type","text":"https://example.com/video/master.m3u8"}
```

坐标必须位于当前窗口内；请求体上限为 64 KiB，输入文本上限为 8192 字符。ASCII 文本使用真实按键事件，其他文本才使用剪贴板回退。不提供文件、命令行、Core 或数据库操作。

## 自动验证

```powershell
C:\Users\lee\.conda\envs\test\python.exe .\scripts\smoke_v7_ui_api.py `
  --port 19739 `
  --token 'replace-with-a-local-token-at-least-16-chars' `
  --output-dir .\artifacts\v7-productization\ui-api `
  --screen-output .\artifacts\v7-productization\ui-api\screen.png `
  --action '{"type":"click","x":954,"y":60}'
```

脚本会验证未授权请求返回 401、窗口尺寸、PNG 格式、图片非空和每次操作后的截图，并输出 `report.json`。
