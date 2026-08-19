import importlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

from backend.app.version import APP_VERSION


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
        assert len(reloaded.settings.token) >= 32
        assert reloaded.settings.token != "55555"
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


def test_firefox_release_uses_one_stable_id():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    wxt_config = (root / "extension" / "wxt.config.ts").read_text(encoding="utf-8")
    native_host = (root / "extension" / "native-host" / "firefox.json").read_text(encoding="utf-8")

    assert '$FirefoxId = "hls-downloader-store@ciaooo55.com"' in build_script
    assert "const firefoxId = 'hls-downloader-store@ciaooo55.com'" in wxt_config
    assert "Firefox build used the wrong extension ID" in build_script
    assert '"hls-downloader-store@ciaooo55.com"' in native_host
    assert 'browser@hls-downloader.ciaooo55.com' not in native_host


def test_app_icon_is_used_by_executable_tray_ui_and_installer():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    tauri_config = (root / "frontend" / "src-tauri" / "tauri.conf.json").read_text(
        encoding="utf-8"
    )
    tauri_main = (root / "frontend" / "src-tauri" / "src" / "main.rs").read_text(
        encoding="utf-8"
    )

    assert (root / "assets" / "app-icon.ico").stat().st_size > 10_000
    assert (root / "assets" / "app-icon.png").stat().st_size > 10_000
    assert "--icon $IconFile" in build_script
    assert "--version-file $PyInstallerVersionFile" in build_script
    assert "--name HLSDownloaderNativeHost" in build_script
    assert build_script.count("--icon $IconFile") >= 2
    assert 'Copy-Item -Path (Join-Path $AssetsDir "app-icon.png")' in build_script
    assert 'Icon "${ICON_FILE}"' in nsis_script
    assert 'UninstallIcon "${ICON_FILE}"' in nsis_script
    assert '!define MUI_ICON "${ICON_FILE}"' in nsis_script
    assert '!define MUI_UNICON "${ICON_FILE}"' in nsis_script
    assert 'VIProductVersion "${APP_FILE_VERSION}"' in nsis_script
    assert '"FileVersion" "${APP_FILE_VERSION}"' in nsis_script
    assert '"/DAPP_FILE_VERSION=$FileVersion"' in build_script
    assert 'File /oname=app-icon-${APP_VERSION}.ico' in nsis_script
    assert '$INSTDIR\\assets\\app-icon-${APP_VERSION}.ico' in nsis_script
    assert '"../../assets/app-icon.png"' in tauri_config
    assert '"../../assets/app-icon.ico"' in tauri_config
    assert "app.default_window_icon()" in tauri_main


def test_desktop_ui_bypasses_webview_cache_and_displays_version():
    root = Path(__file__).resolve().parent.parent
    main = (root / "backend" / "app" / "main.py").read_text(encoding="utf-8")
    app = (root / "frontend" / "src" / "App.tsx").read_text(encoding="utf-8")
    toolbar = (root / "frontend" / "src" / "components" / "DesktopToolbar.tsx").read_text(encoding="utf-8")
    tauri_config = (root / "frontend" / "src-tauri" / "tauri.conf.json").read_text(
        encoding="utf-8"
    )

    assert '"Cache-Control": "no-store, no-cache, must-revalidate, max-age=0"' in main
    assert '"frontendDist": "../dist"' in tauri_config
    assert "setAppVersion(healthData.version" in app
    assert "当前 v${props.version}" in toolbar
    assert 'className="tool-button update-button"' in toolbar


def test_windows_build_emits_setup_and_portable_assets():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert "$ReleaseNamePrefix-Windows-x64-Setup.exe" in build_script
    assert "$ReleaseNamePrefix-Windows-x64-Portable.zip" in build_script
    assert 'Join-Path $PortableStage "portable"' in build_script
    assert 'Join-Path $PortableStage "native-host\\versions"' in build_script
    assert 'HLSDownloaderNativeHost-$Version.exe' in build_script
    assert 'Join-Path $PortableStage "native-host\\manifests"' in build_script
    assert '"chrome-$Version.json"' in build_script
    assert 'Join-Path $Root "scripts\\upgrade-portable.ps1"' in build_script
    assert "upgrade-portable.ps1 -TargetDir" in build_script
    assert 'Join-Path $StageDir "core.log"' in build_script
    assert 'Join-Path $StageDir "core-error.log"' in build_script
    assert "Compress-Archive" in build_script
    assert 'Join-Path $env:LOCALAPPDATA "HLSDownloaderBuildTools"' in build_script
    assert '$NsisToolsDir = Join-Path $NsisRuntimeRoot' in build_script


