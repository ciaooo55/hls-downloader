"""Package the validated local runtime, excluding all user profile data."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import zipfile


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def run(args):
    repo = Path(__file__).resolve().parents[1]
    root, output, report_path = args.runtime.resolve(), args.output.resolve(), args.report.resolve()
    if not all(p.is_relative_to(repo) for p in (root, output, report_path)):
        raise ValueError("Runtime, package and report must remain in the project directory")
    if output.exists():
        raise FileExistsError("Do not overwrite an existing delivery package")
    evidence = report_path.parent
    for name in ("portable-app.json", "portable-browser.json"):
        if not json.loads((evidence / name).read_text(encoding="utf-8"))["passed"]:
            raise ValueError(f"Active portable validation failed: {name}")
    resources = root / "app/resources"
    provenance = json.loads((resources / "BUILD-PROVENANCE.json").read_text(encoding="utf-8"))
    if provenance.get("package_tier") != "local-validation" or provenance.get("release_ready") is not False:
        raise ValueError("This packager only handles explicitly local validation runtimes")
    patch = provenance["local_runtime_patch"]
    snapshot = json.loads((resources / "SOURCE-SNAPSHOT.json").read_text(encoding="utf-8"))
    patched_sources = {name.replace("\\", "/"): sha for name, sha in patch["modified_sources"].items()}
    for name, sha in {**snapshot, **patched_sources}.items():
        if digest(repo / name) != sha:
            raise ValueError(f"Runtime source does not match its build: {name}")
    snapshot.update(patched_sources)
    cfg = (root / "app/HLSDownloader.cfg").read_text(encoding="utf-8")
    jar_name = next(line.split("$APPDIR\\", 1)[1] for line in cfg.splitlines() if line.startswith("app.classpath=$APPDIR\\HLSDownloaderDesktop-"))
    if digest(root / "app" / jar_name) != patch["sha256"]:
        raise ValueError("The patched Compose JAR does not match its provenance")
    provenance.update(package_tier="local-validation", package_format="portable", release_ready=False,
                      packaged_at_utc=datetime.now(timezone.utc).isoformat(), user_profile_included=False,
                      feature_parity_path="artifacts/v7-productization/feature-parity.json",
                      feature_parity_sha256=digest(repo / "artifacts/v7-productization/feature-parity.json"))
    files = {}
    for directory in ("app", "runtime", "extensions"):
        for path in sorted((root / directory).rglob("*")):
            if path.is_symlink() or path.is_junction():
                raise ValueError(f"Unexpected runtime link: {path}")
            if path.is_file():
                name = path.relative_to(root).as_posix()
                if name.endswith(("HLSDownloaderNativeHost.chrome.json", "HLSDownloaderNativeHost.firefox.json")):
                    continue
                files[name] = path
    files["HLSDownloader.exe"] = root / "HLSDownloader.exe"
    for name in ("LICENSE", "PRIVACY.md", "TERMS.md", "THIRD_PARTY_NOTICES.md"):
        files[name] = repo / name
    generated = {
        "portable": b"",
        "app/resources/BUILD-PROVENANCE.json": json.dumps(provenance, ensure_ascii=False, indent=2).encode("utf-8"),
        "app/resources/SOURCE-SNAPSHOT.json": json.dumps(snapshot, ensure_ascii=False, indent=2).encode("utf-8"),
        "app/resources/FEATURE-PARITY.json": (repo / "artifacts/v7-productization/feature-parity.json").read_bytes(),
        "Register-Browser.cmd": b'@echo off\r\n"%~dp0app\\resources\\HLSDownloaderEngine.exe" --register-native-host\r\nif errorlevel 1 pause\r\n',
        "README.txt": (
            "HLS Downloader 7.0.2 本地便携验证版\n\n"
            "解压后双击 HLSDownloader.exe。首次运行会在本目录创建 data 和 downloads。\n"
            "接入浏览器：先运行 Register-Browser.cmd（当前用户注册，无需管理员），再加载 extensions/Chromium 中的插件。\n"
            "Chrome/Edge：扩展管理页开启开发者模式，选择加载已解压的扩展。移动整个目录后重新运行 Register-Browser.cmd。\n"
            "Firefox 的当前开发插件目录在 extensions/Firefox；这是未签名源码构建，开发加载不会自动成为持久安装。\n"
            "网页字幕按钮和插件设置支持总开关、当前页面/站点控制、来源与目标语言，默认自动识别到中文。\n"
            "真实云端翻译需另行配置百炼账户；本包不包含账户凭据、用户数据库或下载文件。\n"
            "本包含便携工作台路径修复，已完成实际下载和浏览器链路验收；旧 MSI 不含本次修复。\n"
            "本地验证包未签名，release_ready=false；不代表所有网站或真实云端字幕质量已通过。\n"
        ).encode("utf-8"),
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    manifest = {}
    with zipfile.ZipFile(output, "x", zipfile.ZIP_DEFLATED, compresslevel=1) as archive:
        for name, path in files.items():
            if name in generated:
                continue
            manifest[name] = digest(path)
            archive.write(path, "HLSDownloader/"+name)
        for name, content in generated.items():
            manifest[name] = hashlib.sha256(content).hexdigest()
            archive.writestr("HLSDownloader/"+name, content)
    print("Portable archive created; verifying every file", flush=True)
    with zipfile.ZipFile(output) as archive:
        for name, sha in manifest.items():
            with archive.open("HLSDownloader/"+name) as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != sha:
                    raise ValueError(f"Packaged file hash mismatch: {name}")
        if any(name.startswith(("HLSDownloader/data/", "HLSDownloader/downloads/")) for name in archive.namelist()):
            raise ValueError("User profile data must never be packaged")
    report = {"passed": True, "package_tier": "local-validation", "release_ready": False,
              "path": str(output), "bytes": output.stat().st_size, "sha256": digest(output),
              "user_profile_included": False, "files_sha256_verified": len(manifest), "source_files_verified": len(snapshot),
              "files": manifest}
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({k: v for k, v in report.items() if k != "files"}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    run(parser.parse_args())
