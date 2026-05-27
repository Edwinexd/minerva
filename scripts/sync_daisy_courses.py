"""Daily sync of DSV course offerings from Daisy into Minerva.

Walks the current and next semester via dsv-wrapper's
`daisy.get_courses(semester)` API, resolves each course's role-grouped
participants down to SU eppns, and idempotently posts the batch to
Minerva's `/api/service/daisy-courses` endpoint. Owner resolution,
membership additions, alias registration, and play-designation creation
all happen server-side; this script's only job is the dsv-wrapper
plumbing.

Called by `.github/workflows/daisy-sync.yml` on a daily schedule.
Requires: SU_USERNAME, SU_PASSWORD, MINERVA_API_URL, MINERVA_SERVICE_API_KEY.

Error policy: only `AmbiguousMatchError` is handled per-person (it's a
normal outcome for plain-text student-handledare entries that don't
resolve to a unique search hit). Everything else (ParseError,
NetworkError, AuthenticationError) propagates and crashes the run so
the failure is visible; the next day's cron starts fresh.
"""

import os
import sys
from datetime import date
from typing import Any

import requests
from dsv_wrapper import (
    AmbiguousMatchError,
    DaisyClient,
    Semester,
    TermSeason,
)


def get_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        print(f"error: {name} not set", file=sys.stderr)
        sys.exit(1)
    return value


def current_and_next_semesters(today: date) -> list[Semester]:
    """Rolling 2-semester window derived from today's calendar.

    VT covers Jan-Jun, HT covers Jul-Dec. The cutoff in July is a bit
    arbitrary (VT formally ends in early June, HT formally starts in
    late August) but by July teachers are already prepping HT, so
    treating July as HT-current keeps the imported set forward-looking.
    """
    year = today.year
    if today.month <= 6:
        return [
            Semester(year=year, season=TermSeason.VT),
            Semester(year=year, season=TermSeason.HT),
        ]
    return [
        Semester(year=year, season=TermSeason.HT),
        Semester(year=year + 1, season=TermSeason.VT),
    ]


# Realm suffixes whose eppns auth via Minerva's SU Shibboleth. Daisy
# stores its `usernames` field as full eppns already (`edwin@SU.SE`,
# `edwin@dsv.su.se`, occasionally `someone@kth.se`). We accept any
# `@*.su.se` realm and skip everything else, since KTH / LU / etc.
# eppns can't authenticate against SU's IdP. Anything without an `@`
# is also skipped; we never invent a realm we don't have evidence
# for.
_SU_REALM_SUFFIXES = (".su.se",)


def eppn_from_username(raw: str) -> str | None:
    """Translate a Daisy staff login into the Shibboleth eppn Minerva
    expects.

    Daisy stores usernames as full eppns already (`edwin@SU.SE`,
    `edwin@dsv.su.se`, occasionally `someone@kth.se`); we just
    lowercase and gate on the realm being inside SU's Shibboleth.
    Returns None for missing-@ inputs, foreign realms, or empty
    local-parts; the caller treats None as "skip this login" and
    the person's other usernames may still resolve.
    """
    raw = raw.strip().lower()
    if "@" not in raw:
        return None
    local, _, realm = raw.rpartition("@")
    if not local or not realm:
        return None
    # Allow `@su.se` and any DSV-style subdomain (`@dsv.su.se`).
    # Excludes other Swedish unis (`@kth.se`, `@lu.se`, ...).
    if realm != "su.se" and not realm.endswith(_SU_REALM_SUFFIXES):
        return None
    return raw


