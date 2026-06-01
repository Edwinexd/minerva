#!/usr/bin/env python3
"""
Validate the git-managed Grafana dashboards under
k8s/base/observability/dashboards/.

These dashboards are provisioned from disk into the `grafana-dashboards`
ConfigMap (see k8s/base/observability/kustomization.yaml) and hot-reloaded
by Grafana's file provider, so a malformed one ships silently: Grafana just
fails to load that file and the panel is gone, with no deploy-time error.
This script is the deploy-time error. It is both the pre-commit hook
(`validate-dashboards`) and a standalone test:

    python3 scripts/validate-dashboards.py

Checks, per dashboard:
  - valid JSON; required top-level fields (uid / title / schemaVersion /
    panels) with sane types; uid is a Grafana-legal slug.
  - panel ids are unique.
  - every gridPos fits the 24-column grid (0 <= x, 1 <= w <= 24, x+w <= 24).
  - LAYOUT: panels sharing a `y` (a row) sum to exactly width 24, so every
    row tiles the full grid with no dead space; and no two panels overlap.
  - datasource uids referenced by panels/targets exist in the provisioning
    (prometheus / loki), or are template vars / Grafana built-ins.
  - query panels carry a non-empty expr + refId per target.

Cross-file:
  - dashboard uids are unique across files (a collision silently drops one
    in Grafana).
  - the set of *.json files matches the file list in the kustomization's
    configMapGenerator, so a newly-added dashboard can't be forgotten from
    the ConfigMap (or a deleted one left dangling).

Exit 1 and print `file: message` lines on any violation.
"""

from __future__ import annotations

import glob
import json
import os
import re
import sys

GRID_WIDTH = 24

# Repo-relative; the hook runs `pass_filenames: false` so we always scan the
# whole set (needed for the cross-file uid + kustomization checks).
DASHBOARD_DIR = "k8s/base/observability/dashboards"
KUSTOMIZATION = "k8s/base/observability/kustomization.yaml"

# Datasource uids defined in grafana.yaml's provisioning. Keep in sync if a
# datasource is added there. Concrete uids must be in this set; template
# variables (`${...}`) and Grafana built-ins (`-- Grafana --`) pass through.
ALLOWED_DS_UIDS = {"prometheus", "loki"}

UID_RE = re.compile(r"^[a-zA-Z0-9_-]{1,40}$")

# Panel types that don't carry data queries, so they're exempt from the
# expr/refId check.
NON_QUERY_PANELS = {"row", "text", "dashlist", "news", "alertlist"}


def _iter_leaf_panels(panels):
    """Flatten one level of row-nested panels into a flat panel list."""
    for p in panels:
        if not isinstance(p, dict):
            continue
        if p.get("type") == "row" and isinstance(p.get("panels"), list):
            # Collapsed-row children render as real panels; include them.
            yield p
            yield from _iter_leaf_panels(p["panels"])
        else:
            yield p


def _ds_uid_ok(ds) -> bool:
    if ds is None:
        return True  # inherits the dashboard/default datasource
    if isinstance(ds, str):
        uid = ds
    elif isinstance(ds, dict):
        uid = ds.get("uid")
    else:
        return False
    if uid is None:
        return True
    if uid.startswith("$") or uid.startswith("--"):
        return True  # template var or Grafana built-in (-- Grafana --)
    return uid in ALLOWED_DS_UIDS


