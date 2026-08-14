from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

from .config import PROJECT_ROOT


def locate_native_engine_executable(project_root: Path | None = None) -> Path | None:
    """Packaged HTTP engine next to Core/Shell. Source debug builds are last."""
    names = ("HLSNativeEngine.exe", "hls-native-engine.exe", "hls-native-engine")
    env_path = str(os.environ.get("HLS_NATIVE_ENGINE") or "").strip()
    if env_path:
        configured = Path(env_path)
        if configured.is_file():
            return configured
        if configured.is_dir():
            for name in names:
                candidate = configured / name
                if candidate.is_file():
                    return candidate

    roots: list[Path] = []
    if project_root is not None:
        roots.append(Path(project_root))
    roots.append(Path(PROJECT_ROOT))
    roots.append(Path(os.environ.get("HLS_NATIVE_SHELL_ROOT") or "."))
    if getattr(sys, "frozen", False):
        roots.append(Path(sys.executable).resolve().parent)
    roots.append(Path.cwd())
    crate = native_engine_source_dir()
    if crate is not None:
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


def native_engine_source_dir() -> Path | None:
    here = Path(__file__).resolve()
    for parent in here.parents:
        candidate = parent / "native_engine"
        if (candidate / "Cargo.toml").is_file():
            return candidate
    return None


def build_native_engine_debug() -> Path | None:
    source = native_engine_source_dir()
    if source is None:
        return None
    cargo = "cargo.exe" if os.name == "nt" else "cargo"
    target_name = "hls-native-engine.exe" if os.name == "nt" else "hls-native-engine"
    target = source / "target" / "debug" / target_name
    completed = subprocess.run(
        [cargo, "build", "--manifest-path", str(source / "Cargo.toml")],
        cwd=str(source),
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0 or not target.is_file():
        return None
    return target


def write_native_job(*, job_path: Path, payload: dict) -> Path:
    job_path.parent.mkdir(parents=True, exist_ok=True)
    job_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
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
