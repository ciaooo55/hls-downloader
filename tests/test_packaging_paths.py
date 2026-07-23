import importlib
import sys
from pathlib import Path


def test_project_root_uses_executable_directory_when_frozen(monkeypatch, tmp_path):
    exe = tmp_path / "install" / "HLSDownloader.exe"
    exe.parent.mkdir()
    exe.write_bytes(b"")

    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(exe))
    monkeypatch.setenv("LOCALAPPDATA", str(tmp_path / "local"))
    monkeypatch.setenv("USERPROFILE", str(tmp_path / "user"))

    import backend.app.paths as paths
    import backend.app.config as config

    importlib.reload(paths)
    reloaded = importlib.reload(config)
    try:
        assert reloaded.PROJECT_ROOT == exe.parent
        assert reloaded.CONFIG_PATH == tmp_path / "local" / "HLS Downloader" / "config.json"
        assert reloaded.settings.download_dir == str(
            (tmp_path / "user" / "Downloads" / "HLS Downloader").resolve()
        )
        assert reloaded.settings.token == "55555"
        assert reloaded.settings.ffmpeg_path == str((exe.parent / "bin" / "ffmpeg.exe").resolve())
    finally:
        monkeypatch.delattr(sys, "frozen", raising=False)
        importlib.reload(paths)
        importlib.reload(config)


def test_ui_dist_uses_project_root(monkeypatch, tmp_path):
    exe = tmp_path / "install" / "HLSDownloader.exe"
    exe.parent.mkdir()
    exe.write_bytes(b"")

    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(exe))

    import backend.app.paths as paths
    import backend.app.config as config
    import backend.app.main as main

    importlib.reload(paths)
    importlib.reload(config)
    reloaded_main = importlib.reload(main)
    try:
        assert reloaded_main.UI_DIST == exe.parent / "frontend" / "dist"
    finally:
        monkeypatch.delattr(sys, "frozen", raising=False)
        importlib.reload(paths)
        importlib.reload(config)
        importlib.reload(main)


def test_installer_and_release_exclude_legacy_userscript():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert "userscript" not in build_script.lower()
    assert "userscript" not in nsis_script.lower()
    assert "m3u8-sniffer.user.js" not in build_script


def test_installer_bundles_loadable_edge_extension_and_removes_it_on_uninstall():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert 'Join-Path $StageDir "browser-extension\\chrome"' in build_script
    assert 'Join-Path $ExtensionDir ".output\\chrome-mv3\\*"' in build_script
    assert '${STAGE_DIR}\\browser-extension\\chrome\\*' in nsis_script
    assert 'RMDir /r "$INSTDIR\\browser-extension"' in nsis_script


def test_app_icon_is_used_by_executable_tray_ui_and_installer():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    desktop = (root / "backend" / "desktop.py").read_text(encoding="utf-8")

    assert (root / "assets" / "app-icon.ico").stat().st_size > 10_000
    assert (root / "assets" / "app-icon.png").stat().st_size > 10_000
    assert "--icon $IconFile" in build_script
    assert 'Copy-Item -Path (Join-Path $AssetsDir "app-icon.png")' in build_script
    assert 'Icon "${ICON_FILE}"' in nsis_script
    assert 'UninstallIcon "${ICON_FILE}"' in nsis_script
    assert '!define MUI_ICON "${ICON_FILE}"' in nsis_script
    assert '!define MUI_UNICON "${ICON_FILE}"' in nsis_script
    assert 'VIProductVersion "${APP_FILE_VERSION}"' in nsis_script
    assert '"FileVersion" "${APP_FILE_VERSION}"' in nsis_script
    assert '"/DAPP_FILE_VERSION=$FileVersion"' in build_script
    assert 'assets" / "app-icon.png"' in desktop


def test_desktop_ui_bypasses_webview_cache_and_displays_version():
    root = Path(__file__).resolve().parent.parent
    desktop = (root / "backend" / "desktop.py").read_text(encoding="utf-8")
    main = (root / "backend" / "app" / "main.py").read_text(encoding="utf-8")
    app = (root / "frontend" / "src" / "App.tsx").read_text(encoding="utf-8")
    toolbar = (root / "frontend" / "src" / "components" / "DesktopToolbar.tsx").read_text(encoding="utf-8")

    assert "/ui?version={APP_VERSION}" in desktop
    assert '"Cache-Control": "no-store, no-cache, must-revalidate, max-age=0"' in main
    assert "setAppVersion(healthData.version" in app
    assert "当前 v${props.version}" in toolbar
    assert 'className="tool-button update-button"' in toolbar


