#!/usr/bin/env python3
"""Make the source-list endpoint able to return blank sources.

WHY THIS EXISTS
---------------
The server's export walks *every* source:

    Project.to_labelplus()          # app/models/project.py
      -> File.to_labelplus()        # app/models/file.py
           -> self.sources()        # no `blank` filter

but the only endpoint that lists sources calls

    File.to_translator(target, paging=..., user=...)   # app/apis/source.py

without `show_blank`, so it takes the default `show_blank=False` and executes
`sources.filter(blank=False)` (`app/models/file.py`).

Blank sources are not rare: importing a LabelPlus file creates one for every label that
has no translation. Dropping them shifts every subsequent label index in the exported
txt — silently, because the file still looks well-formed.

The desktop client sends `show_blank=true` and detects the filtering by looking for gaps
in the `rank` sequence, so an unpatched server produces a loud warning rather than a
quietly-wrong export. Applying this patch makes the output match the server's own export
byte for byte.

USAGE
-----
    python3 apply-backend-show-blank.py /path/to/moeflow-backend

Idempotent: running it twice is a no-op. Works on both `moeflow-com/moeflow` (backend-v1)
and the iroha fork.
"""

import re
import sys
from pathlib import Path

SCHEMA_ANCHOR = "class SourceSearchSchema(DefaultSchema):"
SCHEMA_FIELD = "    show_blank = fields.Bool(missing=False)\n"

HANDLER_OLD = "return file.to_translator(\n            target=target, paging=data[\"paging\"], user=self.current_user\n        )"
HANDLER_NEW = "return file.to_translator(\n            target=target,\n            paging=data[\"paging\"],\n            show_blank=data[\"show_blank\"],\n            user=self.current_user,\n        )"


def patch_schema(root: Path) -> str:
    path = root / "app" / "validators" / "source.py"
    text = path.read_text(encoding="utf-8")
    if "show_blank" in text:
        return "schema: already patched"

    if SCHEMA_ANCHOR not in text:
        raise SystemExit(f"anchor not found in {path}; upstream layout changed")

    text = text.replace(SCHEMA_ANCHOR, SCHEMA_ANCHOR + "\n" + SCHEMA_FIELD, 1)
    path.write_text(text, encoding="utf-8")
    return "schema: added show_blank field"


def patch_handler(root: Path) -> str:
    path = root / "app" / "apis" / "source.py"
    text = path.read_text(encoding="utf-8")
    if "show_blank=data" in text:
        return "handler: already patched"

    if HANDLER_OLD in text:
        text = text.replace(HANDLER_OLD, HANDLER_NEW, 1)
    else:
        # Tolerate reformatting (black, a different argument order, …).
        pattern = re.compile(
            r"return file\.to_translator\(\s*target=target,\s*paging=data\[\"paging\"\],\s*user=self\.current_user,?\s*\)"
        )
        text, count = pattern.subn(HANDLER_NEW, text, count=1)
        if count == 0:
            raise SystemExit(f"call site not found in {path}; upstream layout changed")

    path.write_text(text, encoding="utf-8")
    return "handler: forwards show_blank"


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    root = Path(sys.argv[1])
    if not (root / "app").is_dir():
        raise SystemExit(f"{root} does not look like a moeflow backend checkout")

    print(patch_schema(root))
    print(patch_handler(root))
    print("\nRestart the backend, then re-run a local export and compare against the")
    print("server's own export — they should now be byte-identical.")


if __name__ == "__main__":
    main()
