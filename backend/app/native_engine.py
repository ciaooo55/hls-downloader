from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

from .config import PROJECT_ROOT
from .native_shell import locate_native_shell_executable


def locate_native_engine_executable(project_root: Path | None = None) -> Path | None:
    """Same HLSNativeShell.exe, invoked with --job. No second binary."""
    env_path = str(os.environ.get("HLS_NATIVE_ENGINE") or "").strip()
    if env_path:
        configured = Path(env_path)
        if configured.is_file():
            return configured
    packaged = locate_native_shell_executable(project_root)
    if packaged is not None:
        return packaged
    names = ("HLSNativeShell.exe", "hls-native-shell.exe", "hls-native-shell")
    roots = [
        Path(PROJECT_ROOT),
        Path(os.environ.get("HLS_NATIVE_SHELL_ROOT") or "."),
        Path.cwd(),
    ]
    if getattr(sys, "frozen", False):
        roots.insert(0, Path(sys.executable).resolve().parent)
    crate = Path(__file__).resolve().parents[2] / "native_shell"
    if (crate / "Cargo.toml").is_file():
        roots.append(crate / "target" / "release")
        roots.append(crate / "target" / "debug")
    seen: set[Path] = set()
    for root in roots:
        try:
            resolved = root.resolve()
        except OSError:
            resolved = root
        if resolved in seen:
            continue
        seen.add(resolved)
        for name in names:
            candidate = resolved / name
            if candidate.is_file():
                return candidate
    return None


def build_native_engine_debug() -> Path | None:
    crate = Path(__file__).resolve().parents[2] / "native_shell"
    if not (crate / "Cargo.toml").is_file():
        return None
    cargo = "cargo.exe" if os.name == "nt" else "cargo"
    target_name = "hls-native-shell.exe" if os.name == "nt" else "hls-native-shell"
    target = crate / "target" / "debug" / target_name
    completed = subprocess.run(
        [cargo, "build", "--manifest-path", str(crate / "Cargo.toml")],
        cwd=str(crate),
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0 or not target.is_file():
        return None
    return target


def write_native_job(*, job_path: Path, payload: dict) -> Path:
    job_path.parent.mkdir(parents=True, exist_ok=True)
    job_path.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    return job_path


def run_native_engine(
    *,
    executable: Path,
    job_path: Path,
    cwd: Path | None = None,
) -> subprocess.Popen[bytes]:
    creationflags = 0
    startupinfo = None
    if os.name == "nt":
        creationflags = int(getattr(subprocess, "CREATE_NO_WINDOW", 0))
        startupinfo = subprocess.STARTUPINFO()
        startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startupinfo.wShowWindow = 0
    return subprocess.Popen(
        [str(executable), "--job", str(job_path)],
        cwd=str(cwd or executable.parent),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        startupinfo=startupinfo,
        creationflags=creationflags,
    )


def resolved_proxy_url(url: str) -> str:
    from .network_proxy import _proxy_route

    kind, proxy = _proxy_route(url)
    if kind == "proxy" and proxy:
        return str(proxy)
    return ""
