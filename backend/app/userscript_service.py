import json
import os
import re
import uuid
from pathlib import Path


USERSCRIPT_FILENAME = "m3u8-sniffer.user.js"
USERSCRIPT_VERSION = "4.3.0"


def render_userscript(source: str, host: str, port: int, token: str, version: str = USERSCRIPT_VERSION) -> str:
    api_base = f"http://{host}:{port}/api"
    replacements = {
        "API_BASE": json.dumps(api_base, ensure_ascii=False),
        "TOKEN": json.dumps(token, ensure_ascii=False),
        "SCRIPT_VERSION": json.dumps(version, ensure_ascii=False),
    }
    rendered = source
    for name, value in replacements.items():
        pattern = re.compile(
            rf"^(\s*const {name}\s*=\s*).+?;(\s*//.*)?\s*$",
            re.MULTILINE,
        )
        rendered, count = pattern.subn(
            lambda match: f"{match.group(1)}{value};{match.group(2) or ''}",
            rendered,
            count=1,
        )
        if count != 1:
            raise ValueError(f"userscript constant not found: {name}")
    rendered, count = re.subn(
        r"^(// @version\s+).+$",
        lambda match: f"{match.group(1)}{version}",
        rendered,
        count=1,
        flags=re.MULTILINE,
    )
    if count != 1:
        raise ValueError("userscript metadata version not found")
    return rendered


def export_userscript(directory: str | Path, content: str, overwrite: bool = False) -> Path:
    folder = Path(directory)
    if not folder.exists() or not folder.is_dir():
        raise NotADirectoryError(str(folder))
    target = folder / USERSCRIPT_FILENAME
    if target.exists() and not overwrite:
        raise FileExistsError(str(target))

    temporary = folder / f".{USERSCRIPT_FILENAME}.{uuid.uuid4().hex}.tmp"
    try:
        temporary.write_text(content, encoding="utf-8", newline="\n")
        os.replace(temporary, target)
    finally:
        temporary.unlink(missing_ok=True)
    return target
