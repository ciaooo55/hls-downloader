from pathlib import Path

path = Path("native_shell/src/cast.rs")
text = path.read_text(encoding="utf-8")
old = "            for byte in [b'b', b'c', b'd'] {\n"
new = "            for byte in *b\"bcd\" {\n"
if old not in text:
    raise SystemExit("TVBox deadline byte loop anchor missing")
text = text.replace(old, new, 1)
path.write_text(text, encoding="utf-8")
