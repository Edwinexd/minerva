"""
Regenerates the architecture / pipeline figures used in docs/ARCHITECTURE.md.

Uses raw graphviz (via the `graphviz` Python package) so we get full control
over node layout: compact rows of "small icon + bold name + light subtitle"
instead of the diagrams library's default oversized-icon-on-top look.

Requires: graphviz binary + the `graphviz` Python package + the `diagrams`
package (we vendor a few icons from its bundled resources).

  sudo apt-get install graphviz
  python3 -m venv /tmp/diags
  /tmp/diags/bin/pip install graphviz diagrams
  /tmp/diags/bin/python docs/diagrams/build.py
"""

from __future__ import annotations

import base64
import mimetypes
import re
import shutil
from pathlib import Path

import graphviz

OUT = Path(__file__).resolve().parent
ASSETS = OUT / "_assets"
ASSETS.mkdir(exist_ok=True)

# --- icon resolution ---------------------------------------------------------
# Pull the few PNGs we use from the diagrams package (so we don't recommit
# them) and keep the HF asset that already lives in _assets/.
_DIAGRAMS_RES = Path(
    "/tmp/diags/lib/python3.13/site-packages/resources"
).resolve()


def _resolve_diagrams_icon(rel: str) -> Path:
    return _DIAGRAMS_RES / rel


ICONS = {
    "rust": _resolve_diagrams_icon("programming/language/rust.png"),
    "react": _resolve_diagrams_icon("programming/framework/react.png"),
    "python": _resolve_diagrams_icon("programming/language/python.png"),
    "postgres": _resolve_diagrams_icon("onprem/database/postgresql.png"),
    "qdrant": _resolve_diagrams_icon("onprem/database/qdrant.png"),
    "apache": _resolve_diagrams_icon("onprem/network/apache.png"),
    "github": _resolve_diagrams_icon("onprem/vcs/github.png"),
    "huggingface": ASSETS / "huggingface.png",
}

# All resolved at script start; missing assets become "no icon".
ICONS = {k: v for k, v in ICONS.items() if v.exists()}


# --- styling -----------------------------------------------------------------
GRAPH_BASE = {
    "fontname": "Helvetica",
    "fontsize": "13",
    "bgcolor": "white",
    "pad": "0.4",
    "splines": "spline",
    "nodesep": "0.4",
    "ranksep": "0.55",
    "rankdir": "TB",
}
NODE_BASE = {
    "fontname": "Helvetica",
    "fontsize": "11",
    "shape": "plain",
    "margin": "0",
}
EDGE_BASE = {
    "fontname": "Helvetica",
    "fontsize": "9",
    "color": "#666666",
    "arrowsize": "0.7",
}
CLUSTER_BASE = {
    "fontname": "Helvetica-Bold",
    "fontsize": "10",
    "labelloc": "t",
    "labeljust": "l",
    "bgcolor": "#fafafa",
    "color": "#9aa0a6",
    "style": "rounded,dashed",
    "margin": "10",
}
GUARD = "#cc4444"


def _new_graph(name: str, rankdir: str = "TB", **extra) -> graphviz.Digraph:
    g = graphviz.Digraph(name, format="svg")
    g.attr(**{**GRAPH_BASE, "rankdir": rankdir, **extra})
    g.attr("node", **NODE_BASE)
    g.attr("edge", **EDGE_BASE)
    return g


