"""Exercise the shipped popup and installer registration in a disposable Windows runner."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

from selenium.webdriver.common.by import By
from selenium.webdriver.support.ui import WebDriverWait

from smoke_extension_takeover import EXTENSION_ID, _create_driver


def run(browser, root, install_dir=None):
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("This check requires a disposable GitHub-hosted Windows installation.")
    extension = root / "extension"
    if install_dir:
        os.environ.pop("HLS_V7_CORE_TCP", None)
        os.environ.pop("HLS_V7_CORE_BIND", None)
        os.environ.update(
            HLS_V7_DATA_DIR=str(root / "data"),
            HLS_V7_DOWNLOAD_DIR=str(root / "downloads"),
            HLS_V7_PIPE=rf"\\.\pipe\HLSInstalledBrowser-{uuid.uuid4().hex}",
            HLS_V7_PRESENTER_TRACE_FILE=str(root / "presenter.log"),
        )
    if browser == "edge":
        binary = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft/Edge/Application/msedge.exe"
    else:
        binary = Path(os.environ["ProgramFiles"]) / "Mozilla Firefox/firefox.exe"
    driver = _create_driver(browser, binary, extension, root / "profile", root, None)
    try:
        popup = f"chrome-extension://{EXTENSION_ID}/popup.html"
        if browser == "firefox":
            addon = json.loads((extension / "manifest.json").read_text(encoding="utf-8"))
            driver.set_context("chrome")
            uuids = json.loads(driver.execute_script(
                "return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
            driver.set_context("content")
            popup = "moz-extension://" + uuids[addon["browser_specific_settings"]["gecko"]["id"]] + "/popup.html"
        driver.get(popup)
        button = WebDriverWait(driver, 15).until(lambda d: d.find_element(By.CSS_SELECTOR, ".desktop-open"))

        def launch_state(_driver):
            state = button.get_attribute("data-state")
            return state if state in ("ready", "failed") else False

        for _ in range(2 if install_dir else 1):
            button.click()
            if install_dir:
                state = WebDriverWait(driver, 30, poll_frequency=0.2).until(launch_state)
                if state != "ready":
                    raise RuntimeError(f"{browser}: {driver.find_element(By.CSS_SELECTOR, '.launch-status').text}")
            else:
                WebDriverWait(driver, 15).until(lambda _: button.is_enabled())

        # A ready receipt is necessary for the candidate, but the installed
        # presenter must also have a responsive workbench window.
        process_check = (
            "$p=Get-Process HLSDownloaderPresenter -ErrorAction SilentlyContinue | "
            "Where-Object { $_.Path -and $_.MainWindowHandle -ne 0 -and $_.Responding -and "
            "$_.MainWindowTitle -like 'HLS Downloader [0-9]*' -and "
            "(!$env:HLS_TEST_INSTALLED_PRESENTER -or $_.Path -eq $env:HLS_TEST_INSTALLED_PRESENTER) }; "
            "if (@($p).Count -ne 1) {exit 1}; $p.MainWindowTitle"
        )
        check_env = dict(os.environ)
        if install_dir:
            check_env["HLS_TEST_INSTALLED_PRESENTER"] = str(
                install_dir / "app" / "resources" / "HLSDownloaderPresenter.exe"
            )
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            check = subprocess.run(
                ["powershell.exe", "-NoProfile", "-Command", process_check],
                capture_output=True, env=check_env, timeout=10
            )
            if check.returncode == 0:
                print(f"{browser}: installed extension opened a responsive workbench")
                return
            time.sleep(0.4)
        raise RuntimeError("Missing workbench after popup activation: " + driver.find_element(By.TAG_NAME, "body").text)
    finally:
        driver.quit()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("browser", choices=("edge", "firefox"))
    parser.add_argument("root", type=Path)
    parser.add_argument("--install-dir", type=Path)
    args = parser.parse_args()
    run(args.browser, args.root.resolve(strict=True),
        args.install_dir.resolve(strict=True) if args.install_dir else None)