def test_installer_and_portable_upgrade_stop_partial_old_installs():
    root = Path(__file__).resolve().parent.parent
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    portable_upgrade = (root / "scripts" / "upgrade-portable.ps1").read_text(encoding="utf-8")

    assert f'!define APP_VERSION "{APP_VERSION}"' in nsis_script
    close_macro = nsis_script[nsis_script.index("!macro CloseRunningApp") : nsis_script.index("!macroend", nsis_script.index("!macro CloseRunningApp"))]
    assert 'IfFileExists "$INSTDIR\\HLSDownloader.exe"' not in close_macro
    assert 'shutdown-running.ps1" -InstallDir "$INSTDIR"' in close_macro
    assert "shutdown-running.ps1" in portable_upgrade
    assert "register-native-host.ps1" in portable_upgrade
    assert "RegistryPrefix" in portable_upgrade
    assert '"config.json"' in portable_upgrade
    assert '"data.db"' in portable_upgrade


def test_installer_unregistration_survives_a_missing_legacy_helper():
    root = Path(__file__).resolve().parents[1]
    nsis = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    disconnect = nsis.split("!macro DisconnectLegacyNativeHost", 1)[1].split("!macroend", 1)[0]

    assert 'DeleteRegKey HKCU "Software\\Google\\Chrome\\NativeMessagingHosts' in disconnect
    assert 'DeleteRegKey HKCU "Software\\Mozilla\\NativeMessagingHosts' in disconnect
    assert "Var RemoveDownloads" in nsis
    assert 'StrCpy $RemoveDownloads "delete"' in nsis
    assert '${If} $RemoveDownloads == "delete"' in nsis
    assert "Function .onInstFailed" in nsis
    assert "!define MUI_CUSTOMFUNCTION_ABORT RestoreUpgradeAfterAbort" in nsis
    assert "Function .onUserAbort" not in nsis
    assert "Call RestoreUpgradeAfterAbort" in nsis
    self_delete = nsis.split("Function ScheduleSelfDelete", 1)[1].split("FunctionEnd", 1)[0]
    assert "HLS_DOWNLOADER_DELETE_SELF_PATH" in self_delete
    assert "GetEnvironmentVariable" in self_delete
    assert "-LiteralPath '$EXEPATH'" not in self_delete
    install_section = nsis.split('Section "Install"', 1)[1].split("SectionEnd", 1)[0]
    assert install_section.index("InitPluginsDir") < install_section.index('File /oname=shutdown-running.ps1')
    close_macro = nsis.split("!macro CloseRunningApp", 1)[1].split("!macroend", 1)[0]
    assert "SetErrorLevel 2" in close_macro
    assert "Abort" in close_macro


def test_portable_upgrade_smoke_is_isolated_from_the_real_browser_registry():
    root = Path(__file__).resolve().parent.parent
    smoke = (root / "scripts" / "smoke-portable-upgrade.ps1").read_text(encoding="utf-8")

    assert "HLSDownloaderPortableUpgradeSmoke" in smoke
    assert "Refusing to use an upgrade smoke path outside the project" in smoke
    assert "PreservedConfig" in smoke
    assert "OfficialAppProcessCountAfter" in smoke
    assert "packageVersion" in smoke


def test_installer_upgrade_smoke_isolated_and_runs_from_release_build():
    root = Path(__file__).resolve().parent.parent
    smoke = (root / "scripts" / "smoke-installer-upgrade.ps1").read_text(encoding="utf-8")
    nsis = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    build = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert "/BUILD-SMOKE=1" in smoke
    assert "HLSDownloaderInstallerSmoke" in smoke
    assert "Assert-OfficialState" in smoke
    assert "CoverInstallClosedRunningApp" in smoke
    assert "CoverInstallReplacedExecutables" in smoke
    assert "RunningCoverInstallReplacedStaleCore" in smoke
    assert "VersionedShellIcon" in smoke
    assert "ExactNativeHostRegistered" in smoke
    assert "ExecutableVersionsMatchPackage" in smoke
    assert "Mutate-ExecutableDosStub" in smoke
    assert "PyInstaller locates its package cookie at EOF" in smoke
    assert "WaitForExit(90000)" in smoke
    assert "UninstallClosedRunningApp" in smoke
    assert 'Uninstaller left application content behind' in smoke
    assert '-not $leftover.Count' in smoke
    assert "Uninstall.exe is deleted before RMDir /r _internal" in smoke
    assert 'StrCpy $NativeRegistryArgs \'-RegistryPrefix "HKCU:\\Software\\HLSDownloaderInstallerSmoke"\'' in nsis
    assert '${If} $BuildSmoke != "1"' in nsis
    assert 'Invoke-Step "Smoke test installer cover upgrade and uninstall"' in build