def test_windows_build_emits_setup_and_portable_assets():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert "HLSDownloader-Windows-x64-Setup.exe" in build_script
    assert "HLSDownloader-Windows-x64-Portable.zip" in build_script
    assert 'Join-Path $PortableStage "portable"' in build_script
    assert "Compress-Archive" in build_script


def test_windows_build_emits_extension_packages_and_current_checksums():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert 'Join-Path $ReleaseDir "HLSDownloader-Chrome.zip"' in build_script
    assert 'Join-Path $ReleaseDir "HLSDownloader-Firefox-Unsigned.zip"' in build_script
    assert 'Join-Path $ReleaseDir "SHA256SUMS.txt"' in build_script
    assert "Get-FileHash -LiteralPath" in build_script
    assert "Where-Object Name -ne \"SHA256SUMS.txt\"" in build_script


def test_windows_build_uses_tools_from_path_on_clean_runner():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert 'Get-Command "makensis.exe"' in build_script
    assert 'Get-Command "choco.exe"' in build_script
    assert 'Get-Command "conda.exe"' in build_script
    assert 'attempt $attempt/3' in build_script
    assert 'Copy-MediaTool "ffmpeg.exe"' in build_script
    assert 'Copy-MediaTool "ffprobe.exe"' in build_script
    assert "Get-Command $Name" in build_script
    assert "Copy-Item" in build_script
    assert '$StageDir, $ReleaseDir, $BinDir, $ToolsDir' in build_script
    assert "return $installedCandidates[0]" not in build_script
    assert "$env:ChocolateyInstall" in build_script
    assert 'Join-Path $env:ChocolateyInstall "lib\\ffmpeg\\tools"' in build_script
    assert "Sort-Object Length -Descending" in build_script
    assert "& $destination -version" in build_script
    assert "Bundled media tool validation failed" in build_script


def test_windows_package_includes_tray_runtime_and_clean_uninstall():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    requirements = (root / "backend" / "requirements.txt").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert "pystray==" in requirements
    assert "curl_cffi==0.14.0" in requirements
    assert "pnpm run tauri:build" in build_script
    assert "src-tauri\\target\\release\\HLSDownloader.exe" in build_script
    assert "--name HLSDownloaderCore" in build_script
    assert "--collect-all pystray" not in build_script
    assert "--collect-all curl_cffi" in build_script
    assert 'HLSDownloader.exe$\\" --shutdown' not in nsis_script
    assert 'shutdown-running.ps1' in nsis_script
    shutdown_script = (root / "scripts" / "shutdown-running.ps1").read_text(encoding="utf-8")
    assert "api/app/shutdown" in shutdown_script
    assert "taskkill.exe\" /IM HLSDownloader.exe /T /F" in shutdown_script
    assert '[System.IO.FileShare]::None' in shutdown_script
    assert '-InstallDir "$INSTDIR" -TimeoutSeconds 20' in nsis_script
    assert 'CloseRunningAppRetry${Suffix}' in nsis_script
    assert 'MB_RETRYCANCEL' in nsis_script
    assert 'CloseRunningAppAbort${Suffix}' in nsis_script
    assert 'RMDir /r "$LOCALAPPDATA\\HLS Downloader"' in nsis_script
    assert nsis_script.count('RMDir /r "$LOCALAPPDATA\\HLS Downloader"') >= 3
    assert 'Sleep 1000' in nsis_script
    assert nsis_script.count('Delete "$INSTDIR\\HLSDownloader.exe"') >= 3
    assert nsis_script.count('RMDir "$INSTDIR"') >= 3
    assert "MB_YESNO" in nsis_script
    assert 'RMDir /r "$INSTDIR"' in nsis_script
    assert "QuietUninstallString" in nsis_script
    assert 'File /oname=config.default.json "${STAGE_DIR}\\config.json"' in nsis_script
    assert 'CopyFiles /SILENT "$INSTDIR\\config.default.json" "$INSTDIR\\config.json"' not in nsis_script
    assert "$smokePortableMarker" in build_script
    assert 'Set-Content -LiteralPath $smokePortableMarker' in build_script
    assert 'Remove-Item -LiteralPath $smokePortableMarker' in build_script
    assert 'compose\\binaries\\main\\app\\HLSDownloader' not in build_script
    assert '!insertmacro CloseRunningApp Install' in nsis_script
    assert '!insertmacro CloseRunningApp Uninstall' in nsis_script
    install_cleanup = nsis_script.index('RMDir /r "$INSTDIR\\_internal"')
    install_copy = nsis_script.index('SetOutPath "$INSTDIR\\_internal"')
    assert install_cleanup < install_copy
    assert '"/DELETESELF="' in nsis_script
    assert "Wait-Process -Id $0" in nsis_script
    assert "Remove-Item -LiteralPath '$EXEPATH'" in nsis_script
    assert "Call ScheduleSelfDelete" in nsis_script


