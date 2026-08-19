from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
CORE_TOKENS = {
    "bg",
    "surface",
    "surface-2",
    "surface-3",
    "border",
    "text",
    "muted",
    "faint",
    "primary",
    "primary-hover",
    "on-primary",
    "green",
    "amber",
    "red",
    "purple",
    "shadow",
    "rail",
}


def _tokens(path: Path, selector: str) -> dict[str, str]:
    text = path.read_text(encoding="utf-8")
    match = re.search(rf"{re.escape(selector)}\s*\{{([^}}]+)\}}", text)
    assert match, f"missing theme selector {selector!r} in {path}"
    return {
        name: re.sub(r"\s+", "", value).lower()
        for name, value in re.findall(r"--([a-z0-9-]+)\s*:\s*([^;]+);", match.group(1))
    }


def _core(tokens: dict[str, str]) -> dict[str, str]:
    missing = CORE_TOKENS - tokens.keys()
    assert not missing, f"missing shared theme tokens: {sorted(missing)}"
    return {name: tokens[name] for name in sorted(CORE_TOKENS)}


def test_desktop_base_and_cockpit_tokens_match() -> None:
    base = ROOT / "frontend" / "src" / "styles.css"
    cockpit = ROOT / "frontend" / "src" / "cockpit-shell.css"

    assert _core(_tokens(base, ":root")) == _core(_tokens(cockpit, ":root"))
    assert _core(_tokens(base, ':root[data-theme="dark"]')) == _core(
        _tokens(cockpit, ':root[data-theme="dark"]')
    )


def test_extension_and_desktop_tokens_match() -> None:
    cockpit = ROOT / "frontend" / "src" / "cockpit-shell.css"
    extension = ROOT / "extension" / "lib" / "theme.ts"

    assert _core(_tokens(extension, '[data-hlsd-theme="light"]')) == _core(
        _tokens(cockpit, ":root")
    )
    assert _core(_tokens(extension, '[data-hlsd-theme="dark"]')) == _core(
        _tokens(cockpit, ':root[data-theme="dark"]')
    )