def _box(
    g: graphviz.Digraph,
    nid: str,
    title: str,
    *,
    icon: str | None = None,
    subtitle: str | None = None,
    fill: str = "#ffffff",
    stroke: str = "#444444",
    shape: str = "box",
    bold: bool = True,
) -> None:
    """Render a node as an HTML-label table: small icon + name (+ subtitle)."""

    img_cell = ""
    if icon and icon in ICONS:
        img_cell = (
            f'<TD WIDTH="22" HEIGHT="22" FIXEDSIZE="TRUE">'
            f'<IMG SRC="{ICONS[icon]}" SCALE="TRUE"/></TD>'
        )

    title_html = f"<B>{title}</B>" if bold else title
    sub_html = (
        f'<BR/><FONT COLOR="#666" POINT-SIZE="9">{subtitle}</FONT>'
        if subtitle
        else ""
    )

    if shape == "diamond":
        # rhombus approximated via a single rounded cell with diagonal corners.
        outer = (
            f'<TABLE BORDER="1" CELLBORDER="0" CELLSPACING="0" CELLPADDING="6"'
            f' BGCOLOR="#fff7e6" COLOR="#a78a3d" STYLE="ROUNDED">'
            f"<TR>{img_cell}<TD ALIGN='LEFT'>{title_html}{sub_html}</TD></TR>"
            f"</TABLE>"
        )
    elif shape == "stadium":
        outer = (
            f'<TABLE BORDER="2" CELLBORDER="0" CELLSPACING="0" CELLPADDING="8"'
            f' BGCOLOR="{fill}" COLOR="{stroke}" STYLE="ROUNDED">'
            f"<TR>{img_cell}<TD ALIGN='LEFT'>{title_html}{sub_html}</TD></TR>"
            f"</TABLE>"
        )
    else:
        outer = (
            f'<TABLE BORDER="1" CELLBORDER="0" CELLSPACING="0" CELLPADDING="6"'
            f' BGCOLOR="{fill}" COLOR="{stroke}" STYLE="ROUNDED">'
            f"<TR>{img_cell}<TD ALIGN='LEFT'>{title_html}{sub_html}</TD></TR>"
            f"</TABLE>"
        )
    g.node(nid, label=f"<{outer}>")


def _cluster(parent: graphviz.Digraph, cid: str, label: str) -> graphviz.Digraph:
    sg = parent.subgraph(name=f"cluster_{cid}")
    return sg


def _add_cluster(
    parent: graphviz.Digraph, cid: str, label: str
) -> graphviz.Digraph:
    sub = graphviz.Digraph(name=f"cluster_{cid}")
    sub.attr(label=label, **CLUSTER_BASE)
    return sub


# --- 1. System overview ------------------------------------------------------
def system_overview() -> graphviz.Digraph:
    g = _new_graph("system", rankdir="LR", ranksep="0.7", nodesep="0.3")

    with g.subgraph(name="cluster_clients") as c:
        c.attr(label="Clients", **CLUSTER_BASE)
        _box(c, "web", "Web SPA", icon="react", subtitle="React + TanStack")
        _box(c, "embed", "Iframe embed")
        _box(c, "moodle", "Moodle plugin", subtitle="local_minerva")
        _box(c, "lti", "LTI 1.3 platform")

    with g.subgraph(name="cluster_edge") as c:
        c.attr(label="Apache edge", **CLUSTER_BASE)
        _box(c, "shib", "mod_shib", icon="apache", subtitle="Shibboleth SSO")
        _box(c, "lua", "mod_lua", icon="apache", subtitle="external-auth invites")

    with g.subgraph(name="cluster_app") as c:
        c.attr(label="minerva-app", **CLUSTER_BASE)
        _box(c, "api", "axum HTTP API", icon="rust")
        _box(c, "worker", "ingest worker")
        _box(c, "kg", "KG linker", subtitle="debounced sweeper")
        _box(c, "cron", "Schedulers", subtitle="Canvas + transcripts")

    with g.subgraph(name="cluster_state") as c:
        c.attr(label="Stateful", **CLUSTER_BASE)
        _box(c, "pg", "PostgreSQL 16", icon="postgres")
        _box(c, "qd", "Qdrant", icon="qdrant", subtitle="per-course collections")
        _box(c, "docs", "/data0/minerva/data", subtitle="document blobs")
        _box(c, "hf", "fastembed cache", icon="huggingface")

    with g.subgraph(name="cluster_ai") as c:
        c.attr(label="AI providers", **CLUSTER_BASE)
        _box(c, "llm", "Cerebras", subtitle="OpenAI-compatible LLM")
        _box(c, "oai", "OpenAI", subtitle="embeddings")

    with g.subgraph(name="cluster_ext") as c:
        c.attr(label="External content", **CLUSTER_BASE)
        _box(c, "canvas", "Canvas LMS", subtitle="REST sync")
        _box(c, "play", "play.dsv.su.se", subtitle="VTT transcripts")

    # Client -> edge / app
    g.edge("web", "shib")
    g.edge("embed", "api")
    g.edge("moodle", "api")
    g.edge("lti", "api")
    g.edge("shib", "api")
    g.edge("lua", "api")

    # API <-> stateful (read/write).
    g.edge("api", "pg", dir="both")
    g.edge("api", "qd", dir="both")
    g.edge("api", "docs", dir="both")

    # API <-> LLM (request + SSE response).
    g.edge("api", "llm", dir="both", label="chat")

    # Worker writes embeddings + caches models.
    g.edge("worker", "qd")
    g.edge("worker", "hf", dir="both")
    g.edge("worker", "oai", label="embed")

    # KG linker reads from Qdrant + asks LLM.
    g.edge("kg", "qd", dir="both")
    g.edge("kg", "llm", dir="both", label="link / classify")

    # Schedulers pull from external content (request + response).
    g.edge("cron", "canvas", dir="both", label="pull")
    g.edge("cron", "play", dir="both", label="pull")

    return g


