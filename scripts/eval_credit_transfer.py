"""Measure the credit transfer comparison against cases whose answer is known.

No staff labels are needed: every case is a real DSV course plan turned
into an "external" syllabus, so the plan it came from is the right match
and its outcomes are the right rows.

    load-plans  (dev only) put real DSV plans into a local backend's plan
            archive, the way the nightly Daisy sync does, so the dev
            scheduler downloads, parses and indexes them
    build   pick plans from a credit transfer workspace, download their
            PDFs from SU and write the cases (one directory each)
    run     send every case through the workspace's chat N times and
            save each reply (assessment, tool events, tokens) as JSONL
    score   read the JSONL and print the metrics per variant

Variants (each from one plan, its codes, name and department removed so
the search cannot match on them):

    self     the Swedish plan as is: the plan should be found and compared
             with full overlap, every outcome covered
    self_en  the English translation, when SU has one: the same, across
             languages
    drop     about 40% of the outcomes removed: overlap partial, the
             removed outcomes gap and the rest covered
    decoy    three outcomes of an unrelated plan added: still full overlap

Environment:

    MINERVA_EVAL_URL         backend base URL (default http://localhost:3000)
    MINERVA_EVAL_WORKSPACE   id of the credit transfer workspace
    MINERVA_EVAL_USER        user to act as (default: the backend's choice)
    MINERVA_EVAL_AUTH_HEADER header carrying it (default X-Dev-User, the
                             dev-mode login)

`load-plans` also needs MINERVA_SERVICE_API_KEY, the key the local backend
was started with. Its default code list, `eval_credit_transfer_codes.txt`,
is the DSV plans prod mirrored in Nov 2026.

Run from the repo root: `venv/bin/python scripts/eval_credit_transfer.py
build --out eval-runs/cases`. Needs `pdftotext` (poppler) on PATH, the
same extractor the server uses, so cited pages line up.
"""

import argparse
import concurrent.futures
import json
import os
import random
import re
import statistics
import subprocess
import sys
import tempfile
import time
from collections import Counter, defaultdict
from pathlib import Path

import requests

DEFAULT_NOTE = "Jämför den bifogade kursplanen med DSV:s kurser."

OUTCOME_START = {
    "sv": ("förväntade studieresultat",),
    "en": (
        "expected learning outcomes",
        "intended learning outcomes",
        "learning outcomes",
    ),
}
OUTCOME_END = {
    "sv": (
        "undervisning",
        "kunskapskontroll och examination",
        "kurslitteratur",
        "övergångsbestämmelser",
    ),
    "en": (
        "education",
        "teaching",
        "forms of examination",
        "examination",
        "required reading",
    ),
}
FOOTER = re.compile(r"^\s{20,}\S|^\s*(sidan|page)\s+\d+/\d+\s*$", re.IGNORECASE)
BULLET = re.compile(r"^\s*(?:[-•–*]|\d+[.)])\s+")
# DSV and SU course codes (IB133N, ML751N) and Daisy designations (PROG1).
COURSE_CODE = re.compile(r"\b[A-Z]{2}\d{3}[A-Z0-9]\b")
INSTITUTION = [
    ("Institutionen för data- och systemvetenskap", "Institutionen för informatik"),
    ("Department of Computer and Systems Sciences", "Department of Informatics"),
    ("Stockholms universitet", "Exempeluniversitetet"),
    ("Stockholm University", "Example University"),
]

DROP_FRACTION = 0.4
DECOY_OUTCOMES = 3
MATCH_THRESHOLD = 0.6


# Plumbing


def env(name: str, default: str | None = None) -> str:
    value = os.environ.get(name, default)
    if not value:
        sys.exit(f"error: {name} not set")
    return value


class Api:
    def __init__(self) -> None:
        self.base = env("MINERVA_EVAL_URL", "http://localhost:3000").rstrip("/")
        self.workspace = env("MINERVA_EVAL_WORKSPACE")
        self.session = requests.Session()
        user = os.environ.get("MINERVA_EVAL_USER")
        if user:
            header = os.environ.get("MINERVA_EVAL_AUTH_HEADER", "X-Dev-User")
            self.session.headers[header] = user

    def url(self, path: str) -> str:
        return f"{self.base}/api/workspaces/{self.workspace}{path}"

    def get(self, path: str, **kw):
        r = self.session.get(self.url(path), timeout=60, **kw)
        r.raise_for_status()
        return r.json()

    def workspace_config(self) -> dict:
        r = self.session.get(f"{self.base}/api/workspaces/{self.workspace}", timeout=60)
        r.raise_for_status()
        return r.json()


