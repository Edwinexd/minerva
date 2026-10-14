"""
Regenerates the architecture / pipeline figures used in docs/ARCHITECTURE.md.

Uses raw graphviz (via the `graphviz` Python package) so we get full control
over node layout: compact rows of "small icon + bold name + light subtitle"
instead of the diagrams library's default oversized-icon-on-top look.

Requires: graphviz binary + the `graphviz` Python package + the `diagrams`
package (we vendor a few icons from its bundled resources).

  sudo apt-get install graphviz   # or: brew install graphviz
  python3 -m venv venv
  venv/bin/pip install -r docs/diagrams/requirements.txt
  venv/bin/python docs/diagrams/build.py
"""

from __future__ import annotations

import base64
import importlib.util
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
# The diagrams package ships its icons in a sibling `resources` directory.
_DIAGRAMS_SPEC = importlib.util.find_spec("diagrams")
_DIAGRAMS_RES = (
    Path(_DIAGRAMS_SPEC.origin).resolve().parent.parent / "resources"
    if _DIAGRAMS_SPEC and _DIAGRAMS_SPEC.origin
    else Path("/nonexistent")
)


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

    # HTML labels ignore "\n"; break lines the way the table expects.
    title = title.replace("\n", "<BR/>")
    subtitle = subtitle.replace("\n", "<BR/>") if subtitle else subtitle

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

    # Actions hold the SU login, so everything that logs in to Play runs
    # there and reports back through the service API.
    with g.subgraph(name="cluster_gha") as c:
        c.attr(label="GitHub Actions (SU login)", **CLUSTER_BASE)
        _box(c, "gha_tr", "transcripts.yml", icon="github",
             subtitle="hourly: designations + VTT")
        _box(c, "gha_ve", "visual-extraction.yml", icon="github",
             subtitle="every 20 min: signed sources")

    with g.subgraph(name="cluster_edge") as c:
        c.attr(label="Apache edge", **CLUSTER_BASE)
        _box(c, "shib", "mod_shib", icon="apache", subtitle="Shibboleth SSO")
        _box(c, "lua", "mod_lua", icon="apache", subtitle="external-auth invites")

    with g.subgraph(name="cluster_app") as c:
        c.attr(label="minerva-app (axum, scales horizontally)", **CLUSTER_BASE)
        _box(c, "api", "HTTP API", icon="rust", subtitle="auth + chat strategy")

    with g.subgraph(name="cluster_bg") as c:
        c.attr(label="Background workers", **CLUSTER_BASE)
        _box(c, "worker", "minerva-worker", icon="rust",
             subtitle="ingest + KG linker")
        _box(c, "cron", "minerva-scheduler", icon="rust",
             subtitle="Canvas / NRPS / visual extraction")
        _box(c, "fetcher", "minerva-fetcher", icon="rust",
             subtitle="Play source probe + download")

    with g.subgraph(name="cluster_models") as c:
        c.attr(label="Model servers (internal gRPC)", **CLUSTER_BASE)
        _box(c, "embedder", "minerva-embedder", icon="huggingface",
             subtitle="FastEmbedder LRU")
        _box(c, "reranker", "minerva-reranker", icon="huggingface",
             subtitle="cross-encoder LRU")

    with g.subgraph(name="cluster_state") as c:
        c.attr(label="Stateful", **CLUSTER_BASE)
        _box(c, "pg", "PostgreSQL 16", icon="postgres")
        _box(c, "qd", "Qdrant", icon="qdrant", subtitle="per-course collections")
        _box(c, "docs", "/data0/minerva/data", subtitle="document blobs")
        _box(c, "staging", "visual staging",
             subtitle="lecture videos, bounded by free disk")

    with g.subgraph(name="cluster_ai") as c:
        c.attr(label="AI providers", **CLUSTER_BASE)
        # One registry, many vendors: which one a call lands on is the
        # `provider` column of the model it names in the `chat_models`
        # catalog, and a provider is only registered when its key is set.
        _box(c, "llm", "LLM registry",
             subtitle="Cerebras / OpenAI / Anthropic<BR/>Groq / Gemini")
        _box(c, "oai", "OpenAI", subtitle="embeddings")

    with g.subgraph(name="cluster_ext") as c:
        c.attr(label="External content", **CLUSTER_BASE)
        _box(c, "canvas", "Canvas LMS", subtitle="REST sync")
        _box(c, "play", "play.dsv.su.se", subtitle="lecture video + VTT")

    with g.subgraph(name="cluster_olympus") as c:
        c.attr(label="Olympus (DSV Slurm, L40S GPUs)", **CLUSTER_BASE)
        _box(c, "slurm", "slide-ocr workers", icon="python",
             subtitle="OCR (vLLM) + figure CLIP")

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

    # API <-> LLM (request + SSE response). The per-course model decides
    # the provider; every provider is reached through the same registry.
    g.edge("api", "llm", dir="both", label="chat")

    # API -> model servers over gRPC (chat query embed + rerank). The api
    # owns no model weights; it reaches the embedder / reranker pods.
    g.edge("api", "embedder", label="gRPC")
    g.edge("api", "reranker", label="gRPC")

    # Worker: claim queue, write vectors, read/write blobs, embed via gRPC
    # (or OpenAI for openai-provider courses), classify + KG-link via LLM.
    g.edge("worker", "pg", dir="both")
    g.edge("worker", "qd")
    g.edge("worker", "docs", dir="both")
    g.edge("worker", "embedder", label="gRPC embed")
    g.edge("worker", "oai", label="embed")
    g.edge("worker", "llm", dir="both", label="link / classify")

    # Scheduler: due-row scans + Canvas pulls (downloads land in docs), the
    # Slurm workers it submits over SSH, and the figure-vector sweep.
    g.edge("cron", "pg", dir="both")
    g.edge("cron", "docs")
    g.edge("cron", "canvas", dir="both", label="pull")
    g.edge("cron", "slurm", label="sbatch / sacct (SSH)")
    g.edge("cron", "embedder", label="figure context")
    g.edge("cron", "qd", label="figure vectors")
    g.edge("cron", "llm", dir="both", label="crop descriptions (separate loop)")

    # Play is only reached from Actions (login) and the fetcher (signed
    # URLs Actions handed over); both report to Minerva, never to each other.
    g.edge("gha_tr", "play", dir="both", label="pull")
    g.edge("gha_ve", "play", dir="both", label="sources")
    g.edge("gha_tr", "api", label="service API")
    g.edge("gha_ve", "api", label="slots / offers")
    g.edge("fetcher", "pg", dir="both")
    g.edge("fetcher", "play", label="probe + MP4")
    g.edge("fetcher", "staging")

    # Workers pull one item at a time through presigned URLs.
    g.edge("slurm", "api", dir="both", label="next / source / result")
    g.edge("api", "staging", label="serve source")

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
        _box(c, "classify", "kind classifier", subtitle="utility-default model")
        _box(c, "chunk", "chunker")
        _box(c, "embed", "embedder", subtitle="OpenAI or fastembed")
        _box(c, "kg", "KG linker", subtitle="cross-doc edges")

    with g.subgraph(name="cluster_cron") as c:
        c.attr(label="Hourly transcript fetch", **CLUSTER_BASE)
        _box(c, "awaiting", "awaiting_transcript", subtitle="queue")
        _box(c, "ghx", "transcripts.yml", icon="github", subtitle="GitHub Actions")

    with g.subgraph(name="cluster_visual") as c:
        c.attr(label="Visual extraction (Olympus, see its own figure)",
               **CLUSTER_BASE)
        _box(c, "vjobs", "visual_extraction_jobs", icon="postgres",
             subtitle="Play lectures + PDFs")
        _box(c, "ocr", "slide-ocr worker", icon="python",
             subtitle="slides, OCR, figure crops")

    with g.subgraph(name="cluster_out") as c:
        c.attr(label="Outputs", **CLUSTER_BASE)
        _box(c, "qd", "Qdrant", icon="qdrant", subtitle="per-course collections")
        _box(c, "figs", "document_figures", icon="qdrant",
             subtitle="crop descriptions + CLIP vectors")
        _box(c, "describe", "figure_descriptions", icon="postgres",
             subtitle="separate loop: vision model,<BR/>cached descriptions, decoration filter")
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

    # A searchable PDF and a Play lecture with a transcript each get a
    # visual job. The result comes back as a document to re-ingest: OCR
    # text for the PDF (chunks swapped without a gap), a slides-plus-speech
    # child replacing the transcript for a lecture.
    g.edge("ready", "vjobs", label="PDF", style="dashed")
    g.edge("awaiting", "vjobs", label="Play lecture", style="dashed")
    g.edge("vjobs", "ocr")
    g.edge("ocr", "queue", label="OCR text /\nslides + speech", style="dashed",
           constraint="false")
    g.edge("ocr", "figs", label="figures")
    g.edge("figs", "describe", dir="both", label="post-extraction enrichment")

    return g


