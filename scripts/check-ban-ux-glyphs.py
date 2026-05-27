#!/usr/bin/env python3
"""
Block UX-decorative Unicode glyphs (emoji, arrows, dingbats, misc symbols)
from landing in source. Latin letters with diacritics (e.g. Swedish aao),
em-dash (which has its own hook), and box-drawing characters are allowed.

Pre-commit invokes this with the list of changed files; we exit 1 and print
offending `file:lineno: line` to stderr if any are found.

Ranges blocked:
  U+2190..U+21FF  Arrows
  U+2600..U+26FF  Miscellaneous Symbols
  U+2700..U+27BF  Dingbats
  U+2B00..U+2BFF  Miscellaneous Symbols and Arrows
  U+1F300..U+1FAFF Emoji and pictograph supplemental planes
"""

from __future__ import annotations

import re
import sys


# Pattern uses `\uXXXX` / `\UXXXXXXXX` escapes so the script itself does
# not trip the rule it enforces. Python decodes the escapes at compile
# time so the resulting character class matches the literal code points.
PATTERN = re.compile(
    "["
    "\u2190-\u21FF"          # Arrows
    "\u2600-\u26FF"          # Miscellaneous Symbols
    "\u2700-\u27BF"          # Dingbats
    "\u2B00-\u2BFF"          # Miscellaneous Symbols and Arrows
    "\U0001F300-\U0001FAFF"  # Emoji / pictograph supplemental planes
    "]"
)


def main(paths: list[str]) -> int:
    bad = False
    for path in paths:
        try:
            with open(path, "r", encoding="utf-8") as f:
                for lineno, line in enumerate(f, 1):
                    m = PATTERN.search(line)
                    if m:
                        ch = m.group(0)
                        sys.stderr.write(
                            f"{path}:{lineno}: U+{ord(ch):04X} {ch!r}  {line}"
                        )
                        bad = True
        except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
            # Binary files / non-UTF8 / missing paths: pre-commit's `types:
            # [text]` filter usually catches these; ignore silently if not.
            continue
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