def test_installer_keeps_transactional_program_backup_until_success():
    root = Path(__file__).resolve().parent.parent
    nsis = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert 'StrCpy $UpgradeBackupDir "$INSTDIR\\.hls-upgrade-backup"' in nsis
    assert '!insertmacro BackupUpgradeDirectory "_internal"' in nsis
    assert '!insertmacro RestoreUpgradeDirectory "_internal"' in nsis
    assert "Function RestoreApplicationAfterAbort" in nsis
    assert "Call RestoreUpgradeAfterAbort" in nsis
    completed = nsis.index('StrCpy $InstallCompleted "1"')
    assert completed < nsis.index('RMDir /r "$UpgradeBackupDir"', completed)


def _portable_upgrade_fixture(tmp_path: Path, *, registration_fails: bool = False):
    root = Path(__file__).resolve().parent.parent
    source = tmp_path / "new-portable"
    target = tmp_path / "old-portable"
    for folder in (source / "scripts", target / "scripts"):
        folder.mkdir(parents=True)
    shutil.copy2(root / "scripts" / "upgrade-portable.ps1", source / "scripts" / "upgrade-portable.ps1")

    for folder, version in ((source, "new"), (target, "old")):
        (folder / "portable").write_text("", encoding="utf-8")
        (folder / "HLSDownloader.exe").write_text(version, encoding="utf-8")
        (folder / "HLSDownloaderCore.exe").write_text(version, encoding="utf-8")
    (source / "HLSNativeShell.exe").write_text("new", encoding="utf-8")
    (source / "new-program-file.txt").write_text("new", encoding="utf-8")
    (target / "old-program-file.txt").write_text("old", encoding="utf-8")
    (source / "config.json").write_text('{"version":"new"}', encoding="utf-8")
    (target / "config.json").write_text('{"version":"old"}', encoding="utf-8")
    (target / "data.db").write_text("old task database", encoding="utf-8")
    (target / "downloads").mkdir()
    (target / "downloads" / "keep.bin").write_bytes(b"download payload")
    helper = (
        "param([int]$TimeoutSeconds=1,[string]$InstallDir='',[switch]$IncludeNativeHost)\n"
        "$global:LASTEXITCODE=0\n"
    )
    (source / "scripts" / "shutdown-running.ps1").write_text(helper, encoding="utf-8")
    old_register = (
        "param([switch]$Unregister,[string]$RegistryPrefix='')\n"
        "$global:LASTEXITCODE=0\n"
    )
    new_register = old_register + ("throw 'simulated registration failure'\n" if registration_fails else "")
    (source / "scripts" / "register-native-host.ps1").write_text(new_register, encoding="utf-8")
    (target / "scripts" / "register-native-host.ps1").write_text(old_register, encoding="utf-8")
    return source, target


def test_portable_upgrade_swaps_complete_tree_and_preserves_runtime_state(tmp_path):
    source, target = _portable_upgrade_fixture(tmp_path)
    original_download_id = (target / "downloads" / "keep.bin").stat().st_ino

    subprocess.run(
        [
            "pwsh", "-NoProfile", "-NonInteractive", "-File",
            str(source / "scripts" / "upgrade-portable.ps1"),
            "-TargetDir", str(target),
            "-RegistryPrefix", r"HKCU:\Software\HLSDownloaderUpgradeUnitTest",
        ],
        check=True,
        capture_output=True,
        text=True,
    )

    assert (target / "HLSDownloader.exe").read_text(encoding="utf-8") == "new"
    assert (target / "new-program-file.txt").read_text(encoding="utf-8") == "new"
    assert not (target / "old-program-file.txt").exists()
    assert (target / "config.json").read_text(encoding="utf-8") == '{"version":"old"}'
    assert (target / "data.db").read_text(encoding="utf-8") == "old task database"
    assert (target / "downloads" / "keep.bin").read_bytes() == b"download payload"
    assert (target / "downloads" / "keep.bin").stat().st_ino == original_download_id
    assert not (tmp_path / ".old-portable.hls-upgrade-new").exists()
    assert not (tmp_path / ".old-portable.hls-upgrade-backup").exists()