# --- 3. Chat / RAG -----------------------------------------------------------
def chat_pipeline() -> graphviz.Digraph:
    g = _new_graph("chat", rankdir="TB", ranksep="0.65")

    _box(g, "student", "student message", shape="stadium")

    with g.subgraph(name="cluster_aegis") as c:
        c.attr(label="Aegis (pre-send, off hot path)", **CLUSTER_BASE)
        _box(c, "aegis_analyze", "analyze draft",
             subtitle="gpt-oss-120b, debounced keystrokes")
        _box(c, "aegis_rewrite", "rewrite on Use ideas",
             subtitle="gpt-oss-120b")
        c.edge("aegis_analyze", "aegis_rewrite",
               label="optional", style="dashed")

    with g.subgraph(name="cluster_pre") as c:
        c.attr(label="Pre-generation guard", **CLUSTER_BASE)
        _box(c, "intent", "intent classifier", shape="diamond",
             subtitle="gpt-oss-120b", bold=False)
        _box(c, "lift", "kg_state refusal lift")
        _box(c, "strat", "strategy", shape="diamond", bold=False)

    # Both strategies share retrieval ; KG ; prompt ; LLM. They differ in
    # *when* retrieval runs relative to generation:
    #   simple : retrieve once, then generate.
    #   FLARE  : multi-turn. The LLM streams a sentence; if a token is
    #            low-confidence, the partial sentence becomes the next
    #            retrieval query and generation resumes (loop is capped).
    # The legacy `parallel` strategy (concurrent stream + retrieve) was
    # retired in migration 20260519000001; existing rows were remapped to
    # `simple`. The replacement is the orthogonal `tool_use_enabled` toggle,
    # which splits generation into a hidden research phase (model calls
    # tools, with FLARE's logprob signal injected as a tool event) and a
    # clean writeup phase. Rendered here as the dashed orange research box
    # between retrieval and prompt assembly.
    with g.subgraph(name="cluster_retrieve") as c:
        c.attr(label="Retrieval", **CLUSTER_BASE)
        _box(c, "retrieve", "embed query, top-k", icon="qdrant")
        _box(c, "kg", "KG expansion",
             subtitle="part_of_unit / applied_in")
        _box(c, "figsearch", "figure search",
             subtitle="crop descriptions + CLIP,<BR/>selected figures inline")
        c.edge("retrieve", "kg")
        c.edge("retrieve", "figsearch")

    with g.subgraph(name="cluster_gen") as c:
        c.attr(label="Generation", **CLUSTER_BASE)
        _box(c, "research", "research phase",
             shape="diamond",
             subtitle="tool_use_enabled: tools + thinking",
             bold=False)
        _box(c, "prompt", "assemble prompt",
             subtitle="system + chunks + history\n(writeup phase if tool_use)")
        _box(c, "llm", "chat model",
             subtitle="provider from the course model<BR/>(SSE stream)")
        _box(c, "flare_check", "low-logprob token?",
             shape="diamond", subtitle="FLARE only", bold=False)
        c.edge("prompt", "llm")
        c.edge("llm", "flare_check", label="per sentence")

    with g.subgraph(name="cluster_post") as c:
        c.attr(label="Post-generation guard (extraction_guard flag)",
               **CLUSTER_BASE)
        _box(c, "out_class", "output classifier", shape="diamond",
             subtitle="per chunk, gpt-oss-120b", bold=False)
        _box(c, "rewrite", "Socratic rewrite", subtitle="gpt-oss-120b")
        _box(c, "out", "stream to student\n+ inline [n] citations\n+ [Figure N] images",
             shape="stadium")

    with g.subgraph(name="cluster_book") as c:
        c.attr(label="Bookkeeping", **CLUSTER_BASE)
        _box(c, "log", "conversation_flags +\ncourse_token_usage", icon="postgres")
        _box(c, "caps", "daily caps",
             shape="diamond", subtitle="student + owner", bold=False)
        _box(c, "over", "HTTP 429 next turn", shape="stadium")

    g.edge("student", "aegis_analyze",
           label="draft (pre-send)", style="dashed",
           color="#3b6ea5", constraint="false")
    g.edge("aegis_rewrite", "student",
           label="revised draft", style="dashed",
           color="#3b6ea5", constraint="false")
    g.edge("student", "intent", label="on send")
    g.edge("intent", "strat", label="benign")
    g.edge("intent", "lift", label="exfil intent", color=GUARD)
    g.edge("lift", "strat")

    # Each strategy enters retrieval. The KG-expanded chunks flow into the
    # optional research phase when `tool_use_enabled` is on, otherwise the
    # research box is a pass-through to prompt assembly.
    g.edge("strat", "retrieve", label="simple / FLARE-init")

    g.edge("kg", "research")
    g.edge("figsearch", "research", label="up to 3 figures")
    g.edge("research", "prompt")

    # FLARE feedback loop: if a streamed token is low-confidence, use the
    # partial sentence as the next retrieval query and resume generation.
    g.edge("flare_check", "retrieve",
           label="FLARE: re-retrieve\n(partial sentence as query)",
           color="#3b6ea5", style="dashed", constraint="false")
    g.edge("flare_check", "out_class", label="continue / finished")

    g.edge("out_class", "out", label="clean")
    g.edge("out_class", "rewrite", label="over-extraction", color=GUARD)
    g.edge("rewrite", "out")

    g.edge("out", "log")
    g.edge("log", "caps")
    g.edge("caps", "over", label="over", color=GUARD)

    return g