def resolve_participant(
    daisy: DaisyClient,
    cs: Any,
    cache: dict[str, dict],
) -> dict | None:
    """Resolve a `CourseStaff` into the JSON shape the Minerva service
    endpoint expects, or `None` if the name can't be pinned to a
    unique Daisy person.

    `AmbiguousMatchError` is the one expected failure: it fires from
    `cs.get_person_id` when a plain-text participant (student-
    handledare listed without a profile link) doesn't resolve to
    exactly one student-search hit. Employed staff arrive with
    `person_id` already set on the CourseStaff row, so they skip the
    search entirely; the username-uniqueness invariant means each
    staff `person_id` resolves to a single user.

    `cache` keys on Daisy `person_id` so a kursansvarig who teaches
    five courses doesn't trigger five identical profile-page fetches.
    """
    try:
        person_id = cs.get_person_id(daisy)
    except AmbiguousMatchError as e:
        print(f"    ! cannot resolve person_id for {cs.name!r}: {e}")
        return None

    # Same person on multiple courses reuses the resolved identity.
    # Roles vary per course; cs.roles is the source of truth, so we
    # don't cache them.
    if person_id in cache:
        return {**cache[person_id], "daisy_roles": list(cs.roles or [])}

    profile_url = cs.profile_url or ""
    is_student = "studentinfo" in profile_url

    eppns: list[str] = []
    if is_student:
        details = daisy.get_student_details(person_id)
        # Student profile carries a single `username` field rather
        # than the staff `usernames` list.
        username = getattr(details, "username", None)
        if username:
            eppn = eppn_from_username(username)
            if eppn:
                eppns.append(eppn)
    else:
        details = daisy.get_staff_details(person_id)
        for u in details.usernames or []:
            eppn = eppn_from_username(u)
            if eppn:
                eppns.append(eppn)

    if not eppns:
        # Profile parsed fine but no usernames are on file (rare;
        # typically accounts pending provisioning). Not an exception,
        # just nothing to push.
        print(f"    ! no usernames on file for {cs.name!r} (person_id={person_id})")
        return None

    # Dedup while preserving order (dsv-wrapper returns usernames
    # newest-first; we relay that as the canonical primary).
    seen: set[str] = set()
    deduped = [e for e in eppns if not (e in seen or seen.add(e))]

    identity = {
        "eppns": deduped,
        "display_name": cs.name,
        "person_id": person_id,
        "kind": "student" if is_student else "staff",
    }
    cache[person_id] = identity
    return {**identity, "daisy_roles": list(cs.roles or [])}


def build_course_payload(
    daisy: DaisyClient,
    course: Any,
    participant_cache: dict[str, dict],
) -> dict:
    """Per-course payload including detail-page enrichment (syllabus +
    unit) and the resolved participants list."""
    detail = daisy.get_course(course.momenttillf_id)
    roster = daisy.get_course_participants(course.momenttillf_id)

    participants_resolved: list[dict] = []
    for cs in roster:
        resolved = resolve_participant(daisy, cs, participant_cache)
        if resolved is not None:
            participants_resolved.append(resolved)

    return {
        "momenttillf_id": course.momenttillf_id,
        "beteckning": course.beteckning,
        "name": course.name,
        "semester_label": course.semester.label if course.semester else None,
        "info_url": course.info_url,
        "syllabus_url": detail.syllabus_url,
        "unit": detail.unit,
        "participants": participants_resolved,
    }


# Per-POST batch size. Picked to bound three things at once:
#   * runner memory: at any moment we hold at most CHUNK_SIZE
#     fully-resolved course payloads, not the union across both
#     semesters.
#   * backend memory + handler latency: one chunk is ~40 KB JSON
#     and ~250 SQL statements, finishing in a few seconds with a
#     single DB connection held the whole time.
#   * partial-failure granularity: a hard HTTP failure on one batch
#     stops the run loudly; earlier batches stay applied (the
#     backend upsert is idempotent so a next-day retry catches up).
# Today's DSV scale fits in ~7 batches; if the scope expands the
# constant scales linearly without code changes.
CHUNK_SIZE = 25


def iter_payloads(
    daisy: DaisyClient,
    semesters: list[Semester],
    participant_cache: dict[str, dict],
):
    """Yield per-course payloads as we walk Daisy. Lets the caller
    decide between batch-and-drain (normal path) or fully buffered
    (dry-run inspection) without us holding the whole product in
    memory ourselves."""
    for sem in semesters:
        courses = daisy.get_courses(sem)
        print(f"[{sem.label}] {len(courses)} course offerings")
        for course in courses:
            yield build_course_payload(daisy, course, participant_cache)


def post_batch(
    api_url: str,
    headers: dict,
    batch: list[dict],
) -> dict:
    """POST one chunk and raise on transport failure. Per-course
    errors inside the batch come back inside the summary's `errors`
    array; only HTTP-level failures (4xx/5xx/timeout) bubble up."""
    resp = requests.post(
        f"{api_url}/api/service/daisy-courses",
        headers=headers,
        json=batch,
        timeout=300,
    )
    resp.raise_for_status()
    return resp.json()