def test_portable_upgrade_restores_old_tree_when_registration_fails(tmp_path):
    source, target = _portable_upgrade_fixture(tmp_path, registration_fails=True)

    result = subprocess.run(
        [
            "pwsh", "-NoProfile", "-NonInteractive", "-File",
            str(source / "scripts" / "upgrade-portable.ps1"),
            "-TargetDir", str(target),
            "-RegistryPrefix", r"HKCU:\Software\HLSDownloaderUpgradeUnitTest",
        ],
        check=False,
        capture_output=True,
        text=True,
    )

    assert result.returncode != 0
    assert (target / "HLSDownloader.exe").read_text(encoding="utf-8") == "old"
    assert (target / "old-program-file.txt").read_text(encoding="utf-8") == "old"
    assert (target / "downloads" / "keep.bin").read_bytes() == b"download payload"
    assert not (target / "new-program-file.txt").exists()
    assert not (tmp_path / ".old-portable.hls-upgrade-new").exists()
    assert not (tmp_path / ".old-portable.hls-upgrade-backup").exists()


def test_windows_build_emits_extension_assets_only_when_requested():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert "[switch]$IncludeExtensionAssets" in build_script
    assert "$ReleaseNamePrefix-Firefox-Unsigned.zip" in build_script
    assert "$ReleaseNamePrefix-Firefox-Source.zip" in build_script
    assert "$ReleaseNamePrefix-Chrome-Edge-Extension.zip" in build_script
    assert "$ChromiumExtensionStage" in build_script
    assert "$FirefoxId" in build_script
    assert "HLS_FIREFOX_EXTENSION_ID" not in build_script
    assert "if ($IncludeExtensionAssets)" in build_script
    assert "Release directory must contain exactly $($expected.Count) files" in build_script
    assert "SHA256SUMS.txt" not in build_script