def test_windows_package_uses_onedir_and_smoke_tests_graceful_shutdown():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert "--onedir" in build_script
    assert "--name HLSDownloaderNativeHost" in build_script
    assert "--onefile" in build_script
    assert 'dist\\HLSDownloaderCore\\*' in build_script
    assert 'api/app/shutdown' in build_script
    assert 'Packaged Settings schema is missing field' in build_script
    assert '$env:PYTHONPATH = ""' in build_script
    assert 'Graceful shutdown failed' in build_script
    assert "Single-instance check failed" in build_script
    assert "$secondProc.WaitForExit(12000)" in build_script
    assert '${STAGE_DIR}\\_internal' in nsis_script
    assert '${STAGE_DIR}\\app\\*' not in nsis_script
    assert '${STAGE_DIR}\\runtime\\*' not in nsis_script
    assert 'HLSDownloaderCore.exe' in nsis_script
    assert 'RMDir /r "$INSTDIR\\_internal"' in nsis_script
    assert 'RMDir /r "$INSTDIR\\app"' in nsis_script
    assert 'RMDir /r "$INSTDIR\\runtime"' in nsis_script


def test_source_only_gitignore_excludes_generated_binaries():
    root = Path(__file__).resolve().parent.parent
    ignore = (root / ".gitignore").read_text(encoding="utf-8")

    assert "bin/" in ignore
    assert "release/" in ignore
    assert "backend/dist/" in ignore
    assert "frontend/dist/" in ignore


def test_native_host_registration_writes_absolute_executable_path():
    root = Path(__file__).resolve().parent.parent
    script = (root / "scripts" / "register-native-host.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert 'Join-Path $root "HLSDownloaderNativeHost.exe"' in script
    assert "$manifest.path = $hostExecutable" in script
    assert r'Microsoft\Edge\NativeMessagingHosts' in script
    assert "RegistryPrefix" in script
    assert "smoke_native_host.py" in build_script
    assert "Native Messaging protocol smoke test failed" in build_script
    assert "[System.IO.File]::ReadAllText($manifestPath, [System.Text.Encoding]::UTF8)" in build_script
    smoke_cleanup = build_script.index('RegistryPrefix "HKCU:\\Software\\HLSDownloaderBuildSmoke" | Out-Null')
    installer_build = build_script.index('Invoke-Step "Build NSIS installer"')
    assert build_script.index('(Join-Path $ExtensionDir "native-host\\chrome.json")', smoke_cleanup) < installer_build
    assert "正在注册 Chrome/Edge/Firefox 浏览器连接" in nsis_script
    assert r'Software\Microsoft\Edge\NativeMessagingHosts' in nsis_script


def test_firefox_release_includes_reviewable_source_archive():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    reviewer_notes = (root / "extension" / "AMO-BUILD.md").read_text(encoding="utf-8")

    assert "HLSDownloader-Firefox-Source.zip" in build_script
    for source in ("entrypoints", "lib", "public", "package.json", "pnpm-lock.yaml", "wxt.config.ts", "AMO-BUILD.md"):
        assert source in build_script
    assert "pnpm install --frozen-lockfile" in reviewer_notes
    assert "pnpm run build:firefox" in reviewer_notes


def test_extension_source_does_not_assign_untrusted_html():
    root = Path(__file__).resolve().parent.parent
    sources = [
        root / "extension" / "entrypoints" / "content.ts",
        root / "extension" / "entrypoints" / "popup" / "main.ts",
    ]

    for source in sources:
        assert ".innerHTML" not in source.read_text(encoding="utf-8")


def test_firefox_store_id_matches_native_host_and_keeps_legacy_compatibility():
    root = Path(__file__).resolve().parent.parent
    config = (root / "extension" / "wxt.config.ts").read_text(encoding="utf-8")
    native_host = (root / "extension" / "native-host" / "firefox.json").read_text(encoding="utf-8")

    store_id = "hls-downloader-store@ciaooo55.com"
    legacy_id = "browser@hls-downloader.ciaooo55.com"
    assert store_id in config
    assert store_id in native_host
    assert legacy_id not in config
    assert legacy_id in native_host