def pdf_pages(pdf: bytes) -> list[str]:
    """Pages as the server reads them (`minerva_pipeline::pdf`)."""
    with tempfile.NamedTemporaryFile(suffix=".pdf") as f:
        f.write(pdf)
        f.flush()
        out = subprocess.run(
            ["pdftotext", "-layout", f.name, "-"],
            check=True,
            capture_output=True,
        ).stdout.decode("utf-8", "replace")
    pages = out.split("\f")
    if pages and not pages[-1].strip():
        pages.pop()
    return pages


def mark_pages(pages: list[str]) -> str:
    return "\n\n".join(f"[Page {i + 1}]\n{p.rstrip()}" for i, p in enumerate(pages))


def words(text: str) -> list[str]:
    return [w for w in re.findall(r"\w+", text.lower()) if len(w) > 2]


def containment(needle: str, haystack: str) -> float:
    """Share of `needle`'s words found in `haystack`."""
    n = set(words(needle))
    if not n:
        return 0.0
    return len(n & set(words(haystack))) / len(n)


# Outcomes


def find_outcomes(pages: list[str], language: str) -> list[dict]:
    """The plan's intended learning outcomes, each with the lines it spans
    as `(page, line)` pairs, so a variant can remove it or add after it."""
    lines = [
        (p, i, line)
        for p, page in enumerate(pages)
        for i, line in enumerate(page.split("\n"))
    ]
    start = next(
        (
            k
            for k, (_, _, line) in enumerate(lines)
            if line.strip().lower() in OUTCOME_START[language]
        ),
        None,
    )
    if start is None:
        return []
    items: list[dict] = []
    for p, i, line in lines[start + 1 :]:
        stripped = line.strip()
        if stripped.lower() in OUTCOME_END[language]:
            break
        if not stripped or FOOTER.match(line):
            continue
        if BULLET.match(line):
            items.append({"text": BULLET.sub("", line).strip(), "lines": [(p, i)]})
        elif items and not stripped.endswith(":"):
            items[-1]["text"] += " " + stripped
            items[-1]["lines"].append((p, i))
    return items


def edit_pages(
    pages: list[str], drop: set[tuple[int, int]], insert_after=None, inserted=()
) -> list[str]:
    out = []
    for p, page in enumerate(pages):
        kept = []
        for i, line in enumerate(page.split("\n")):
            if (p, i) not in drop:
                kept.append(line)
            if insert_after == (p, i):
                kept.extend(f"- {t}" for t in inserted)
        out.append("\n".join(kept))
    return out


def anonymize(text: str, names: list[str]) -> str:
    for name in sorted({n for n in names if n}, key=len, reverse=True):
        text = re.sub(re.escape(name), "Kursen", text, flags=re.IGNORECASE)
    for old, new in INSTITUTION:
        text = text.replace(old, new)
    return COURSE_CODE.sub("XX000X", text)


# load-plans


def load_plans(args) -> None:
    sys.path.insert(0, str(Path(__file__).parent))
    from dsv_wrapper import SyllabusClient, SyllabusNotFoundError
    from sync_daisy_courses import build_syllabus_payload

    base = env("MINERVA_EVAL_URL", "http://localhost:3000").rstrip("/")
    session = requests.Session()
    session.headers["Authorization"] = f"Bearer {env('MINERVA_SERVICE_API_KEY')}"
    codes = args.codes or Path(args.codes_file).read_text(encoding="utf-8").split()
    pushed = 0
    with SyllabusClient() as archive:
        for code in sorted(set(codes)):
            try:
                sv = archive.get_syllabus_archive(code, language="sv")
                en = archive.get_syllabus_archive(code, language="en")
            except SyllabusNotFoundError:
                print(f"skip {code}: not in SU's archive", file=sys.stderr)
                continue
            r = session.put(
                f"{base}/api/service/syllabus-plans/{code}",
                json=build_syllabus_payload(sv, en, {}),
                timeout=60,
            )
            r.raise_for_status()
            pushed += 1
    print(f"{pushed} plans pushed; the scheduler downloads and indexes them")


