"""
Regenerates the architecture / pipeline figures used in the README.

Requires: graphviz + the `diagrams` python package.

  python3 -m venv /tmp/diags
  /tmp/diags/bin/pip install diagrams
  /tmp/diags/bin/python docs/diagrams/build.py
"""

import base64
import mimetypes
import re
from pathlib import Path

from diagrams import Cluster, Diagram, Edge
from diagrams.generic.database import SQL
from diagrams.generic.storage import Storage
from diagrams.onprem.client import Client, User
from diagrams.onprem.compute import Server
from diagrams.onprem.database import PostgreSQL
from diagrams.onprem.network import Apache
from diagrams.onprem.vcs import Github
from diagrams.programming.flowchart import (
    Action,
    Decision,
    Document,
    InternalStorage,
    StartEnd,
)
from diagrams.programming.framework import React
from diagrams.programming.language import Rust

OUT = Path(__file__).resolve().parent

# --- shared style ------------------------------------------------------------
# All three diagrams share the same look so they can sit on the same page
# without clashing.

GRAPH_ATTR = {
    "fontname": "Helvetica",
    "fontsize": "13",
    "bgcolor": "white",
    "pad": "0.6",
    "splines": "spline",
    "nodesep": "0.55",
    "ranksep": "0.85",
}
NODE_ATTR = {"fontname": "Helvetica", "fontsize": "12"}
EDGE_ATTR = {"fontname": "Helvetica", "fontsize": "11", "color": "#555555"}

# Uniform cluster styling so every box on the page reads as the same kind of
# boundary.
def cluster_attr(label_pos: str = "b") -> dict:
    return {
        "fontname": "Helvetica-Bold",
        "fontsize": "12",
        "labelloc": label_pos,
        "bgcolor": "#fafafa",
        "pencolor": "#9aa0a6",
        "style": "rounded,dashed",
        "margin": "18",
    }


GUARD = "#cc4444"


# -----------------------------------------------------------------------------
# 1. System overview - left to right.
# -----------------------------------------------------------------------------
with Diagram(
    "Minerva system overview",
    filename=str(OUT / "system-overview"),
    outformat="svg",
    show=False,
    direction="LR",
    graph_attr={**GRAPH_ATTR, "rankdir": "LR", "ranksep": "1.2"},
    node_attr=NODE_ATTR,
    edge_attr=EDGE_ATTR,
):
    with Cluster("Clients", graph_attr=cluster_attr()):
        web = React("Web SPA")
        embed = Client("Iframe embed")
        moodle = Server("Moodle plugin\nlocal_minerva")
        lti = Server("LTI 1.3 platform")

    with Cluster("Apache edge", graph_attr=cluster_attr()):
        shib = Apache("mod_shib\nShibboleth SSO")
        lua = Apache("mod_lua\nexternal-auth invites")

    with Cluster("minerva-app", graph_attr=cluster_attr()):
        api = Rust("axum HTTP API")
        worker = Action("ingest worker")
        kg = Action("KG linker / sweeper")
        cron = Action("Canvas + transcript\nschedulers")

    with Cluster("Stateful", graph_attr=cluster_attr()):
        pg = PostgreSQL("PostgreSQL 16")
        qdrant = SQL("Qdrant\nper-course collections")
        docs = Storage("/data0/minerva/data")
        hf = Storage("HuggingFace\nfastembed cache")

    with Cluster("AI providers", graph_attr=cluster_attr()):
        llm = Server("Cerebras /\nOpenAI-compatible LLM")
        oai = Server("OpenAI embeddings")

    with Cluster("External content sources", graph_attr=cluster_attr()):
        canvas = Server("Canvas LMS\n(REST sync)")
        play = Server("play.dsv.su.se\nVTT transcripts")

    web >> shib
    embed >> api
    moodle >> api
    lti >> api
    shib >> api
    lua >> api

    api >> pg
    api >> qdrant
    api >> docs
    api >> Edge(label="chat") >> llm
    worker >> Edge(label="embed") >> oai
    worker >> hf
    worker >> qdrant
    kg >> Edge(label="link / classify") >> llm
    cron >> canvas
    cron >> play


