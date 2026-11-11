#!/usr/bin/env python3
"""Fail when the architecture docs fall behind the code.

Two checks:

1. Coverage. Collects the system's components from the code (binaries,
   workspace kinds, scheduler loops, mirrored document sources, LLM
   providers, GitHub workflows) and requires each to be named in
   docs/ARCHITECTURE.md or docs/diagrams/build.py. Matching ignores case
   and treats `_`, `-` and spaces alike, and CamelCase names also match
   as words (`CreditTransfer` matches "credit transfer").
2. Freshness. build.py stamps its own sha256 into every SVG it renders;
   an SVG whose stamp differs from the current build.py was not
   regenerated after the script changed.

Components that are deliberately left out go in IGNORED with a reason.
"""

from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = [ROOT / "docs" / "ARCHITECTURE.md", ROOT / "docs" / "diagrams" / "build.py"]
DIAGRAMS = ROOT / "docs" / "diagrams"
CRATES = ROOT / "backend" / "crates"
SCHEDULERS = CRATES / "minerva-app-core" / "src" / "schedulers.rs"
MODELS = CRATES / "minerva-core" / "src" / "models.rs"
CONFIG = CRATES / "minerva-app-core" / "src" / "config.rs"
WORKFLOWS = ROOT / ".github" / "workflows"

STAMP_RE = re.compile(r"<!-- build\.py sha256: ([0-9a-f]{64}) -->")

# (kind, name) -> why it is not in the architecture docs.
IGNORED = {
    ("workflow", "ci"): "build and test only; covered by CLAUDE.md",
    ("workflow", "docker"): "image build; covered by CLAUDE.md",
    ("workflow", "deploy"): "deploy pipeline; covered by CLAUDE.md",
    ("workflow", "release"): "release packaging",
    ("workflow", "sync-moodle-plugin"): "plugin mirror; covered by CLAUDE.md",
    # Modules that only supply settings to a loop named by another module.
    ("scheduler loop", "system_defaults"): "settings lookup, not a loop",
}


def normalize(text: str) -> str:
    return re.sub(r"[\s_\-]+", " ", text.lower())


def words(name: str) -> str:
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z])", " ", name)


def binaries() -> list[str]:
    return sorted(p.parent.parent.name for p in CRATES.glob("*/src/main.rs"))


def workspace_kinds() -> list[str]:
    body = re.search(r"pub enum WorkspaceKind \{(.*?)\}", MODELS.read_text(), re.S)
    if not body:
        raise SystemExit(f"WorkspaceKind not found in {MODELS}")
    return re.findall(r"^\s*(\w+),", body.group(1), re.M)


def scheduler_loops() -> list[str]:
    """The module each spawned loop is named after: its first call into
    `crate::<module>` or `minerva_db::queries::<module>`."""
    text = SCHEDULERS.read_text()
    blocks = text.split("tokio::spawn(")[1:]
    loops = []
    call = re.compile(r"(?:crate|minerva_db::queries)::(\w+)::")
    for block in blocks:
        modules = [m for m in call.findall(block) if ("scheduler loop", m) not in IGNORED]
        if modules:
            loops.append(modules[0])
    return sorted(set(loops))


def source_systems() -> list[str]:
    found = set()
    for path in CRATES.glob("*/src/**/*.rs"):
        found.update(re.findall(r'SOURCE_SYSTEM: &str = "([^"]+)"', path.read_text()))
    return sorted(found)


def llm_providers() -> list[str]:
    keys = re.findall(r"(\w+)_api_key: env::var\(", CONFIG.read_text())
    return sorted(k for k in keys if k != "service")


def workflows() -> list[str]:
    return sorted(p.stem for p in WORKFLOWS.glob("*.yml"))


def coverage_errors() -> list[str]:
    haystack = normalize("\n".join(p.read_text() for p in DOCS))
    components = (
        [("binary", n) for n in binaries()]
        + [("workspace kind", n) for n in workspace_kinds()]
        + [("scheduler loop", n) for n in scheduler_loops()]
        + [("document source_system", n) for n in source_systems()]
        + [("LLM provider", n) for n in llm_providers()]
        + [("workflow", n) for n in workflows()]
    )
    errors = []
    for kind, name in components:
        if (kind, name) in IGNORED:
            continue
        if normalize(name) in haystack or normalize(words(name)) in haystack:
            continue
        errors.append(f"{kind} `{name}` is not named in docs/ARCHITECTURE.md or docs/diagrams/build.py")
    return errors


def freshness_errors() -> list[str]:
    digest = hashlib.sha256((DIAGRAMS / "build.py").read_bytes()).hexdigest()
    errors = []
    for svg in sorted(DIAGRAMS.glob("*.svg")):
        match = STAMP_RE.search(svg.read_text())
        if not match or match.group(1) != digest:
            errors.append(f"{svg.relative_to(ROOT)} is stale: re-run docs/diagrams/build.py")
    return errors


def main() -> int:
    errors = coverage_errors() + freshness_errors()
    for e in errors:
        print(e, file=sys.stderr)
    if errors:
        print(
            "\nDocument the component (or add it to IGNORED in "
            "scripts/check-architecture-coverage.py with a reason).",
            file=sys.stderr,
        )
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
