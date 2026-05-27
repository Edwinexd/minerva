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


# Realm prefixes Daisy uses on staff `usernames`. Only the SU-side
# realms are safe to translate into Minerva eppns; KTH users can't auth
# via SU Shibboleth so suffixing their login with `@su.se` would just
# create a phantom user that never matches a real Shib session.
# Unknown / unprefixed usernames are skipped on the same principle:
# we never invent a realm we don't have evidence for.
_SU_REALMS = {"su", "dsv"}


def eppn_from_username(raw: str) -> str | None:
    """Translate a Daisy staff login (e.g. `dsv:edsu8469`) into the
    Shibboleth eppn Minerva expects (`edsu8469@su.se`).

    Returns None when the realm prefix is missing or names a realm we
    can't authenticate from Minerva's SU Shibboleth (e.g. `kth:foo`).
    The caller treats None as "skip this login"; the rest of the
    person's usernames may still resolve.
    """
    raw = raw.strip()
    if ":" not in raw:
        return None
    realm, _, bare = raw.partition(":")
    realm = realm.strip().lower()
    bare = bare.strip().lower()
    if not bare or realm not in _SU_REALMS:
        return None
    return f"{bare}@su.se"


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

    all_payloads: list[dict] = []
    # Per-run identity cache keyed by Daisy `person_id`.
    participant_cache: dict[str, dict] = {}

    with DaisyClient(username=su_username, password=su_password) as daisy:
        for sem in semesters:
            courses = daisy.get_courses(sem)
            print(f"[{sem.label}] {len(courses)} course offerings")
            for course in courses:
                payload = build_course_payload(daisy, course, participant_cache)
                all_payloads.append(payload)

    if not all_payloads:
        print("Nothing to push.")
        return

    # Dry-run knob used by the workflow_dispatch path before the
    # backend endpoint is rolled to prod. We collect the same payload
    # we'd POST, dump a short stats summary + a single sample
    # course's JSON to stdout, and exit. Lets you verify dsv-wrapper
    # compatibility (semester walk, participant resolution, realm
    # mapping) end-to-end without poking the production API.
    if os.environ.get("DRY_RUN", "").lower() in ("1", "true", "yes"):
        import json as _json

        total_participants = sum(len(p["participants"]) for p in all_payloads)
        kursansvarig = sum(
            1
            for p in all_payloads
            for cs in p["participants"]
            if any(
                r.startswith("Kurs-/delkursansvarig")
                or r.lower() == "kursansvarig"
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
        return

    print(f"Posting {len(all_payloads)} courses to Minerva...")
    resp = requests.post(
        f"{api_url}/api/service/daisy-courses",
        headers=headers,
        json=all_payloads,
        timeout=300,
    )
    resp.raise_for_status()
    summary = resp.json()
    print(f"Done: {summary}")


if __name__ == "__main__":
    main()
