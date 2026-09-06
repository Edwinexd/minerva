"""Shared plumbing for the scripts that talk to Minerva's service API.

Both `fetch_transcripts.py` and `sync_daisy_courses.py` run as
`python scripts/<name>.py` from the repo root in GitHub Actions, which
puts the script's own directory on sys.path, so this sibling module
imports without any packaging.

Requires: MINERVA_API_URL, MINERVA_SERVICE_API_KEY (and SU_USERNAME /
SU_PASSWORD for the dsv-wrapper credentials helper).
"""

import os
import sys

import requests

# Default per-request timeout, in seconds. `sync_daisy_courses.py` already
# passed timeout=300 on both of its calls; `fetch_transcripts.py` passed
# none, so a stalled socket could hang the hourly run indefinitely. The
# session below applies this to every request that does not set its own.
DEFAULT_TIMEOUT = 300


def get_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        print(f"error: {name} not set", file=sys.stderr)
        sys.exit(1)
    return value


class MinervaSession(requests.Session):
    """Session that carries the service-API bearer token, the base url,
    and a default timeout.

    Two conveniences, both so call sites stop threading `(api_url,
    headers)` through every function:

      * a url starting with "/" is resolved against `base_url`, i.e.
        `session.get("/api/service/x")` hits `{base_url}/api/service/x`.
        Absolute urls are passed through untouched.
      * requests has no session-level timeout, so `request()` injects
        DEFAULT_TIMEOUT only when the caller did not pass one; an
        explicit timeout always wins.
    """

    def __init__(self, base_url: str, api_key: str) -> None:
        super().__init__()
        self.base_url = base_url
        self.headers["Authorization"] = f"Bearer {api_key}"

    def request(self, method, url, *args, **kwargs):
        if url.startswith("/"):
            url = f"{self.base_url}{url}"
        if "timeout" not in kwargs:
            kwargs["timeout"] = DEFAULT_TIMEOUT
        return super().request(method, url, *args, **kwargs)


def minerva_session() -> MinervaSession:
    """Build the service-API session from the environment. Exits via
    `get_env` when MINERVA_API_URL / MINERVA_SERVICE_API_KEY are unset."""
    base_url = get_env("MINERVA_API_URL").rstrip("/")
    api_key = get_env("MINERVA_SERVICE_API_KEY")
    return MinervaSession(base_url, api_key)


def su_credentials() -> tuple[str, str]:
    """SU login used by dsv-wrapper's Play / Daisy clients."""
    return get_env("SU_USERNAME"), get_env("SU_PASSWORD")