def merge_summary(into: dict, batch_summary: dict) -> None:
    """Sum counters across batches; concat per-course error strings.
    Mirrors `DaisyImportSummary` on the backend so the printed total
    matches what a single-shot POST would have returned."""
    for k in (
        "courses_received",
        "courses_created",
        "courses_updated",
        "members_added",
        "pending_memberships_added",
        "aliases_registered",
        "designations_created",
    ):
        into[k] = into.get(k, 0) + batch_summary.get(k, 0)
    into.setdefault("errors", []).extend(batch_summary.get("errors", []))


def main() -> None:
    api_url = get_env("MINERVA_API_URL").rstrip("/")
    api_key = get_env("MINERVA_SERVICE_API_KEY")
    su_username = get_env("SU_USERNAME")
    su_password = get_env("SU_PASSWORD")

    headers = {"Authorization": f"Bearer {api_key}"}
    today = date.today()
    semesters = current_and_next_semesters(today)
    print(
        f"Today: {today.isoformat()}; syncing semesters: "
        f"{[s.label for s in semesters]}"
    )

    # Per-run identity cache keyed by Daisy `person_id`. Shared
    # across batches so a kursansvarig who teaches courses in both
    # VT and HT only triggers one profile-page fetch per run.
    participant_cache: dict[str, dict] = {}
    dry_run = os.environ.get("DRY_RUN", "").lower() in ("1", "true", "yes")

    with DaisyClient(username=su_username, password=su_password) as daisy:
        if dry_run:
            # Dry-run is for human inspection; buffer everything so the
            # final stats + sample are computed against the full set.
            # At today's scale this is a few MB and the GH runner has
            # 7 GB; if scope explodes the streaming path below applies.
            all_payloads = list(iter_payloads(daisy, semesters, participant_cache))
            _emit_dry_run_summary(all_payloads)
            return

        # Streaming path. We POST each batch as it fills and drop it
        # so peak memory stays at ~CHUNK_SIZE payloads. The participant
        # cache and the accumulated summary are the only objects that
        # grow over the run; both stay tiny.
        total = {"errors": []}
        batch: list[dict] = []
        batches_sent = 0
        for payload in iter_payloads(daisy, semesters, participant_cache):
            batch.append(payload)
            if len(batch) >= CHUNK_SIZE:
                batch_summary = post_batch(api_url, headers, batch)
                merge_summary(total, batch_summary)
                batches_sent += 1
                print(
                    f"  batch {batches_sent}: {len(batch)} courses -> "
                    f"created={batch_summary.get('courses_created', 0)}, "
                    f"updated={batch_summary.get('courses_updated', 0)}, "
                    f"members+={batch_summary.get('members_added', 0)}"
                )
                batch = []

        if batch:
            batch_summary = post_batch(api_url, headers, batch)
            merge_summary(total, batch_summary)
            batches_sent += 1
            print(
                f"  batch {batches_sent}: {len(batch)} courses -> "
                f"created={batch_summary.get('courses_created', 0)}, "
                f"updated={batch_summary.get('courses_updated', 0)}, "
                f"members+={batch_summary.get('members_added', 0)}"
            )

    if batches_sent == 0:
        print("Nothing to push.")
        return
    print(f"Done across {batches_sent} batch(es): {total}")


def _emit_dry_run_summary(all_payloads: list[dict]) -> None:
    """Dump aggregate counters + one sample course as pretty JSON.
    Lets the workflow_dispatch dry-run verify dsv-wrapper compatibility
    end-to-end without poking the production API."""
    import json as _json

    if not all_payloads:
        print("DRY_RUN; nothing to push.")
        return
    total_participants = sum(len(p["participants"]) for p in all_payloads)
    kursansvarig = sum(
        1
        for p in all_payloads
        for cs in p["participants"]
        if any(
            r.startswith("Kurs-/delkursansvarig") or r.lower() == "kursansvarig"
            for r in cs["daisy_roles"]
        )
    )
    print("DRY_RUN; would POST the following:")
    print(f"  courses:                {len(all_payloads)}")
    print(f"  resolved participants:  {total_participants}")
    print(f"  kursansvarig entries:   {kursansvarig}")
    sample = next(
        (p for p in all_payloads if p["participants"]),
        all_payloads[0],
    )
    print("  sample course:")
    print(_json.dumps(sample, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
