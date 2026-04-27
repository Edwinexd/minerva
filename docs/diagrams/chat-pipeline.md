# Chat / RAG pipeline

Source: `backend/crates/minerva-server/src/chat/`. Three strategies share the
same retrieval, KG-expansion, and extraction-guard layers; they differ in
when retrieval happens relative to generation.

```mermaid
flowchart TD
  U[student message] --> EG1{extraction_guard<br/>intent classifier<br/>llama3.1-8b}
  EG1 -->|benign| S{strategy}
  EG1 -->|exfil intent| LIFT[set kg_state<br/>refusal lift]
  LIFT --> S

  S -->|simple| R1[embed query -> Qdrant top-k]
  S -->|parallel| P1[start LLM stream + retrieve concurrently<br/>splice context when ready]
  S -->|FLARE| F1[generate sentence -> low-logprob trigger<br/>retrieve -> continue]

  R1 --> KG[KG expansion<br/>part_of_unit / applied_in partners]
  P1 --> KG
  F1 --> KG

  KG --> CTX[assemble prompt<br/>system + KG-expanded chunks +<br/>conversation history]
  CTX --> LLM[Cerebras / OpenAI-compatible<br/>SSE stream to client]

  LLM --> EG2{extraction_guard<br/>output classifier per chunk}
  EG2 -->|clean| OUT[stream to student]
  EG2 -->|over-extraction| RW[Socratic rewrite<br/>gpt-oss-120b]
  RW --> OUT

  OUT --> LOG[(conversation_flags +<br/>course_token_usage)]
  LOG --> CAP{daily caps<br/>per-student-per-course +<br/>per-owner aggregate}
  CAP -->|over| 429[HTTP 429 next turn]
```

The intent classifier and per-chunk output classifier run on `llama3.1-8b`
for latency; the Socratic rewriter uses `gpt-oss-120b` because it needs to
produce coherent prose. Every classifier decision and rewrite is appended
to `conversation_flags` so teachers can audit activations from the
"Needs Review" tab.