def test_tag_release_only_emits_extension_assets_when_extension_changed():
    root = Path(__file__).resolve().parent.parent
    workflow = (root / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")

    assert "fetch-depth: 0" in workflow
    assert "git describe --tags --abbrev=0" in workflow
    assert "git diff --name-only $previousTag $env:GITHUB_REF_NAME -- extension" in workflow
    assert "steps.extension-assets.outputs.include_extensions" in workflow


def test_windows_build_uses_pinned_verified_packaging_tools():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    installer = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert "$NsisVersion = \"3.12\"" in build_script
    assert "$NsisSha256 = \"56581f90db321581c5381193d796fffcf2d24b2f8fed2160a6c6a3baa67f2c4f\"" in build_script
    assert "$FFmpegArchiveBuild = \"BtbN autobuild 2026-08-01 13:21 (FFmpeg g946272b79a)\"" in build_script
    assert "releases/download/autobuild-2026-08-01-13-21/ffmpeg-N-125881-g946272b79a-win64-gpl.zip" in build_script
    assert "$FFmpegArchiveSha256 = \"a082da6d5ce0cbb9a8ad0112ab7f654d480c707b8caf9d332f4532d78b65257f\"" in build_script
    assert "generate_sbom.py" in build_script
    assert 'File "${STAGE_DIR}\\sbom.cdx.json"' in installer
    assert 'Copy-Item -LiteralPath (Join-Path $Root "TERMS.md")' in build_script
    assert 'Copy-Item -LiteralPath (Join-Path $Root "PRIVACY.md")' in build_script
    assert '[Text.Encoding]::Unicode' in build_script
    assert '!insertmacro MUI_PAGE_LICENSE "${STAGE_DIR}\\TERMS.txt"' in installer
    assert 'File "${STAGE_DIR}\\TERMS.md"' in installer
    assert 'File "${STAGE_DIR}\\PRIVACY.md"' in installer
    assert "Assert-FileSha256" in build_script
    assert "Get-VerifiedArchive" in build_script
    assert 'attempt $attempt/3' in build_script
    assert 'Copy-MediaTool "ffmpeg.exe"' in build_script
    assert 'Copy-MediaTool "ffprobe.exe"' in build_script
    assert "Copy-Item" in build_script
    assert '$StageDir, $ReleaseDir, $BinDir, $ToolsDir' in build_script
    assert "return $installedCandidates[0]" not in build_script
    assert "Sort-Object Length -Descending" in build_script
    assert "& $destination -version" in build_script
    assert "Bundled media tool validation failed" in build_script


def test_windows_package_uses_tauri_tray_and_clean_uninstall():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    requirements = (root / "backend" / "requirements.txt").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    installer_smoke = (root / "scripts" / "smoke-installer-upgrade.ps1").read_text(encoding="utf-8")

    assert "pystray" not in requirements
    assert "pywebview" not in requirements
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
    assert "resume_tasks=true" in shutdown_script
    assert "function Get-TargetProcesses" in shutdown_script
    assert "$targetRunningAtStart" in shutdown_script
    assert "$overallDeadline" in shutdown_script
    assert "AddSeconds([Math]::Max(3, $TimeoutSeconds))" in shutdown_script
    assert "taskkill.exe\" /PID $desktop.Id /T /F" in shutdown_script
    assert '[System.IO.FileShare]::None' in shutdown_script
    assert '-InstallDir "$INSTDIR" -TimeoutSeconds 12 ${IncludeNativeHost}' in nsis_script
    assert 'CloseRunningAppRetry${Suffix}' not in nsis_script
    assert 'DisconnectLegacyNativeHost' in nsis_script
    assert 'register-native-host.ps1" -Unregister' in nsis_script
    assert '无法安全关闭运行中的程序，安装已停止' in nsis_script
    assert "SetErrorLevel 2" in nsis_script
    assert "[int[]]$ExpectedProcessIds = @()" in installer_smoke
    assert "Get-ApplicationProcesses | ForEach-Object { [int]$_.Id }" in installer_smoke
    assert "@((Get-ApplicationProcesses).Id)" not in installer_smoke
    assert 'CloseRunningAppAbort${Suffix}' not in nsis_script
    assert 'StrCpy $R0 0' not in nsis_script
    assert 'IntOp $R0 $R0 + 1' not in nsis_script
    assert 'MB_RETRYCANCEL' not in nsis_script
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
    assert 'PreviousTorrentProgId' in nsis_script
    assert 'Software\\Classes\\.url\\shell\\HLSDownload' in nsis_script
    assert 'Software\\Classes\\InternetShortcut\\shell\\HLSDownload' in nsis_script
    assert 'Software\\Classes\\.magnet\\shell\\HLSDownload' in nsis_script
    assert 'Software\\Classes\\.m3u8\\shell\\HLSDownload' in nsis_script
    assert 'Software\\Classes\\.html\\shell\\HLSDownload' in nsis_script
    assert 'Software\\Classes\\.metalink\\shell\\HLSDownload' in nsis_script
    assert 'Software\\Classes\\.meta4\\shell\\HLSDownload' in nsis_script
    assert 'DeleteRegKey HKCU "Software\\Classes\\.metalink\\shell\\HLSDownload"' in nsis_script
    assert 'DeleteRegKey HKCU "Software\\Classes\\.m3u8\\shell\\HLSDownload"' in nsis_script
    assert 'Software\\Classes\\.m3u8" ""' not in nsis_script
    assert 'Software\\Classes\\.html" ""' not in nsis_script
    assert 'Software\\Classes\\.url" ""' not in nsis_script
    assert 'DeleteRegValue HKCU "Software\\Classes\\.torrent" ""' in nsis_script
    assert 'DeleteRegKey HKCU "Software\\Classes\\.torrent"' not in nsis_script
    assert "HLSNativeShell.exe$\\\" $\\\"%1$\\\"" in nsis_script
    assert 'CreateShortcut "$SMPROGRAMS\\${APP_NAME}\\${APP_NAME}.lnk" "$INSTDIR\\HLSNativeShell.exe"' in nsis_script
    assert 'MUI_FINISHPAGE_RUN "$INSTDIR\\HLSNativeShell.exe"' in nsis_script
    assert "$smokePortableMarker" in build_script
    assert 'Set-Content -LiteralPath $smokePortableMarker' in build_script
    assert 'Remove-Item -LiteralPath $smokePortableMarker' in build_script
    assert 'HLS_DOWNLOADER_BUILD_SMOKE' in build_script
    assert '[System.Net.Sockets.TcpListener]::new' in build_script
    assert '$smokeApiBase/api/health' in build_script
    assert 'compose\\binaries\\main\\app\\HLSDownloader' not in build_script
    assert '!insertmacro DisconnectLegacyNativeHost Install' in nsis_script
    assert '!insertmacro CloseRunningApp Install "-IncludeNativeHost"' in nsis_script
    assert '!insertmacro CloseRunningApp Uninstall "-IncludeNativeHost"' in nsis_script
    assert '!include "x64.nsh"' in nsis_script
    assert '"$WINDIR\\Sysnative\\WindowsPowerShell\\v1.0\\powershell.exe"' in nsis_script
    assert 'Var PowerShellExe' in nsis_script
    assert 'Get-CimInstance Win32_Process' in shutdown_script
    install_cleanup = nsis_script.index('RMDir /r "$INSTDIR\\_internal"')
    install_copy = nsis_script.index('SetOutPath "$INSTDIR\\_internal"')
    assert install_cleanup < install_copy
    assert '"/DELETESELF="' in nsis_script
    assert "Wait-Process -Id $0" in nsis_script
    assert "Remove-Item -LiteralPath ([Environment]::GetEnvironmentVariable" in nsis_script
    assert "Remove-Item -LiteralPath '$EXEPATH'" not in nsis_script
    assert "Call ScheduleSelfDelete" in nsis_script
    assert "NSIS first launches a temporary uninstaller" in installer_smoke
    assert "-not (Test-Path -LiteralPath $uninstaller)" in installer_smoke


def test_windows_package_uses_onedir_and_smoke_tests_graceful_shutdown():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert "--onedir" in build_script
    assert "--name HLSDownloaderNativeHost" in build_script
    assert "--onefile" in build_script
    assert 'dist\\HLSDownloaderCore\\*' in build_script
    assert r'scripts\shutdown-running.ps1' in build_script
    assert '-InstallDir $StageDir -TimeoutSeconds 12' in build_script
    assert 'Packaged Settings schema is missing field' in build_script
    assert '$env:PYTHONPATH = ""' in build_script
    assert 'Installer shutdown helper failed' in build_script
    assert r'Remove-Item -LiteralPath "HKCU:\Software\HLSDownloaderBuildSmoke"' in build_script
    assert "Single-instance check failed" in build_script
    assert "$secondProc.WaitForExit(12000)" in build_script
    assert '${STAGE_DIR}\\_internal' in nsis_script
    assert '${STAGE_DIR}\\app\\*' not in nsis_script
    assert '${STAGE_DIR}\\runtime\\*' not in nsis_script
    assert 'HLSDownloaderCore.exe' in nsis_script
    assert 'HLSNativeShell.exe' in nsis_script
    assert 'HLSNativeEngine.exe' in nsis_script
    conf = json.loads((root / "frontend" / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
    assert conf["app"]["windows"] == []
    assert 'cargo build --release --locked' in build_script
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


def test_native_host_registration_uses_a_versioned_executable_path():
    root = Path(__file__).resolve().parent.parent
    script = (root / "scripts" / "register-native-host.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert "Get-VersionedNativeHost" in script
    assert "Get-VersionedManifest" in script
    assert 'Join-Path $manifestDir "versions"' in script
    assert 'Join-Path $root "HLSDownloaderNativeHost.exe"' in script
    assert "$manifest.path = $hostExecutable" in script
    assert 'Join-Path $manifestsDir "chrome-$selectedHostVersion.json"' in script
    assert 'Join-Path $manifestsDir "firefox-$selectedHostVersion.json"' in script
    assert "HLSDownloaderNativeHost-*.exe" in script
    assert "HLSDownloaderNativeHost-${APP_VERSION}.exe" in nsis_script
    assert 'SetOutPath "$INSTDIR\\native-host\\versions"' in nsis_script
    assert 'SetOutPath "$INSTDIR\\native-host\\manifests"' in nsis_script
    assert 'File /oname=chrome-${APP_VERSION}.json' in nsis_script
    assert 'File "${STAGE_DIR}\\HLSDownloaderNativeHost.exe"' not in nsis_script
    assert 'register-native-host.ps1" -Unregister' in nsis_script
    assert '-HostExecutable "$INSTDIR\\native-host\\versions\\HLSDownloaderNativeHost-${APP_VERSION}.exe"' in nsis_script
    assert r'Microsoft\Edge\NativeMessagingHosts' in script
    assert r'BraveSoftware\Brave-Browser\NativeMessagingHosts' in script
    assert r'Chromium\NativeMessagingHosts' in script
    assert r'Vivaldi\NativeMessagingHosts' in script
    assert r'Opera Software\NativeMessagingHosts' in script
    assert "RegistryPrefix" in script
    assert "smoke_native_host.py" in build_script
    assert "Native Messaging protocol smoke test failed" in build_script
    assert "[System.IO.File]::ReadAllText($manifestPath, [System.Text.Encoding]::UTF8)" in build_script
    smoke_cleanup = build_script.index('RegistryPrefix "HKCU:\\Software\\HLSDownloaderBuildSmoke" | Out-Null')
    installer_build = build_script.index('Invoke-Step "Build NSIS installer"')
    assert build_script.index('(Join-Path $ExtensionDir "native-host\\chrome.json")', smoke_cleanup) < installer_build
    assert "正在切换 Chromium/Firefox 系浏览器连接到新版本" in nsis_script
    assert r'Software\Microsoft\Edge\NativeMessagingHosts' in nsis_script


def test_native_host_registration_honors_explicit_package_version(tmp_path):
    root = Path(__file__).resolve().parent.parent
    app = tmp_path / "app"
    scripts = app / "scripts"
    versions = app / "native-host" / "versions"
    manifests = app / "native-host" / "manifests"
    scripts.mkdir(parents=True)
    versions.mkdir(parents=True)
    manifests.mkdir(parents=True)
    shutil.copy2(root / "scripts" / "register-native-host.ps1", scripts)

    registry_root = rf"HKCU:\Software\HLSDownloaderNativeHostUnitTest\{tmp_path.name}"
    for shell in ("powershell.exe", "pwsh.exe"):
        selected_host = versions / "HLSDownloaderNativeHost-1.2.3.exe"
        newer_host = versions / "HLSDownloaderNativeHost-9.9.9.exe"
        selected_host.write_bytes(b"selected")
        newer_host.write_bytes(b"newer-leftover")
        for browser in ("chrome", "firefox"):
            for version in ("1.2.3", "9.9.9"):
                (manifests / f"{browser}-{version}.json").write_text(
                    json.dumps(
                        {
                            "name": "com.ciaooo55.hls_downloader",
                            "description": "unit test",
                            "path": "placeholder.exe",
                            "type": "stdio",
                            **(
                                {"allowed_origins": ["chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/"]}
                                if browser == "chrome"
                                else {"allowed_extensions": ["hls-downloader-store@ciaooo55.com"]}
                            ),
                        }
                    ),
                    encoding="utf-8",
                )
        shell_registry = registry_root + "\\" + shell.replace(".", "-")
        try:
            subprocess.run(
                [
                    shell,
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(scripts / "register-native-host.ps1"),
                    "-RegistryPrefix",
                    shell_registry,
                    "-HostExecutable",
                    str(selected_host),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            for browser in ("chrome", "firefox"):
                manifest = json.loads(
                    (manifests / f"{browser}-1.2.3.json").read_text(encoding="utf-8")
                )
                assert Path(manifest["path"]).resolve() == selected_host.resolve()
            assert not newer_host.exists()
            assert not (manifests / "chrome-9.9.9.json").exists()
            assert not (manifests / "firefox-9.9.9.json").exists()
        finally:
            subprocess.run(
                [
                    shell,
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    f"Remove-Item -LiteralPath '{shell_registry}' -Recurse -Force -ErrorAction SilentlyContinue",
                ],
                check=False,
                capture_output=True,
                text=True,
            )


def test_installer_does_not_require_a_browser_owned_native_host_to_be_writable():
    root = Path(__file__).resolve().parent.parent
    shutdown_script = (root / "scripts" / "shutdown-running.ps1").read_text(encoding="utf-8")
    register_script = (root / "scripts" / "register-native-host.ps1").read_text(encoding="utf-8")

    assert "Test-ApplicationFilesWritable" in shutdown_script
    assert "-OperationTimeoutSec 2" in shutdown_script
    assert "$script:processPathCache" in shutdown_script
    assert "Native Messaging host is deliberately excluded" in shutdown_script
    assert "HLSDownloaderNativeHost.exe\"))" not in shutdown_script
    assert 'Get-TargetProcesses @("HLSDownloaderNativeHost*")' in shutdown_script
    assert "neither the current registration target nor a" in register_script
    assert 'Get-Process -Name "HLSDownloaderNativeHost*"' in register_script


def test_firefox_release_includes_reviewable_source_archive():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    reviewer_notes = (root / "extension" / "AMO-BUILD.md").read_text(encoding="utf-8")

    assert "$ReleaseNamePrefix-Firefox-Source.zip" in build_script
    assert "BUILD-INFO.txt" in build_script
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


def test_firefox_id_matches_native_host_and_has_no_second_variant():
    root = Path(__file__).resolve().parent.parent
    config = (root / "extension" / "wxt.config.ts").read_text(encoding="utf-8")
    native_host = (root / "extension" / "native-host" / "firefox.json").read_text(encoding="utf-8")

    extension_id = "hls-downloader-store@ciaooo55.com"
    assert extension_id in config
    assert extension_id in native_host
    assert "browser@hls-downloader.ciaooo55.com" not in config
    assert "browser@hls-downloader.ciaooo55.com" not in native_host


def test_tauri_enables_csp_and_splits_handoff_permissions():
    root = Path(__file__).resolve().parent.parent / "frontend" / "src-tauri"
    config = json.loads((root / "tauri.conf.json").read_text(encoding="utf-8"))
    main_capability = json.loads(
        (root / "capabilities" / "default.json").read_text(encoding="utf-8")
    )
    handoff_capability = json.loads(
        (root / "capabilities" / "handoff.json").read_text(encoding="utf-8")
    )

    csp = config["app"]["security"]["csp"]
    assert "default-src 'self'" in csp
    assert "object-src 'none'" in csp
    assert "frame-ancestors 'none'" in csp
    assert main_capability["windows"] == ["main"]
    assert handoff_capability["windows"] == ["handoff-*"]
    assert "fs:default" not in main_capability["permissions"]
    assert "opener:default" not in main_capability["permissions"]
    assert "dialog:default" not in handoff_capability["permissions"]
    assert "process:default" not in handoff_capability["permissions"]


def test_frontend_sse_does_not_put_control_token_in_url():
    root = Path(__file__).resolve().parent.parent
    api_source = (root / "frontend" / "src" / "api.ts").read_text(encoding="utf-8")

    assert "new EventSource" not in api_source
    assert "`${BASE}/events?token=" not in api_source
    assert "fetch(`${apiBase()}/events`" in api_source
    assert "'X-Token': getToken()" in api_source
    assert "file?token=${encodeURIComponent(getToken())}" not in api_source
    assert "playback/index.m3u8?session=${encodeURIComponent(session)}&token=${encodeURIComponent(getToken())}" not in api_source
    assert "playback/media?session=${encodeURIComponent(session)}&token=${encodeURIComponent(getToken())}" not in api_source


def test_v6_package_pins_the_same_ffmpeg_as_the_5x_spec():
    root = Path(__file__).resolve().parent.parent
    v6_build = (root / "scripts" / "build_v6.ps1").read_text(encoding="utf-8")
    spec_build = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis = (root / "installer" / "hls-downloader-v6.nsi").read_text(encoding="utf-8")
    smoke = (root / "scripts" / "smoke_v6_package.ps1").read_text(encoding="utf-8")

    pin = 'a082da6d5ce0cbb9a8ad0112ab7f654d480c707b8caf9d332f4532d78b65257f'
    url = "releases/download/autobuild-2026-08-01-13-21/ffmpeg-N-125881-g946272b79a-win64-gpl.zip"
    assert pin in spec_build
    assert pin in v6_build
    assert url in spec_build
    assert url in v6_build
    assert 'Copy-MediaTool "ffmpeg.exe"' in v6_build
    assert 'Copy-MediaTool "ffprobe.exe"' in v6_build
    assert 'File "${STAGE_DIR}\\ffmpeg.exe"' in nsis
    assert 'File "${STAGE_DIR}\\ffprobe.exe"' in nsis
    assert 'File /nonfatal "${STAGE_DIR}\\ffmpeg.exe"' not in nsis
    assert "ffmpeg.exe" in smoke
    assert "ffprobe.exe" in smoke
