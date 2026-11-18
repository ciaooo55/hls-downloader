"""Exercise the shipped popup and installer registration in a disposable Windows runner."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from selenium.webdriver.common.by import By
from selenium.webdriver.support.ui import WebDriverWait

from smoke_extension_takeover import EXTENSION_ID, _create_driver


def run(browser, root):
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("This check requires a disposable GitHub-hosted Windows installation.")
    extension = root / "extension"
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
        button.click()
        WebDriverWait(driver, 15).until(lambda _: button.is_enabled())
        # The popup may receive a dispatch acknowledgement before the launcher creates a window.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            check = subprocess.run(["powershell.exe", "-NoProfile", "-Command",
                "$p=Get-Process HLSDownloaderPresenter -ErrorAction SilentlyContinue | "
                "Where-Object { $_.MainWindowHandle -ne 0 -and $_.Responding -and "
                "$_.MainWindowTitle -like '*HLS Downloader*' }; if (-not $p) {exit 1}; $p.MainWindowTitle"],
                capture_output=True)
            if check.returncode == 0:
                print(f"{browser}: installed extension opened a responsive workbench")
                return
            time.sleep(0.4)
        raise RuntimeError("Missing workbench after popup activation: " + driver.find_element(By.TAG_NAME, "body").text)
    finally:
        driver.quit()


if __name__ == "__main__":
    run(sys.argv[1], Path(sys.argv[2]).resolve(strict=True))
