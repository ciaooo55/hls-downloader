"""Generate a deterministic CycloneDX SBOM from committed dependency locks."""

from __future__ import annotations

import argparse
import json
import tomllib
import uuid
from pathlib import Path
from urllib.parse import quote

import yaml


ROOT = Path(__file__).resolve().parents[2]


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
    packages = lock.get("package")
    if not isinstance(packages, list) or not packages:
        raise ValueError(f"{lock_path}: missing Cargo packages")
    values = []
    for item in packages:
        if not isinstance(item, dict) or not all(
            isinstance(item.get(key), str) and item[key].strip() for key in ("name", "version")
        ):
            raise ValueError(f"{lock_path}: malformed Cargo package")
        values.append(component("library", "cargo", item["name"], item["version"]))
    return values


def npm_components(lock_path: Path) -> list[dict]:
    lock = yaml.safe_load(lock_path.read_text(encoding="utf-8"))
    packages = lock.get("packages") if isinstance(lock, dict) else None
    if not isinstance(packages, dict) or not packages:
        raise ValueError(f"{lock_path}: missing pnpm packages")
    values = []
    for key in packages:
        if not isinstance(key, str):
            raise ValueError(f"{lock_path}: invalid pnpm package key {key!r}")
        name, _, version = key.partition("(")[0].rpartition("@")
        if not name.strip() or not version.strip():
            raise ValueError(f"{lock_path}: invalid pnpm package key {key!r}")
        values.append(component("library", "npm", name, version))
    return values


def build_sbom(version: str, ui_runtime: str = "native") -> dict:
    values = [
        *cargo_components(ROOT / "Cargo.lock"),
        *npm_components(ROOT / "extension" / "pnpm-lock.yaml"),
        component("application", "generic", "ffmpeg", "9.0.2"),
        component("library", "generic", "libmpv", "20261110-git-a61f6c3abd"),
    ]
    unique = {item["bom-ref"]: item for item in values}
    components = [unique[key] for key in sorted(unique)]
    identity = "\n".join([version, *(item["bom-ref"] for item in components)])
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
    parser.add_argument("--ui-runtime", choices=("native",), default="native")
    args = parser.parse_args()
    payload = build_sbom(str(args.version).strip().lstrip("v"), args.ui_runtime)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
