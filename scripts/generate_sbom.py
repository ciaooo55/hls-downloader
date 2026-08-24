"""Generate a deterministic CycloneDX SBOM from committed dependency locks."""

from __future__ import annotations

import argparse
import json
import tomllib
import uuid
from pathlib import Path
from urllib.parse import quote

import yaml


ROOT = Path(__file__).resolve().parents[1]
def component(kind: str, ecosystem: str, name: str, version: str) -> dict:
    normalized_name = name.strip()
    normalized_version = version.split("(", 1)[0].strip()
    purl_name = quote(normalized_name, safe="/")
    return {
        "type": kind,
        "bom-ref": f"pkg:{ecosystem}/{purl_name}@{normalized_version}",
        "name": normalized_name,
        "version": normalized_version,
        "purl": f"pkg:{ecosystem}/{purl_name}@{normalized_version}",
    }


def cargo_components(lock_path: Path) -> list[dict]:
    lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    return [
        component("library", "cargo", str(item["name"]), str(item["version"]))
        for item in lock.get("package", [])
        if item.get("name") and item.get("version")
    ]


def npm_components(lock_path: Path) -> list[dict]:
    lock = yaml.safe_load(lock_path.read_text(encoding="utf-8")) or {}
    values = []
    for key in (lock.get("packages") or {}):
        raw = str(key)
        if "@" not in raw:
            continue
        name, version = raw.rsplit("@", 1)
        if name and version:
            values.append(component("library", "npm", name, version))
    return values


def compose_components() -> list[dict]:
    # Compose Desktop keeps its deliberately small production dependency set in
    # build.gradle.kts. Record the locked versions without reviving the removed
    # React/Tauri dependency graph.
    return [
        component("framework", "maven", "org.jetbrains.compose.desktop/desktop", "1.11.1"),
        component("library", "maven", "org.jetbrains.compose.material/material-icons-extended", "1.7.3"),
        component("library", "maven", "org.jetbrains.kotlinx/kotlinx-coroutines-swing", "1.10.2"),
        component("library", "maven", "org.jetbrains.kotlinx/kotlinx-serialization-json", "1.9.0"),
        component("framework", "generic", "Eclipse Temurin JRE", "21"),
    ]


def build_sbom(version: str) -> dict:
    values = [
        *cargo_components(ROOT / "native_shell" / "Cargo.lock"),
        *cargo_components(ROOT / "presenter_ui" / "Cargo.lock"),
        *compose_components(),
        *npm_components(ROOT / "extension" / "pnpm-lock.yaml"),
        component("application", "generic", "ffmpeg", "8.1.2"),
        component("library", "generic", "libmpv", "20260814-git-7b8915bc1d"),
    ]
    unique = {item["bom-ref"]: item for item in values}
    components = [unique[key] for key in sorted(unique)]
    identity = "\n".join(unique)
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "serialNumber": f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, identity)}",
        "version": 1,
        "metadata": {
            "component": component("application", "github", "ciaooo55/hls-downloader", version),
            "tools": {
                "components": [component("application", "generic", "hls-downloader-sbom-generator", "1")]
            },
        },
        "components": components,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    payload = build_sbom(str(args.version).strip().lstrip("v"))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