def validate_dashboard(path: str, errors: list[str]) -> str | None:
    """Validate one file. Returns its uid (for the cross-file check) or None."""

    def err(msg: str) -> None:
        errors.append(f"{path}: {msg}")

    try:
        with open(path, "r", encoding="utf-8") as f:
            doc = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        err(f"not valid JSON: {e}")
        return None

    uid = doc.get("uid")
    if not isinstance(uid, str) or not uid:
        err("missing/empty top-level 'uid'")
        uid = None
    elif not UID_RE.match(uid):
        err(f"uid {uid!r} is not a Grafana slug (^[A-Za-z0-9_-]{{1,40}}$)")

    if not isinstance(doc.get("title"), str) or not doc["title"]:
        err("missing/empty top-level 'title'")
    if not isinstance(doc.get("schemaVersion"), int):
        err("missing/non-integer 'schemaVersion'")

    panels_raw = doc.get("panels")
    if not isinstance(panels_raw, list):
        err("missing 'panels' array")
        return uid

    panels = list(_iter_leaf_panels(panels_raw))

    # Unique panel ids.
    seen_ids: dict[int, int] = {}
    for p in panels:
        pid = p.get("id")
        if pid is None:
            err(f"panel {p.get('title', '<untitled>')!r} has no id")
            continue
        seen_ids[pid] = seen_ids.get(pid, 0) + 1
    for pid, n in seen_ids.items():
        if n > 1:
            err(f"panel id {pid} used {n} times (ids must be unique)")

    # gridPos validity + datasource + targets.
    for p in panels:
        title = p.get("title", f"id={p.get('id')}")
        gp = p.get("gridPos")
        if not isinstance(gp, dict) or not all(k in gp for k in ("x", "y", "w", "h")):
            err(f"panel {title!r} missing a complete gridPos {{x,y,w,h}}")
        else:
            x, y, w, h = gp["x"], gp["y"], gp["w"], gp["h"]
            if not all(isinstance(v, int) for v in (x, y, w, h)):
                err(f"panel {title!r} gridPos values must be integers")
            else:
                if x < 0 or y < 0:
                    err(f"panel {title!r} has negative gridPos x/y")
                if not (1 <= w <= GRID_WIDTH):
                    err(f"panel {title!r} width {w} out of range 1..{GRID_WIDTH}")
                if h < 1:
                    err(f"panel {title!r} height {h} must be >= 1")
                if x + w > GRID_WIDTH:
                    err(f"panel {title!r} overflows grid: x({x})+w({w}) > {GRID_WIDTH}")

        if not _ds_uid_ok(p.get("datasource")):
            err(f"panel {title!r} references unknown datasource {p.get('datasource')!r}")

        if p.get("type") not in NON_QUERY_PANELS:
            targets = p.get("targets", [])
            if not isinstance(targets, list) or not targets:
                err(f"panel {title!r} has no targets")
            else:
                for t in targets:
                    if not _ds_uid_ok(t.get("datasource")):
                        err(f"panel {title!r} target references unknown datasource {t.get('datasource')!r}")
                    if not str(t.get("expr", "")).strip():
                        err(f"panel {title!r} target {t.get('refId', '?')} has empty expr")
                    if not t.get("refId"):
                        err(f"panel {title!r} has a target with no refId")

    _check_layout(panels, err)
    return uid


def _check_layout(panels, err) -> None:
    """Rows tile to full width 24, and no two panels overlap."""
    boxes = []
    for p in panels:
        gp = p.get("gridPos")
        if not isinstance(gp, dict):
            continue
        try:
            x, y, w, h = int(gp["x"]), int(gp["y"]), int(gp["w"]), int(gp["h"])
        except (KeyError, TypeError, ValueError):
            continue  # already reported by the gridPos check
        boxes.append((x, y, w, h, p.get("id")))

    # Each row (panels sharing a y) must sum to exactly the grid width, so
    # there's no dead horizontal space. This is the "all rows sum to the
    # same width" invariant.
    width_by_y: dict[int, int] = {}
    for x, y, w, h, pid in boxes:
        width_by_y[y] = width_by_y.get(y, 0) + w
    for y in sorted(width_by_y):
        total = width_by_y[y]
        if total != GRID_WIDTH:
            err(f"row at y={y} sums to width {total}, expected {GRID_WIDTH} "
                f"(panels in a row must tile the full grid)")

    # No overlaps: scan the occupancy grid.
    occupied: dict[tuple[int, int], int] = {}
    for x, y, w, h, pid in boxes:
        for cx in range(x, x + w):
            for cy in range(y, y + h):
                cell = (cx, cy)
                if cell in occupied:
                    err(f"panel id {pid} overlaps panel id {occupied[cell]} "
                        f"at grid cell ({cx},{cy})")
                    return  # one overlap report is enough; they cascade
                occupied[cell] = pid


def _check_kustomization_in_sync(json_files: list[str], errors: list[str]) -> None:
    """The configMapGenerator file list must match the *.json on disk."""
    try:
        with open(KUSTOMIZATION, "r", encoding="utf-8") as f:
            text = f.read()
    except OSError as e:
        errors.append(f"{KUSTOMIZATION}: cannot read ({e})")
        return
    listed = set(re.findall(r"dashboards/([\w.-]+\.json)", text))
    on_disk = {os.path.basename(p) for p in json_files}
    for missing in sorted(on_disk - listed):
        errors.append(
            f"{KUSTOMIZATION}: dashboard {missing!r} exists on disk but is not "
            f"in the configMapGenerator file list (it won't ship to Grafana)"
        )
    for dangling in sorted(listed - on_disk):
        errors.append(
            f"{KUSTOMIZATION}: configMapGenerator lists {dangling!r} but the "
            f"file does not exist"
        )


def main() -> int:
    json_files = sorted(glob.glob(os.path.join(DASHBOARD_DIR, "*.json")))
    errors: list[str] = []

    if not json_files:
        print(f"no dashboards found under {DASHBOARD_DIR}/", file=sys.stderr)
        return 1

    uids: dict[str, str] = {}
    for path in json_files:
        uid = validate_dashboard(path, errors)
        if uid:
            if uid in uids:
                errors.append(
                    f"{path}: uid {uid!r} also used by {uids[uid]} "
                    f"(dashboard uids must be unique)"
                )
            else:
                uids[uid] = path

    _check_kustomization_in_sync(json_files, errors)

    if errors:
        print("Dashboard validation failed:", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
        return 1

    print(f"OK: {len(json_files)} dashboard(s) valid "
          f"({', '.join(os.path.basename(p) for p in json_files)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
