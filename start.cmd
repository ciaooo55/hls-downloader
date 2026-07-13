@echo off
chcp 65001 >nul 2>&1
setlocal

set "ROOT=%~dp0"
cd /d "%ROOT%"

set "PYTHON=python"
where python >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] Python not found. Install Python 3.10+.
    pause
    exit /b 1
)

set "PATH=%ROOT%bin;%PATH%"

echo ========================================
echo   HLS Downloader
echo ========================================
echo.
if not exist "%ROOT%bin\ffmpeg.exe" (
    echo [ERROR] Missing bin\ffmpeg.exe. Put ffmpeg.exe in the project bin folder.
    pause
    exit /b 1
)

if not exist "%ROOT%bin\ffprobe.exe" (
    echo [ERROR] Missing bin\ffprobe.exe. Put ffprobe.exe in the project bin folder.
    pause
    exit /b 1
)

echo Installing dependencies...
%PYTHON% -m pip install -r requirements.txt
if %ERRORLEVEL% NEQ 0 (
    echo [ERROR] Dependency installation failed. Check the pip output above.
    pause
    exit /b 1
)

echo.
echo Backend: http://127.0.0.1:8765
echo UI:      http://127.0.0.1:8765/ui
echo.
echo Press Ctrl+C to stop.
echo ========================================
echo.

start "" cmd /c "timeout /t 2 >nul && start http://127.0.0.1:8765/ui"

cd /d "%ROOT%backend"
%PYTHON% -m uvicorn app.main:app --host 127.0.0.1 --port 8765

pause