# build


def build(args) -> None:
    api = Api()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    plans = [p for p in api.get("/credit-transfer/plans") if p["status"] == "indexed"]
    if args.codes:
        wanted = set(args.codes)
        plans = [p for p in plans if p["code"] in wanted]
    rng = random.Random(args.seed)
    rng.shuffle(plans)

    loaded = []
    for plan in plans:
        if len(loaded) >= args.limit:
            break
        files = {f["language"]: f["pdf_url"] for f in plan["latest"]["files"]}
        if "sv" not in files:
            continue
        try:
            sv = pdf_pages(requests.get(files["sv"], timeout=60).content)
        except (requests.RequestException, subprocess.CalledProcessError) as e:
            print(f"skip {plan['code']}: {e}", file=sys.stderr)
            continue
        outcomes = find_outcomes(sv, "sv")
        if len(outcomes) < 2:
            print(f"skip {plan['code']}: {len(outcomes)} outcomes parsed", file=sys.stderr)
            continue
        en = None
        if "en" in files:
            try:
                en = pdf_pages(requests.get(files["en"], timeout=60).content)
            except (requests.RequestException, subprocess.CalledProcessError):
                en = None
        loaded.append({"plan": plan, "sv": sv, "en": en, "outcomes": outcomes})
        print(f"plan {plan['code']}: {len(outcomes)} outcomes", file=sys.stderr)

    count = 0
    for k, entry in enumerate(loaded):
        plan, sv, outcomes = entry["plan"], entry["sv"], entry["outcomes"]
        names = [plan["name_sv"], plan.get("name_en") or ""]
        base = {
            "target": plan["code"],
            "target_name": plan["name_sv"],
            "plan_version": plan["latest"]["valid_from"],
            "dsv_pages": sv,
            "outcomes": [o["text"] for o in outcomes],
            "note": args.note,
        }

        def write(variant: str, pages: list[str], **extra) -> None:
            nonlocal count
            case_id = f"{plan['code']}-{variant}"
            text = anonymize(mark_pages(pages), names)
            d = out / case_id
            d.mkdir(exist_ok=True)
            (d / "syllabus.txt").write_text(text, encoding="utf-8")
            case = {**base, "id": case_id, "variant": variant, **extra}
            (d / "case.json").write_text(
                json.dumps(case, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
            )
            count += 1

        write("self", sv, expected_verdict="full", dropped=[])

        if entry["en"] and len(find_outcomes(entry["en"], "en")) >= 2:
            write("self_en", entry["en"], expected_verdict="full", dropped=[])

        n_drop = max(1, min(len(outcomes) - 1, round(len(outcomes) * DROP_FRACTION)))
        dropped = rng.sample(range(len(outcomes)), n_drop)
        lines = {ln for i in dropped for ln in outcomes[i]["lines"]}
        write(
            "drop",
            edit_pages(sv, lines),
            expected_verdict="partial",
            dropped=[outcomes[i]["text"] for i in sorted(dropped)],
        )

        others = [
            e
            for e in loaded
            if e is not entry and e["plan"]["code"][:2] != plan["code"][:2]
        ] or [e for e in loaded if e is not entry]
        if others:
            donor = others[k % len(others)]
            decoys = [o["text"] for o in donor["outcomes"][:DECOY_OUTCOMES]]
            write(
                "decoy",
                edit_pages(sv, set(), outcomes[-1]["lines"][-1], decoys),
                expected_verdict="full",
                dropped=[],
                decoy_from=donor["plan"]["code"],
            )
    print(f"{count} cases from {len(loaded)} plans in {out}")


# run


def send(api: Api, case_dir: Path, case: dict) -> dict:
    started = time.monotonic()
    syllabus = (case_dir / "syllabus.txt").read_bytes()
    r = api.session.post(
        api.url("/conversations/attachments"),
        files={"file": ("kursplan.txt", syllabus, "text/plain")},
        timeout=120,
    )
    r.raise_for_status()
    cid = r.json()["conversation_id"]
    error = None
    with api.session.post(
        api.url(f"/conversations/{cid}/message"),
        json={"content": case["note"]},
        stream=True,
        timeout=600,
    ) as stream:
        stream.raise_for_status()
        for raw in stream.iter_lines(decode_unicode=True):
            if not raw or not raw.startswith("data:"):
                continue
            try:
                event = json.loads(raw[5:].strip())
            except json.JSONDecodeError:
                continue
            if event.get("type") == "error":
                error = event.get("error")
            if event.get("type") in ("done", "error"):
                break
    detail = api.get(f"/conversations/{cid}")
    reply = next(
        (m for m in reversed(detail["messages"]) if m["role"] == "assistant"), None
    )
    return {
        "conversation_id": cid,
        "seconds": round(time.monotonic() - started, 1),
        "error": error if reply is None or error else None,
        "assessment": reply and reply.get("assessment"),
        "content": reply and reply["content"],
        "tool_events": (reply and reply.get("tool_events")) or [],
        "model": reply and reply.get("model_used"),
        "tokens_prompt": reply and reply.get("tokens_prompt"),
        "tokens_completion": reply and reply.get("tokens_completion"),
    }


def run(args) -> None:
    api = Api()
    cases = sorted(Path(args.cases).glob("*/case.json"))
    if args.only:
        cases = [c for c in cases if any(o in c.parent.name for o in args.only)]
    config = api.workspace_config()
    run_id = time.strftime("%Y%m%d-%H%M%S")
    jobs = [(c, n) for c in cases for n in range(args.runs)]
    print(f"{len(jobs)} turns ({len(cases)} cases x {args.runs})", file=sys.stderr)

    def one(job):
        path, n = job
        case = json.loads(path.read_text(encoding="utf-8"))
        try:
            result = send(api, path.parent, case)
        except requests.RequestException as e:
            result = {"error": str(e)}
        return {
            "run_id": run_id,
            "case": case["id"],
            "repeat": n,
            "workspace": {
                k: config.get(k)
                for k in ("model", "strategy", "tool_use_enabled", "embedding_model")
            },
            **result,
        }

    with open(args.out, "a", encoding="utf-8") as out, concurrent.futures.ThreadPoolExecutor(
        args.parallel
    ) as pool:
        for i, row in enumerate(pool.map(one, jobs), 1):
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
            out.flush()
            status = row.get("error") or ("ok" if row.get("assessment") else "no comparison")
            print(f"[{i}/{len(jobs)}] {row['case']} #{row['repeat']}: {status}", file=sys.stderr)


# score


def corpus_codes(events: list[dict]) -> tuple[list[str], list[str], int]:
    """Plans the opening search seeded, plans `get_plan` added, and how
    many `search_plans` calls the research phase made."""
    seeded, added, searches = [], [], 0
    for e in events:
        result = e.get("result")
        if e["name"] == "plan_search" and isinstance(result, list):
            seeded = [p["code"] for p in result]
        elif e["name"] == "get_plan" and isinstance(result, dict):
            added += result.get("added", [])
        elif e["name"] == "search_plans":
            searches += 1
    return seeded, added, searches


def page_text(text: str, page: int) -> str | None:
    """Page `page` of a `[Page N]`-marked text."""
    parts = re.split(r"\[Page (\d+)\]\n", text)
    for k in range(1, len(parts) - 1, 2):
        if int(parts[k]) == page:
            return parts[k + 1]
    return None


def best_outcome(row_text: str, outcomes: list[str]) -> int | None:
    scored = [(containment(o, row_text) + containment(row_text, o), i) for i, o in enumerate(outcomes)]
    score, i = max(scored, default=(0, None))
    return i if score / 2 >= MATCH_THRESHOLD else None


def score_row(row: dict, case: dict, syllabus: str) -> dict:
    a = row.get("assessment")
    seeded, added, searches = corpus_codes(row.get("tool_events", []))
    target = case["target"]
    s = {
        "case": case["id"],
        "variant": case["variant"],
        "target": target,
        "failed": bool(row.get("error")) or not row.get("content"),
        "comparison": a is not None,
        "in_seed": target in seeded,
        "in_corpus": target in seeded or target in added,
        "plans_added": len(added),
        "plan_searches": searches,
        "tokens": (row.get("tokens_prompt") or 0) + (row.get("tokens_completion") or 0),
        "seconds": row.get("seconds"),
    }
    if not a:
        return s
    codes = [c["code"] for c in a["candidates"]]
    s["compared"] = target in codes
    s["rank"] = codes.index(target) + 1 if target in codes else None
    s["ruled_out_target"] = target in [r["code"] for r in a["ruled_out"]]
    s["other_full"] = sum(1 for c in a["candidates"] if c["code"] != target and c["verdict"] == "full")
    s["compared_set"] = sorted(codes)
    cand = next((c for c in a["candidates"] if c["code"] == target), None)
    if not cand:
        return s
    s["verdict"] = cand["verdict"]
    s["verdict_ok"] = cand["verdict"] == case["expected_verdict"]

    outcomes, dropped = case["outcomes"], set(case["dropped"])
    hit, labels, exact, binary = set(), 0, 0, 0
    cites = {"dsv_ok": 0, "dsv_checked": 0, "ext_ok": 0, "ext_checked": 0, "no_page": 0}
    for o in cand["outcomes"]:
        i = best_outcome(o["dsv_outcome"], outcomes)
        if i is not None:
            hit.add(i)
            want = "gap" if outcomes[i] in dropped else "covered"
            labels += 1
            exact += o["coverage"] == want
            binary += (o["coverage"] == "covered") == (want == "covered")
        if o.get("dsv_page"):
            page = case["dsv_pages"][o["dsv_page"] - 1] if o["dsv_page"] <= len(case["dsv_pages"]) else ""
            cites["dsv_checked"] += 1
            cites["dsv_ok"] += containment(o["dsv_outcome"], page) >= MATCH_THRESHOLD
        else:
            cites["no_page"] += 1
        if o.get("external_outcome") and o.get("external_page"):
            page = page_text(syllabus, o["external_page"]) or ""
            cites["ext_checked"] += 1
            cites["ext_ok"] += containment(o["external_outcome"], page) >= MATCH_THRESHOLD
    s["rows"] = len(cand["outcomes"])
    s["outcome_recall"] = len(hit) / len(outcomes)
    s["unmatched_rows"] = len(cand["outcomes"]) - sum(
        1 for o in cand["outcomes"] if best_outcome(o["dsv_outcome"], outcomes) is not None
    )
    s["coverage_exact"] = exact / labels if labels else None
    s["coverage_binary"] = binary / labels if labels else None
    s.update(cites)
    return s


def mean(values) -> str:
    vals = [float(v) for v in values if v is not None]
    return f"{statistics.mean(vals):.2f}" if vals else "-"


def rate(rows, key) -> str:
    vals = [r[key] for r in rows if key in r and r[key] is not None]
    return f"{sum(map(bool, vals))}/{len(vals)}" if vals else "-"


def score(args) -> None:
    cases = {
        json.loads(p.read_text(encoding="utf-8"))["id"]: p.parent
        for p in Path(args.cases).glob("*/case.json")
    }
    rows = [json.loads(line) for line in open(args.results, encoding="utf-8") if line.strip()]
    if args.run_id:
        rows = [r for r in rows if r["run_id"] in args.run_id]
    scored = []
    for row in rows:
        d = cases.get(row["case"])
        if d is None:
            continue
        case = json.loads((d / "case.json").read_text(encoding="utf-8"))
        scored.append(score_row(row, case, (d / "syllabus.txt").read_text(encoding="utf-8")))
    if not scored:
        sys.exit("no scored rows")

    by_variant = defaultdict(list)
    for s in scored:
        by_variant[s["variant"]].append(s)
    groups = [(v, by_variant[v]) for v in ("self", "self_en", "drop", "decoy") if v in by_variant]
    groups.append(("all", scored))

    metrics = [
        ("turns", lambda g: str(len(g))),
        ("failed", lambda g: rate(g, "failed")),
        ("chose comparison", lambda g: rate(g, "comparison")),
        ("target in opening search", lambda g: rate(g, "in_seed")),
        ("target in plans after research", lambda g: rate(g, "in_corpus")),
        ("target compared", lambda g: rate(g, "compared")),
        ("target ranked first", lambda g: f"{sum(1 for s in g if s.get('rank') == 1)}/{len(g)}"),
        ("target ruled out", lambda g: rate(g, "ruled_out_target")),
        ("verdict as expected", lambda g: rate(g, "verdict_ok")),
        ("other plans called full (mean)", lambda g: mean(s.get("other_full") for s in g)),
        ("outcome rows found (recall)", lambda g: mean(s.get("outcome_recall") for s in g)),
        ("rows matching no outcome (mean)", lambda g: mean(s.get("unmatched_rows") for s in g)),
        ("coverage label exact", lambda g: mean(s.get("coverage_exact") for s in g)),
        ("coverage covered/not right", lambda g: mean(s.get("coverage_binary") for s in g)),
        (
            "DSV page citations right",
            lambda g: f"{sum(s.get('dsv_ok', 0) for s in g)}/{sum(s.get('dsv_checked', 0) for s in g)}",
        ),
        (
            "external page citations right",
            lambda g: f"{sum(s.get('ext_ok', 0) for s in g)}/{sum(s.get('ext_checked', 0) for s in g)}",
        ),
        ("rows without DSV page", lambda g: str(sum(s.get("no_page", 0) for s in g))),
        ("plans added by research (mean)", lambda g: mean(s["plans_added"] for s in g)),
        ("search_plans calls (mean)", lambda g: mean(s["plan_searches"] for s in g)),
        ("tokens per turn (mean)", lambda g: mean(s["tokens"] for s in g)),
        ("seconds per turn (mean)", lambda g: mean(s["seconds"] for s in g)),
    ]
    header = "| metric | " + " | ".join(name for name, _ in groups) + " |"
    print(header)
    print("|" + "---|" * (len(groups) + 1))
    for label, fn in metrics:
        print(f"| {label} | " + " | ".join(fn(g) for _, g in groups) + " |")

    by_case = defaultdict(list)
    for s in scored:
        by_case[s["case"]].append(s)
    repeated = {c: g for c, g in by_case.items() if len(g) > 1}
    if repeated:
        agree, sets = [], []
        for g in repeated.values():
            verdicts = Counter(s.get("verdict") for s in g)
            agree.append(verdicts.most_common(1)[0][1] / len(g))
            compared = [frozenset(s.get("compared_set", [])) for s in g]
            sets.append(len(set(compared)) == 1)
        print()
        print(f"Stability over {len(repeated)} repeated cases:")
        print(f"- runs agreeing with the case's most common verdict: {statistics.mean(agree):.2f}")
        print(f"- cases comparing the same plans every run: {sum(sets)}/{len(sets)}")

    misses = [s for s in scored if s.get("comparison") and not s.get("compared")]
    if misses:
        print()
        print("Target not compared:")
        for s in misses:
            where = "ruled out" if s.get("ruled_out_target") else (
                "in plans, not compared" if s["in_corpus"] else "never found"
            )
            print(f"- {s['case']}: {where}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)

    lp = sub.add_parser("load-plans", help="put real DSV plans into a dev backend")
    lp.add_argument("--codes", nargs="*", help="plan codes (default: --codes-file)")
    lp.add_argument(
        "--codes-file",
        default=str(Path(__file__).with_name("eval_credit_transfer_codes.txt")),
    )
    lp.set_defaults(func=load_plans)

    b = sub.add_parser("build", help="write cases from the workspace's plans")
    b.add_argument("--out", required=True, help="directory for the cases")
    b.add_argument("--limit", type=int, default=20, help="plans to use")
    b.add_argument("--codes", nargs="*", help="use these plan codes only")
    b.add_argument("--seed", type=int, default=1, help="sampling seed")
    b.add_argument("--note", default=DEFAULT_NOTE, help="the staff note sent with each case")
    b.set_defaults(func=build)

    r = sub.add_parser("run", help="send the cases through the chat")
    r.add_argument("--cases", required=True)
    r.add_argument("--out", required=True, help="JSONL file to append to")
    r.add_argument("--runs", type=int, default=1, help="repeats per case")
    r.add_argument("--parallel", type=int, default=2)
    r.add_argument("--only", nargs="*", help="case ids containing any of these")
    r.set_defaults(func=run)

    s = sub.add_parser("score", help="print the metrics")
    s.add_argument("--cases", required=True)
    s.add_argument("--results", required=True)
    s.add_argument("--run-id", nargs="*", help="score these runs only")
    s.set_defaults(func=score)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
