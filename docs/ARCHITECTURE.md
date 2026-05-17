# Minerva architecture

The figures here are rendered from [`docs/diagrams/build.py`](diagrams/build.py)
(graphviz under the hood). To regenerate after editing:

```bash
sudo apt-get install graphviz
python3 -m venv /tmp/diags
/tmp/diags/bin/pip install graphviz diagrams
/tmp/diags/bin/python docs/diagrams/build.py
```

## System overview

![System overview](diagrams/system-overview.svg)

Apache unsets identity headers `early` outside of `mod_shib` / Lua paths;
LMS, iframe, and service-account routes carry their own bearer-token or
HMAC-signed-token middleware. Double-headed arrows indicate read/write
relationships; single-headed arrows are push-only.

## Document ingest pipeline

![Ingest pipeline](diagrams/ingest-pipeline.svg)

The kind classifier runs *before* chunking, so assignments and solutions can
be excluded from prompt context for student-facing chats. Embeddings are
written to a per-course Qdrant collection versioned by
`(course_id, embedding_model)`; re-embedding under a new model is lazy and
the old version stays live until rotation finishes. The KG linker reads
excerpts and embeddings from Qdrant (no PDF re-parsing) and caches per-pair
decisions.

## Chat / RAG pipeline

![Chat pipeline](diagrams/chat-pipeline.svg)

Both strategies share the same retrieval ; KG expansion ; prompt assembly
; LLM core. They differ only in *when* retrieval happens relative to
generation:

- **simple**: retrieve once, then generate. The default.
- **FLARE**: per-sentence feedback loop *during* generation. The LLM streams
  a sentence; if any token is low-confidence, the partial sentence is fed
  back as the next retrieval query and generation resumes against the
  augmented context. Iteration is capped per response (the dashed blue
  arrow in the diagram). FLARE doesn't precede the regular pipeline; it
  loops inside it.

The legacy `parallel` strategy (stream + retrieve concurrently) was retired
in migration `20260519000001_tool_use_and_drop_parallel.sql`; existing rows
were remapped to `simple`. Its replacement is the orthogonal **tool-use**
axis: each course has a `tool_use_enabled` toggle that, when on, splits
generation into a hidden-thinking research phase (the model calls
`keyword_search`, RAG-seed and KG-expansion tools, and for `flare` the
logprob signal is injected as a tool event) followed by a clean writeup
phase. Research thinking, per-tool expandable results, wall-clock duration,
and a research/writeup token-split (prompt + completion subsets) are
persisted on the message and rendered above the assistant bubble.

Two independent classifier paths sit around the chat hot path:

- **Extraction guard** (gated by `extraction_guard`): per-turn intent
  classifier before generation, per-chunk output classifier after, KG-driven
  multi-turn proximity tracking, and a Socratic rewriter on `gpt-oss-120b`
  when the output check trips. Every decision is appended to
  `conversation_flags` so teachers can audit activations from the
  "Needs Review" tab.
- **Aegis** (gated by `aegis`): pre-send prompt-coaching analyzer that
  runs off the hot path. Every debounced keystroke POSTs to
  `/aegis/analyze`; the verdict the student had on screen at submit is
  persisted alongside the message. Severity-tagged suggestions (CLEAR-grounded
  rubric, 8 kinds) soft-block Send; `Use ideas` POSTs to `/aegis/rewrite`
  for a `gpt-oss-120b` revision of the draft.

Inline citations: replies carry `[n]` badges (naked-digit + filename-form
variants both accepted); the right-rail sources panel reports actually-cited
sources first with a toggle for the uncited remainder.

Classifiers run on `llama3.1-8b` for latency. Token spend lands in
`course_token_usage` under per-feature categories so daily caps
(per-student-per-course + per-owner aggregate) cover Aegis, the
extraction guard, and the writeup phase as cleanly as they cover the main
chat reply.