# -----------------------------------------------------------------------------
# 2. Document ingest pipeline - top to bottom.
# -----------------------------------------------------------------------------
with Diagram(
    "Document ingest",
    filename=str(OUT / "ingest-pipeline"),
    outformat="svg",
    show=False,
    direction="TB",
    graph_attr={**GRAPH_ATTR, "rankdir": "TB"},
    node_attr=NODE_ATTR,
    edge_attr=EDGE_ATTR,
):
    with Cluster("Sources", graph_attr=cluster_attr()):
        upload = User("Direct upload")
        moodle_in = Server("Moodle / Canvas sync")
        mbz = Document("MBZ import")
        play_url = Server("play.dsv URL drop")

    with Cluster("Queue", graph_attr=cluster_attr()):
        docs_db = PostgreSQL("documents\nstatus = pending")

    with Cluster("Ingest worker", graph_attr=cluster_attr()):
        gate = Decision("mime / source\nrouter")
        extract = Action("poppler / extractor")
        classify = Action("kind classifier\nllama3.1-8b")
        chunk = Action("chunker")
        embed_worker = Action("embedder\nOpenAI or fastembed")
        kg_link = Action("KG linker\ncross-doc edges")

    with Cluster("Hourly transcript fetch", graph_attr=cluster_attr()):
        awaiting = InternalStorage("awaiting_transcript\nqueue")
        cron_gh = Github("transcripts.yml\nGitHub Actions")

    with Cluster("Outputs", graph_attr=cluster_attr()):
        qdrant = SQL("Qdrant\nper-course collections")
        linker_db = PostgreSQL("linker_decisions\nkg_state")
        ready = StartEnd("status = ready")

    [upload, moodle_in, mbz, play_url] >> docs_db >> gate

    gate >> Edge(label="PDF / docx /\nplain text") >> extract
    gate >> Edge(label="play.dsv URL", style="dashed") >> awaiting
    awaiting >> Edge(style="dashed") >> cron_gh >> extract

    extract >> classify >> chunk >> embed_worker
    embed_worker >> qdrant
    embed_worker >> kg_link >> linker_db
    embed_worker >> ready


# -----------------------------------------------------------------------------
# 3. Chat / RAG pipeline - top to bottom.
# -----------------------------------------------------------------------------
with Diagram(
    "Chat / RAG",
    filename=str(OUT / "chat-pipeline"),
    outformat="svg",
    show=False,
    direction="TB",
    graph_attr={**GRAPH_ATTR, "rankdir": "TB"},
    node_attr=NODE_ATTR,
    edge_attr=EDGE_ATTR,
):
    with Cluster("Input", graph_attr=cluster_attr()):
        student = User("student message")

    with Cluster("Pre-generation guard", graph_attr=cluster_attr()):
        intent = Decision("intent classifier\nllama3.1-8b")
        lift = Action("kg_state\nrefusal lift")
        strat = Decision("strategy")

    with Cluster("Retrieval", graph_attr=cluster_attr()):
        simple = Action("simple\nembed query, top-k")
        parallel = Action("parallel\nstream + retrieve")
        flare = Action("FLARE\nlow-logprob retrieve")
        kg_expand = Action("KG expansion\npart_of_unit / applied_in")

    with Cluster("Generation", graph_attr=cluster_attr()):
        prompt = Action("assemble prompt\nsystem + chunks + history")
        llm = Server("Cerebras /\nOpenAI-compatible\nSSE stream")

    with Cluster("Post-generation guard", graph_attr=cluster_attr()):
        out_class = Decision("output classifier\nper chunk")
        rewrite = Action("Socratic rewrite\ngpt-oss-120b")
        out = Client("stream to student")

    with Cluster("Bookkeeping", graph_attr=cluster_attr()):
        log = PostgreSQL("conversation_flags\ncourse_token_usage")
        caps = Decision("daily caps\nstudent + owner")
        over = StartEnd("HTTP 429\nnext turn")

    student >> intent
    intent >> Edge(label="benign") >> strat
    intent >> Edge(label="exfil intent", color=GUARD) >> lift >> strat

    strat >> Edge(label="simple") >> simple
    strat >> Edge(label="parallel") >> parallel
    strat >> Edge(label="FLARE") >> flare
    [simple, parallel, flare] >> kg_expand >> prompt >> llm >> out_class

    out_class >> Edge(label="clean") >> out
    out_class >> Edge(label="over-extraction", color=GUARD) >> rewrite >> out

    out >> log >> caps
    caps >> Edge(label="over", color=GUARD) >> over

# -----------------------------------------------------------------------------
# Post-process: inline external <image> references as base64 data URIs so the
# SVGs are self-contained (graphviz writes absolute paths that point into the
# diagrams Python package, which won't resolve when the SVG is loaded from
# anywhere else).
# -----------------------------------------------------------------------------
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


for name in ("system-overview", "ingest-pipeline", "chat-pipeline"):
    _inline_images(OUT / f"{name}.svg")

print("done")
