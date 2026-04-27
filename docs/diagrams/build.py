"""
Regenerates the architecture / pipeline figures used in the README.

Requires: graphviz + the `diagrams` python package.

  python3 -m venv /tmp/diags
  /tmp/diags/bin/pip install diagrams
  /tmp/diags/bin/python docs/diagrams/build.py
"""

from pathlib import Path

from diagrams import Cluster, Diagram, Edge
from diagrams.generic.compute import Rack
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
from diagrams.programming.language import Python, Rust

OUT = Path(__file__).resolve().parent

# --- shared style ------------------------------------------------------------
GRAPH_ATTR = {
    "fontname": "Helvetica",
    "fontsize": "12",
    "bgcolor": "transparent",
    "pad": "0.5",
    "splines": "spline",
    "nodesep": "0.6",
    "ranksep": "0.85",
    "concentrate": "false",
}
NODE_ATTR = {"fontname": "Helvetica", "fontsize": "11"}
EDGE_ATTR = {"fontname": "Helvetica", "fontsize": "10", "color": "#555555"}
CLUSTER_ATTR = {
    "fontname": "Helvetica",
    "fontsize": "11",
    "labelloc": "b",
    "bgcolor": "#fafafa",
    "pencolor": "#bbbbbb",
    "style": "rounded,dashed",
    "margin": "16",
}

GUARD_EDGE = Edge(color="#cc4444", style="bold")


# -----------------------------------------------------------------------------
# 1. System overview - left to right.
# -----------------------------------------------------------------------------
with Diagram(
    "Minerva system overview",
    filename=str(OUT / "system-overview"),
    outformat="png",
    show=False,
    direction="LR",
    graph_attr={
        **GRAPH_ATTR,
        "rankdir": "LR",
        "ranksep": "1.4",
        "nodesep": "0.4",
        "splines": "ortho",
        "size": "16,9!",
    },
    node_attr=NODE_ATTR,
    edge_attr=EDGE_ATTR,
):
    with Cluster("Clients", graph_attr=CLUSTER_ATTR):
        web = React("Web SPA")
        embed = Client("Iframe embed")
        moodle = Server("Moodle plugin\nlocal_minerva")
        canvas = Server("Canvas LMS")
        lti = Server("LTI 1.3 platform")

    with Cluster("Apache edge", graph_attr=CLUSTER_ATTR):
        shib = Apache("mod_shib\nShibboleth SSO")
        lua = Apache("mod_lua\nexternal-auth invites")

    with Cluster("minerva-app", graph_attr=CLUSTER_ATTR):
        api = Rust("axum HTTP API")
        worker = Action("ingest worker")
        kg = Action("KG linker / sweeper")
        cron = Action("Canvas + transcript\nschedulers")

    with Cluster("Stateful", graph_attr=CLUSTER_ATTR):
        pg = PostgreSQL("PostgreSQL 16")
        qdrant = SQL("Qdrant\nper-course collections")
        docs = Storage("/data0/minerva/data")
        hf = Storage("HuggingFace\nfastembed cache")

    with Cluster("External AI", graph_attr=CLUSTER_ATTR):
        llm = Server("Cerebras /\nOpenAI-compatible LLM")
        oai = Server("OpenAI embeddings")
        play = Server("play.dsv.su.se\nVTT transcripts")

    web >> shib
    embed >> api
    moodle >> api
    canvas >> api
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
    outformat="png",
    show=False,
    direction="TB",
    graph_attr={**GRAPH_ATTR, "rankdir": "TB"},
    node_attr=NODE_ATTR,
    edge_attr=EDGE_ATTR,
):
    with Cluster("Sources", graph_attr=CLUSTER_ATTR):
        upload = User("Direct upload")
        moodle_in = Server("Moodle / Canvas sync")
        mbz = Document("MBZ import")
        play_url = Server("play.dsv URL drop")

    docs_db = PostgreSQL("documents\nstatus = pending")

    with Cluster("Ingest worker", graph_attr=CLUSTER_ATTR):
        gate = Decision("mime / source\nrouter")
        extract = Action("poppler / extractor")
        classify = Action("kind classifier\nllama3.1-8b")
        chunk = Action("chunker")
        embed_worker = Action("embedder\nOpenAI or fastembed")
        kg_link = Action("KG linker\ncross-doc edges")

    with Cluster("Hourly transcript fetch", graph_attr=CLUSTER_ATTR):
        awaiting = InternalStorage("awaiting_transcript\nqueue")
        cron_gh = Github("transcripts.yml\nGitHub Actions")

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
    outformat="png",
    show=False,
    direction="TB",
    graph_attr={**GRAPH_ATTR, "rankdir": "TB"},
    node_attr=NODE_ATTR,
    edge_attr=EDGE_ATTR,
):
    student = User("student message")

    with Cluster("Pre-generation guard", graph_attr=CLUSTER_ATTR):
        intent = Decision("intent classifier\nllama3.1-8b")
        lift = Action("kg_state\nrefusal lift")
        strat = Decision("strategy")

    with Cluster("Retrieval", graph_attr=CLUSTER_ATTR):
        simple = Action("simple\nembed query, top-k")
        parallel = Action("parallel\nstream + retrieve")
        flare = Action("FLARE\nlow-logprob retrieve")
        kg_expand = Action("KG expansion\npart_of_unit / applied_in")

    with Cluster("Generation", graph_attr=CLUSTER_ATTR):
        prompt = Action("assemble prompt\nsystem + chunks + history")
        llm = Server("Cerebras /\nOpenAI-compatible\nSSE stream")

    with Cluster("Post-generation guard", graph_attr=CLUSTER_ATTR):
        out_class = Decision("output classifier\nper chunk")
        rewrite = Action("Socratic rewrite\ngpt-oss-120b")
        out = Client("stream to student")

    with Cluster("Bookkeeping", graph_attr=CLUSTER_ATTR):
        log = PostgreSQL("conversation_flags\ncourse_token_usage")
        caps = Decision("daily caps\nstudent + owner")

    over = StartEnd("HTTP 429\nnext turn")

    student >> intent
    intent >> Edge(label="benign") >> strat
    intent >> Edge(label="exfil intent", color="#cc4444") >> lift >> strat

    strat >> Edge(label="simple") >> simple
    strat >> Edge(label="parallel") >> parallel
    strat >> Edge(label="FLARE") >> flare
    [simple, parallel, flare] >> kg_expand >> prompt >> llm >> out_class

    out_class >> Edge(label="clean") >> out
    out_class >> Edge(label="over-extraction", color="#cc4444") >> rewrite >> out

    out >> log >> caps
    caps >> Edge(label="over", color="#cc4444") >> over

print("done")