# --- 2. Document ingest ------------------------------------------------------
def ingest_pipeline() -> graphviz.Digraph:
    g = _new_graph("ingest", rankdir="TB")

    with g.subgraph(name="cluster_sources") as c:
        c.attr(label="Sources", **CLUSTER_BASE)
        _box(c, "upload", "Direct upload")
        _box(c, "ms", "Moodle / Canvas sync")
        _box(c, "mbz", "MBZ import")
        _box(c, "play_url", "play.dsv URL drop")

    _box(g, "queue", "documents", icon="postgres", subtitle="status = pending")

    with g.subgraph(name="cluster_worker") as c:
        c.attr(label="Ingest worker", **CLUSTER_BASE)
        _box(c, "gate", "mime / source router", shape="diamond", bold=False)
        _box(c, "extract", "poppler / extractor")
        _box(c, "classify", "kind classifier", subtitle="llama3.1-8b")
        _box(c, "chunk", "chunker")
        _box(c, "embed", "embedder", subtitle="OpenAI or fastembed")
        _box(c, "kg", "KG linker", subtitle="cross-doc edges")

    with g.subgraph(name="cluster_cron") as c:
        c.attr(label="Hourly transcript fetch", **CLUSTER_BASE)
        _box(c, "awaiting", "awaiting_transcript", subtitle="queue")
        _box(c, "ghx", "transcripts.yml", icon="github", subtitle="GitHub Actions")

    with g.subgraph(name="cluster_out") as c:
        c.attr(label="Outputs", **CLUSTER_BASE)
        _box(c, "qd", "Qdrant", icon="qdrant", subtitle="per-course collections")
        _box(c, "linker_db", "linker_decisions", icon="postgres", subtitle="kg_state")
        _box(c, "ready", "status = ready", shape="stadium")

    for s in ("upload", "ms", "mbz", "play_url"):
        g.edge(s, "queue")
    g.edge("queue", "gate")

    g.edge("gate", "extract", label="PDF / docx /\nplain text")
    g.edge("gate", "awaiting", label="play.dsv URL", style="dashed")
    g.edge("awaiting", "ghx", style="dashed")
    g.edge("ghx", "extract")

    g.edge("extract", "classify")
    g.edge("classify", "chunk")
    g.edge("chunk", "embed")
    g.edge("embed", "qd")
    g.edge("embed", "kg")
    g.edge("kg", "linker_db")
    g.edge("embed", "ready")

    return g


