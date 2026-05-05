"""Stream the per-course study NDJSON export to a local file.

The admin UI also has a "Download JSONL" button that does this in the
browser; this script is for headless usage (cron, ad-hoc shell pulls,
CI). Each line of the output is a self-contained JSON object for one
participant; see `routes::study::admin_export_jsonl` for the schema.

Authentication mirrors the rest of the admin endpoints: in production
you need a Shibboleth session cookie (paste from devtools); in dev mode
set `MINERVA_DEV_USER` and the `X-Dev-User` header is sent instead.

Usage:
    python scripts/fetch_study_export.py \\
        --course-id <uuid> \\
        --output study-aegis.jsonl

Env:
    MINERVA_BASE_URL    default https://minerva.dsv.su.se
    MINERVA_SHIB_COOKIE  raw Cookie header value (e.g.
                         "_shibsession_xxx=yyy"); needed in prod
    MINERVA_DEV_USER     eppn for dev-mode bypass; needed in dev
"""

from __future__ import annotations

import argparse
import os
import sys

import requests


DEFAULT_BASE_URL = "https://minerva.dsv.su.se"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--course-id",
        required=True,
        help="UUID of the study course to export.",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Path to write the NDJSON to. Existing files are overwritten.",
    )
    parser.add_argument(
        "--base-url",
        default=os.environ.get("MINERVA_BASE_URL", DEFAULT_BASE_URL),
        help="Minerva base URL (default %(default)s).",
    )
    args = parser.parse_args()

    url = f"{args.base_url.rstrip('/')}/api/admin/study/courses/{args.course_id}/export.jsonl"

    headers: dict[str, str] = {}
    cookie = os.environ.get("MINERVA_SHIB_COOKIE")
    dev_user = os.environ.get("MINERVA_DEV_USER")
    if cookie:
        headers["Cookie"] = cookie
    if dev_user:
        headers["X-Dev-User"] = dev_user
    if not cookie and not dev_user:
        print(
            "warning: neither MINERVA_SHIB_COOKIE nor MINERVA_DEV_USER set; "
            "the request will likely 401.",
            file=sys.stderr,
        )

    print(f"GET {url}", file=sys.stderr)
    with requests.get(url, headers=headers, stream=True, timeout=300) as resp:
        if resp.status_code != 200:
            # Surface the body so the operator can see the error code/msg.
            print(
                f"error: {resp.status_code} {resp.reason}\n{resp.text}",
                file=sys.stderr,
            )
            return 1

        # Stream chunk-by-chunk so we don't buffer the whole file in
        # memory; the server already sends one line per participant
        # without internal buffering.
        with open(args.output, "wb") as f:
            for chunk in resp.iter_content(chunk_size=64 * 1024):
                if chunk:
                    f.write(chunk)

    size = os.path.getsize(args.output)
    print(f"wrote {args.output} ({size} bytes)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