# --- 4. Visual extraction ----------------------------------------------------
def visual_extraction() -> graphviz.Digraph:
    g = _new_graph("visual", rankdir="LR", ranksep="0.75", nodesep="0.35")

    with g.subgraph(name="cluster_gha") as c:
        c.attr(label="GitHub Actions", **CLUSTER_BASE)
        _box(c, "gha", "visual-extraction.yml", icon="github",
             subtitle="every 20 min, SU login,<BR/>headers only")
        _box(c, "deploy", "deploy-slide-ocr.yml", icon="github",
             subtitle="on gpu/slide-ocr/ push")

    _box(g, "play", "play.dsv.su.se",
         subtitle="signed probe + download URLs, VTT")

    with g.subgraph(name="cluster_app") as c:
        c.attr(label="minerva-app", **CLUSTER_BASE)
        _box(c, "routes", "/api/service/visual-extraction", icon="rust",
             subtitle="slots, offers, releases,<BR/>work queue, results")

    with g.subgraph(name="cluster_fetcher") as c:
        c.attr(label="minerva-fetcher", **CLUSTER_BASE)
        _box(c, "probe", "slide source probe",
             subtitle="frames via ffmpeg + tesseract")
        _box(c, "download", "download",
             subtitle="earliest deadline first")
        c.edge("probe", "download", label="winner")

    with g.subgraph(name="cluster_state") as c:
        c.attr(label="Minerva state", **CLUSTER_BASE)
        _box(c, "jobs", "visual_extraction_jobs", icon="postgres",
             subtitle="the only queue state")
        _box(c, "staging", "visual staging",
             subtitle="free disk less 10 GiB guard")
        _box(c, "qd", "Qdrant", icon="qdrant",
             subtitle="figure context + CLIP")

    with g.subgraph(name="cluster_sched") as c:
        c.attr(label="minerva-scheduler", **CLUSTER_BASE)
        _box(c, "tick", "visual_extraction::tick", icon="rust",
             subtitle="every minute: enqueue, Slurm,<BR/>figure index, metrics")
        _box(c, "describe", "figure_descriptions::tick", icon="rust",
             subtitle="independent loop: vision model,<BR/>Flex when enabled, cached descriptions")

    _box(g, "descriptions", "figure_descriptions", icon="postgres",
         subtitle="leases, retries, descriptions,<BR/>decorative crops excluded")
    _box(g, "vision", "catalog vision model",
         subtitle="Standard / Flex variants")
    g.edge("describe", "descriptions", dir="both", label="claim / cache")
    g.edge("describe", "vision", dir="both", label="crop + context / description")
    g.edge("descriptions", "tick", label="ready to index")

    with g.subgraph(name="cluster_olympus") as c:
        c.attr(label="Olympus (Slurm, L40S GPUs)", **CLUSTER_BASE)
        _box(c, "gate", "forced-command SSH gates",
             subtitle="slide-ocr-gate / ssh-gate")
        _box(c, "worker", "slide-ocr worker", icon="python",
             subtitle="Unlimited-OCR on vLLM,<BR/>CLIP ViT-B/32, at most 12 h")
        c.edge("gate", "worker", label="sbatch")

    _box(g, "ingest", "minerva-worker", icon="rust",
         subtitle="slides + speech child,<BR/>OCR text re-ingest")

    # Actions: reserve a slot, describe the lecture's sources, offer them.
    # A lecture Play cannot serve yet is released with a backoff instead.
    g.edge("gha", "play", dir="both", label="login, sources, cues")
    g.edge("gha", "routes",
           label="reserve, offer / release\n(not ready: 1 h doubling to 7 d)")
    g.edge("routes", "jobs", dir="both")

    # Fetcher: probe multi-source lectures, download the slide source.
    g.edge("probe", "jobs", dir="both", label="claim")
    g.edge("probe", "play", label="sampled frames")
    g.edge("download", "play", label="MP4")
    g.edge("download", "staging")

    # Scheduler: Slurm over SSH, dead-worker requeue, figure vectors.
    g.edge("tick", "jobs", dir="both")
    g.edge("tick", "gate", label="sbatch / sacct / scancel")
    g.edge("tick", "qd", label="figure vectors")

    # Worker: one item at a time over presigned URLs.
    g.edge("worker", "routes", dir="both",
           label="next, GET source,\nPUT result")
    g.edge("staging", "routes", label="source")
    g.edge("routes", "ingest", label="accepted result")

    g.edge("deploy", "gate", label="worker code", style="dashed")

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
    _render(visual_extraction(), "visual-extraction")
    print("done")