# --- 3. Chat / RAG -----------------------------------------------------------
def chat_pipeline() -> graphviz.Digraph:
    g = _new_graph("chat", rankdir="TB", ranksep="0.65")

    _box(g, "student", "student message", shape="stadium")

    with g.subgraph(name="cluster_pre") as c:
        c.attr(label="Pre-generation guard", **CLUSTER_BASE)
        _box(c, "intent", "intent classifier", shape="diamond",
             subtitle="llama3.1-8b", bold=False)
        _box(c, "lift", "kg_state refusal lift")
        _box(c, "strat", "strategy", shape="diamond", bold=False)

    with g.subgraph(name="cluster_simple") as c:
        c.attr(label="simple", **CLUSTER_BASE)
        _box(c, "r1", "embed query, top-k", icon="qdrant")

    with g.subgraph(name="cluster_parallel") as c:
        c.attr(label="parallel", **CLUSTER_BASE)
        _box(c, "p1", "stream + retrieve\nconcurrently")

    # FLARE shown as a multi-turn loop: stream a sentence, score logprobs,
    # if low -> retrieve again -> resume; iteration is capped.
    with g.subgraph(name="cluster_flare") as c:
        c.attr(label="FLARE (multi-turn loop)", **CLUSTER_BASE)
        _box(c, "f_gen", "generate next sentence")
        _box(c, "f_score", "low-logprob token?",
             shape="diamond", bold=False)
        _box(c, "f_retr", "re-retrieve\n(use sentence as query)", icon="qdrant")
        _box(c, "f_done", "response complete\nor cap reached", shape="stadium")
        c.edge("f_gen", "f_score")
        c.edge("f_score", "f_retr", label="yes")
        c.edge("f_retr", "f_gen", label="resume", color="#3b6ea5")
        c.edge("f_score", "f_done", label="no /\nfinished")

    _box(g, "kg", "KG expansion", subtitle="part_of_unit / applied_in")

    with g.subgraph(name="cluster_gen") as c:
        c.attr(label="Generation", **CLUSTER_BASE)
        _box(c, "prompt", "assemble prompt",
             subtitle="system + chunks + history")
        _box(c, "llm", "Cerebras LLM",
             subtitle="OpenAI-compatible SSE stream")

    with g.subgraph(name="cluster_post") as c:
        c.attr(label="Post-generation guard", **CLUSTER_BASE)
        _box(c, "out_class", "output classifier", shape="diamond",
             subtitle="per chunk", bold=False)
        _box(c, "rewrite", "Socratic rewrite", subtitle="gpt-oss-120b")
        _box(c, "out", "stream to student", shape="stadium")

    with g.subgraph(name="cluster_book") as c:
        c.attr(label="Bookkeeping", **CLUSTER_BASE)
        _box(c, "log", "conversation_flags +\ncourse_token_usage", icon="postgres")
        _box(c, "caps", "daily caps",
             shape="diamond", subtitle="student + owner", bold=False)
        _box(c, "over", "HTTP 429 next turn", shape="stadium")

    g.edge("student", "intent")
    g.edge("intent", "strat", label="benign")
    g.edge("intent", "lift", label="exfil intent", color=GUARD)
    g.edge("lift", "strat")

    g.edge("strat", "r1", label="simple")
    g.edge("strat", "p1", label="parallel")
    g.edge("strat", "f_gen", label="FLARE")

    g.edge("r1", "kg")
    g.edge("p1", "kg")
    g.edge("f_done", "kg")

    g.edge("kg", "prompt")
    g.edge("prompt", "llm")
    g.edge("llm", "out_class")

    g.edge("out_class", "out", label="clean")
    g.edge("out_class", "rewrite", label="over-extraction", color=GUARD)
    g.edge("rewrite", "out")

    g.edge("out", "log")
    g.edge("log", "caps")
    g.edge("caps", "over", label="over", color=GUARD)

    return g


# --- driver ------------------------------------------------------------------
HREF_RE = re.compile(r'(xlink:href|href)="([^"]+\.(?:png|jpg|jpeg|svg))"')


def _inline_images(svg_path: Path) -> None:
    text = svg_path.read_text()

    def repl(match: re.Match) -> str:
        attr, src = match.group(1), match.group(2)
        path = Path(src)
        if not path.is_absolute() or not path.exists():
            return match.group(0)
        mime = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        data = base64.b64encode(path.read_bytes()).decode("ascii")
        return f'{attr}="data:{mime};base64,{data}"'

    new = HREF_RE.sub(repl, text)
    svg_path.write_text(new)


def _render(g: graphviz.Digraph, name: str) -> None:
    target = OUT / name
    g.render(filename=str(target), format="svg", cleanup=True)
    _inline_images(target.with_suffix(".svg"))


if __name__ == "__main__":
    if not shutil.which("dot"):
        raise SystemExit("graphviz `dot` binary not found on PATH")
    _render(system_overview(), "system-overview")
    _render(ingest_pipeline(), "ingest-pipeline")
    _render(chat_pipeline(), "chat-pipeline")
    print("done")
