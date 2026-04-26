//! Cross-document linking pass: builds the edges of the course
//! knowledge graph after every doc has a `kind`.
//!
//! The pass is course-scoped. It takes every classified document in
//! a course, fans out embedding-based candidate pairs, and asks
//! gpt-oss-120b to label each pair with a typed relation:
//!
//!   * `solution_of(src=sample_solution, dst=assignment_brief|lab_brief|exam)`
//!   * `part_of_unit(src, dst)` -- two docs that belong to the same
//!     week / module / topic, regardless of kind.
//!
//! **Filenames are NOT used anywhere in this module.** Real DSV course
//! filenames are unreliable (stale templates, copy/paste, names that
//! contradict content), so:
//!
//!   * Candidate generation is pure embedding cosine similarity.
//!   * The LLM sees only `kind`, `classifier_rationale`, and a content
//!     `excerpt` per doc -- no filename, no marker tokens, no derived
//!     priors.
//!   * Post-filters are similarity-floor / confidence-floor /
//!     duplicate-content -- all derived from the actual document
//!     vectors, never from filenames.
//!
//! Why one big call rather than per-pair: pairwise scales O(n^2) and
//! drowns context in noise. A single call lets the model do simple
//! sanity checks across the corpus (cluster the lab+brief+solution+
//! rubric for a unit together based on content) and emit only the
//! confident edges. For courses bigger than the model can fit in one
//! turn we'd paginate by embedding cluster -- but in practice DSV
//! courses are 20-200 docs, well within budget.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use minerva_db::queries::document_relations::RejectedPairKey;
use minerva_db::queries::documents::DocumentRow;
use qdrant_client::qdrant::{Condition, Filter, ScrollPointsBuilder};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use uuid::Uuid;

use crate::strategy::common::{cerebras_request_with_retry, payload_int, payload_string};

const LINKER_MODEL: &str = "gpt-oss-120b";

/// Drop edges the model emits below this confidence.
///
/// Lowered from 0.6 to 0.5 after observing gpt-oss-120b emit
/// `{"edges": []}` on a 47-doc course (266 candidate pairs) when
/// the prompt nudged it toward "don't guess". The model defaults to
/// silence under uncertainty, so the floor needs to be permissive
/// enough that it'll commit to a reasonable guess and let the
/// downstream similarity filter do calibration.
const MIN_EDGE_CONFIDENCE: f32 = 0.5;

/// Drop edges between docs whose embedding similarity is below this
/// threshold. Tuned empirically: lecture+exercise pairs in the same
/// unit usually score 0.5-0.8; pairs from different units sit
/// 0.2-0.5. 0.45 gives a generous floor without burning candidates.
const MIN_EMBEDDING_SIMILARITY: f32 = 0.45;

/// `solution_of` requires a higher similarity floor than
/// `part_of_unit` because solutions are essentially restatements of
/// their assignments and so should score very close.
const MIN_SOLUTION_OF_SIMILARITY: f32 = 0.6;

/// Cosine similarity above which we treat two docs as effectively
/// identical (duplicate uploads). Such pairs are dropped: they're
/// not "in the same unit", they're the same document.
const DUPLICATE_SIMILARITY: f32 = 0.985;

/// Per-doc top-K when generating embedding-similarity candidates.
/// Bounded so a fully-connected course doesn't explode candidate
/// count; combined with the symmetric edge dedup this caps total
/// candidates at roughly N * TOP_K / 2.
const EMBEDDING_TOP_K: usize = 8;

/// Hard cap on docs sent to the linker in one call. Keeps the prompt
/// token cost bounded; courses larger than this would need pagination
/// (not implemented in V1 -- DSV courses are well under the cap).
const MAX_DOCS_PER_CALL: usize = 300;

/// Per-doc content excerpt size for the LLM prompt. Head of the
/// document text -- the linker's only grounding signal beyond
/// (kind, classifier_rationale). Filenames are deliberately excluded.
///
/// Sized for "the LLM has enough context to recognise a shared
/// problem statement / topic". 400 chars (the previous value) was
/// too tight: a typical assignment brief opens with course-name
/// boilerplate, and 400 chars rarely got past the boilerplate to
/// the actual problem. 1500 captures the problem statement on
/// almost every DSV doc we've inspected. Total prompt size with
/// 47 candidate-included docs * 1500 chars ≈ 70KB of content,
/// well inside gpt-oss-120b's 128K window.
const EXCERPT_CHARS: usize = 1500;

const LINKER_SYSTEM_PROMPT: &str = r#"You label edges in a course-document knowledge graph.

You will receive:

1. A "documents" array. Each document has:
   - "id": opaque identifier (use this verbatim when referring to docs)
   - "kind": one of "lecture", "lecture_transcript", "reading", "tutorial_exercise", "assignment_brief", "sample_solution", "lab_brief", "exam", "syllabus", "unknown"
   - "classifier_rationale": short note from the per-document classifier
   - "excerpt": the first few hundred characters of the document text

2. A "candidates" array of pre-selected pairs that are SEMANTICALLY similar
   in content (high embedding cosine similarity). Each candidate has:
   - "src_id", "dst_id": the two document ids
   - "similarity": cosine similarity in [0, 1] of the docs' content embeddings

You are NOT given filenames. Filenames in real courses are unreliable
(stale templates, copy/paste, names that contradict the content).
Decide everything from the kind, the classifier_rationale, the excerpt,
and the embedding similarity.

For EACH candidate pair, decide whether there is a typed relation between
the two documents. You may also decide there is NONE (omit it from output).

Possible relations:

- "solution_of": src is the sample_solution; dst is the assignment_brief /
  lab_brief / exam it answers. Requires one side to have
  kind=sample_solution AND the other to have kind in
  {assignment_brief, lab_brief, exam}. Use when the solution's excerpt
  references the same problem the assignment poses (numbers, function
  names, dataset, scenario). High embedding similarity is a strong
  hint but the kind-pair check is the gate. Confidence ~0.85+ when
  the alignment is unambiguous; lower if the solution might cover
  multiple assignments.

- "part_of_unit": both documents belong to the same week / module /
  chapter / topic / unit. Emit when the two excerpts plainly discuss
  the same SPECIFIC subtopic in a way that suggests they're paired
  course material -- not just "both touch on inheritance somewhere".
  Examples that SHOULD emit a part_of_unit edge (with confidence ~0.8):
    * Two assignment_briefs that pose the same task in different
      formats (e.g. a PDF and an HTML version of the same exercise).
    * A reading + an assignment_brief / tutorial_exercise that asks
      the student to apply the same specific concept the reading
      introduces.
    * A lecture overview / section summary + the lecture itself
      covering the same content.
    * An assignment_brief + the page describing its submission rules
      ("how to hand in lab 2").
  Examples that should NOT emit:
    * Two sequential lectures on related themes (e.g. "Arv I" and
      "Arv II"). They belong to ADJACENT units; this graph captures
      same-unit, not topical-neighbours.
    * Two docs that happen to share a course-wide topic word
      ("inheritance", "loops", "OO"). That's the whole course's
      vocabulary, not a unit signal.
    * Generic syllabus / instructions material that mentions a topic
      in passing.

Calibration: pairs in the candidate list ALREADY passed an embedding
similarity threshold (>=0.45 cosine), so they share substantive
content. Your job is to decide which kind of relation each pair is,
not whether ANY relation exists -- in a typical 30-50 doc DSV course
we expect 5-30% of candidate pairs to map to a real edge. If you're
labelling 0 of N candidates, you're being too conservative.

Confidence guidance:
  * 0.85+ : the pairing is unambiguous (e.g. solution clearly answers
    the assignment, two docs are obviously different formats of the
    same exercise).
  * 0.7-0.84 : confident but not certain. Use freely.
  * 0.5-0.69 : a reasonable guess based on visible content overlap.
    Emit these too -- the server floor is 0.5 and filters edges
    further by embedding similarity, so a moderate confidence is
    fine.
  * <0.5 : skip. Random guesses hurt.

Output format -- JSON, nothing else:
{
  "edges": [
    {
      "src_id": <id from candidate>,
      "dst_id": <id from candidate>,
      "relation": "solution_of" | "part_of_unit",
      "confidence": float in [0, 1],
      "rationale": short specific string. CITE concrete evidence visible
        in the excerpts: a specific phrase appearing in both, a problem
        statement in one and its answer in the other, a concept the
        first introduces and the second exercises. Do NOT invent shared
        tokens. Do NOT cite filenames -- you don't have them.
    }
  ]
}

HARD RULES:
- ONLY emit edges for pairs in the candidates list. Do NOT propose pairs
  that are not candidates.
- AT MOST ONE edge per candidate pair.
- Do NOT emit self-loops (same src_id and dst_id).
- DO NOT emit any edge for pairs whose excerpts are essentially identical
  (likely duplicate uploads of the same document). These belong nowhere.
- Confidence < 0.6 will be dropped server-side; aim higher or skip the
  edge entirely.
- For solution_of, prefer concrete content alignment (problem -> solution)
  over similarity alone.
- For part_of_unit, you need both content overlap AND a STRUCTURAL
  relationship visible in the excerpts. Topic-word co-occurrence is not
  a structural relationship.

Reply with the JSON object only. No prose."#;

#[derive(Debug, Clone)]
pub struct ProposedEdge {
    pub src_id: Uuid,
    pub dst_id: Uuid,
    pub relation: String,
    pub confidence: f32,
    pub rationale: Option<String>,
}

#[derive(Debug)]
pub struct LinkerOutput {
    pub edges: Vec<ProposedEdge>,
    /// Docs the linker considered (for telemetry / log lines).
    pub considered: usize,
}

/// Inputs the linker needs that aren't on `DocumentRow` itself.
///
/// Note what's NOT here: no fastembed, no OpenAI key, no docs_path.
/// The linker reads everything it needs (chunk text for excerpts,
/// chunk vectors for the lazy-pooled-embedding backfill) from
/// Qdrant. We never re-read PDFs from disk or call an embedder
/// during a relink -- the ingest pipeline has already done that work
/// and persisted it to the doc row + Qdrant.
pub struct LinkContext<'a> {
    pub http: &'a reqwest::Client,
    pub api_key: &'a str,
    pub db: &'a PgPool,
    pub qdrant: &'a Arc<Qdrant>,
}

/// True iff the (src, dst, relation) triple has been vetoed by a teacher.
/// `part_of_unit` is undirected; the linker normalises src < dst before
/// upsert, but the candidate set sees pairs in arbitrary order, so we
/// check both orderings here.
fn is_rejected(rejected: &HashSet<RejectedPairKey>, a: Uuid, b: Uuid, relation: &str) -> bool {
    rejected.contains(&RejectedPairKey {
        src_doc_id: a,
        dst_doc_id: b,
        relation: relation.to_string(),
    }) || rejected.contains(&RejectedPairKey {
        src_doc_id: b,
        dst_doc_id: a,
        relation: relation.to_string(),
    })
}

/// Pair-level test: should the linker consider this pair at all? Used
/// to drop candidates BEFORE the LLM call -- if both relation types
/// for a pair have been vetoed, there's no point asking the model.
fn pair_fully_rejected(rejected: &HashSet<RejectedPairKey>, a: Uuid, b: Uuid) -> bool {
    is_rejected(rejected, a, b, "solution_of")
        && is_rejected(rejected, a, b, "part_of_unit")
        // solution_of is directional, so we also need to check b->a.
        && is_rejected(rejected, b, a, "solution_of")
}

/// Run the cross-doc linker over a course's classified documents.
///
/// Pipeline:
///   1. **Embeddings**: every doc has a pooled embedding from the
///      ingest pipeline. For docs missing one (older data), lazily
///      backfill by re-embedding the doc text. Embeddings are
///      L2-normalised so cosine similarity is just a dot product.
///   2. **Candidate generation**: per doc, top-K most similar OTHER
///      docs above `MIN_EMBEDDING_SIMILARITY`. PURE embedding-based --
///      no filename heuristics.
///   3. **Content excerpts**: for each doc that appears in any
///      candidate, read the first EXCERPT_CHARS from disk so the LLM
///      grounds its decisions in actual content.
///   4. **LLM labelling**: single Cerebras call labels each
///      candidate as solution_of / part_of_unit / nothing.
///   5. **Post-filters**: confidence floor, similarity floors per
///      relation type, duplicate detection (cosine ~ 1), teacher
///      vetoes.
pub async fn link_course(
    ctx: &LinkContext<'_>,
    course_id: Uuid,
    docs: &[DocumentRow],
) -> Result<LinkerOutput, String> {
    let classified: Vec<&DocumentRow> = docs
        .iter()
        .filter(|d| d.kind.is_some() && d.status == "ready")
        .collect();

    if classified.len() < 2 {
        return Ok(LinkerOutput {
            edges: Vec::new(),
            considered: classified.len(),
        });
    }

    if ctx.api_key.is_empty() {
        // Dev / test env without CEREBRAS_API_KEY. Skip rather than
        // burn time on a guaranteed-401 call.
        return Ok(LinkerOutput {
            edges: Vec::new(),
            considered: classified.len(),
        });
    }

    let truncated_owned: Vec<&DocumentRow> = if classified.len() > MAX_DOCS_PER_CALL {
        tracing::warn!(
            "linker: course has {} classified docs, capping linker input at {} (V1 doesn't paginate)",
            classified.len(),
            MAX_DOCS_PER_CALL,
        );
        classified.into_iter().take(MAX_DOCS_PER_CALL).collect()
    } else {
        classified
    };
    let truncated: &[&DocumentRow] = &truncated_owned;

    // Load teacher-vetoed pairs ONCE per linker pass. Cheap query;
    // saves us asking the model about pairs we'd just drop anyway.
    let rejected =
        minerva_db::queries::document_relations::rejected_pairs_for_course(ctx.db, course_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    "linker: failed to load rejected pairs ({}); proceeding without veto list",
                    e
                );
                HashSet::new()
            });
    if !rejected.is_empty() {
        tracing::info!(
            "linker: course {} has {} teacher-vetoed pair(s) -- those will be skipped",
            course_id,
            rejected.len(),
        );
    }

    // Step 1: gather pooled embeddings, lazily backfilling any that
    // are missing.
    let embeddings: HashMap<Uuid, Vec<f32>> = gather_embeddings(ctx, course_id, truncated).await?;

    // Step 2: embedding-similarity candidates (the only candidate
    // channel -- no filename heuristics).
    let mut candidates: HashSet<(Uuid, Uuid)> = HashSet::new();
    let similarity_by_pair: HashMap<(Uuid, Uuid), f32> = build_similarity_matrix(
        truncated,
        &embeddings,
        EMBEDDING_TOP_K,
        MIN_EMBEDDING_SIMILARITY,
        &mut candidates,
    );

    // Drop probable-duplicate pairs (cosine ~ 1) before we even
    // bother sending them to the model -- they're not "in the same
    // unit", they're the same document re-uploaded. Logged as DEBUG
    // (per-pair) plus one INFO summary line so a course with N
    // duplicate uploads doesn't spam N lines into the log.
    let candidates_before = candidates.len();
    candidates.retain(|pair| {
        let sim = similarity_by_pair.get(pair).copied().unwrap_or(0.0);
        if sim >= DUPLICATE_SIMILARITY {
            tracing::debug!(
                "linker: dropping likely-duplicate candidate {:?}<->{:?} (similarity {:.3})",
                pair.0,
                pair.1,
                sim
            );
            return false;
        }
        true
    });
    let dup_dropped = candidates_before - candidates.len();
    if dup_dropped > 0 {
        tracing::info!(
            "linker: dropped {} duplicate-content candidate pair(s) (cosine >= {:.3})",
            dup_dropped,
            DUPLICATE_SIMILARITY,
        );
    }

    // Drop pairs where every possible relation has been vetoed by a
    // teacher -- no LLM call needed. Pairs where SOME relations are
    // vetoed still go to the model but get filtered post-hoc.
    candidates.retain(|pair| {
        if pair_fully_rejected(&rejected, pair.0, pair.1) {
            tracing::info!(
                "linker: dropping fully-rejected pair {:?}<->{:?}",
                pair.0,
                pair.1,
            );
            return false;
        }
        true
    });

    if candidates.is_empty() {
        return Ok(LinkerOutput {
            edges: Vec::new(),
            considered: truncated.len(),
        });
    }

    // Step 3: content excerpts for every doc in any candidate pair.
    let mut docs_in_candidates: HashSet<Uuid> = HashSet::new();
    for (a, b) in &candidates {
        docs_in_candidates.insert(*a);
        docs_in_candidates.insert(*b);
    }
    let excerpts: HashMap<Uuid, String> =
        load_excerpts(ctx.qdrant, course_id, truncated, &docs_in_candidates).await;

    // Step 4: LLM labels candidates.
    let edges = call_linker_llm(
        ctx.http,
        ctx.api_key,
        truncated,
        &candidates,
        &similarity_by_pair,
        &excerpts,
    )
    .await?;
    // Surface the raw LLM edge count separately from the filtered
    // count, so an "0 edges written" log line is unambiguous between
    // "the model emitted nothing" and "the model emitted N but every
    // one tripped a post-filter". Both modes have shipped as bugs
    // before; explicit counts make them debuggable from logs alone.
    tracing::info!(
        "linker: course {} -- LLM proposed {} edge(s) over {} candidate pair(s)",
        course_id,
        edges.len(),
        candidates.len(),
    );

    // Step 5: post-filters.
    let mut kept = Vec::with_capacity(edges.len());
    let mut dropped_sim = 0usize;
    let mut dropped_rejected = 0usize;
    for edge in edges {
        // Teacher veto wins over the model. We already pre-filtered
        // fully-rejected pairs from candidates, but a partial veto
        // (e.g. solution_of vetoed but part_of_unit not) still let
        // the pair through; drop the specific vetoed relation here.
        if is_rejected(&rejected, edge.src_id, edge.dst_id, &edge.relation) {
            tracing::info!(
                "linker: post-filter dropped {} {:?}<->{:?} (teacher-vetoed)",
                edge.relation,
                edge.src_id,
                edge.dst_id,
            );
            dropped_rejected += 1;
            continue;
        }

        let pair_key = if edge.src_id < edge.dst_id {
            (edge.src_id, edge.dst_id)
        } else {
            (edge.dst_id, edge.src_id)
        };
        let sim = similarity_by_pair.get(&pair_key).copied().unwrap_or(0.0);

        // Relation-specific similarity floor.
        let floor = match edge.relation.as_str() {
            "solution_of" => MIN_SOLUTION_OF_SIMILARITY,
            _ => MIN_EMBEDDING_SIMILARITY,
        };

        if sim < floor {
            tracing::info!(
                "linker: post-filter dropped {} {:?}<->{:?} (similarity {:.3} below {:.2})",
                edge.relation,
                edge.src_id,
                edge.dst_id,
                sim,
                floor,
            );
            dropped_sim += 1;
            continue;
        }

        kept.push(edge);
    }
    if dropped_sim > 0 || dropped_rejected > 0 {
        tracing::info!(
            "linker: post-filter summary -- {} similarity floor, {} teacher-vetoed",
            dropped_sim,
            dropped_rejected,
        );
    }

    Ok(LinkerOutput {
        edges: kept,
        considered: truncated.len(),
    })
}

/// Pull pooled embeddings from `DocumentRow` where available, fall
/// back to re-embedding on the fly for older docs whose
/// pooled_embedding is NULL. Lazy backfill writes the result back to
/// the DB so subsequent link calls don't repeat the work.
/// Gather pooled embeddings for every doc the linker is going to
/// consider. Two paths:
///
/// 1. **Hot path**: `documents.pooled_embedding` is set by the ingest
///    pipeline at upload time (mean-pool + L2-normalise of all chunk
///    vectors), so this is just a clone out of the DB row -- no
///    network, no recompute.
///
/// 2. **Cold path** (only when `pooled_embedding IS NULL`, i.e. data
///    that predates the column or a transient ingest failure): scroll
///    Qdrant for the doc's existing chunk VECTORS and mean-pool them.
///    No file re-read, no embedder call -- vectors are already there.
///    The result is persisted back so the next relink hits the hot
///    path.
///
/// For docs without an entry in either source (e.g. a `sample_solution`
/// uploaded before pooled_embedding was added: chunks weren't upserted
/// to Qdrant AND the column was NULL), we just skip them -- they won't
/// participate in the similarity matrix this run, but the next ingest
/// of a new doc will repopulate.
async fn gather_embeddings(
    ctx: &LinkContext<'_>,
    course_id: Uuid,
    docs: &[&DocumentRow],
) -> Result<HashMap<Uuid, Vec<f32>>, String> {
    let mut out: HashMap<Uuid, Vec<f32>> = HashMap::new();
    let collection = format!("course_{}", course_id);
    let collection_exists = ctx
        .qdrant
        .collection_exists(&collection)
        .await
        .unwrap_or(false);
    for doc in docs {
        if let Some(emb) = &doc.pooled_embedding {
            if !emb.is_empty() {
                out.insert(doc.id, emb.clone());
                continue;
            }
        }
        if !collection_exists {
            continue;
        }
        match pool_from_qdrant(ctx.qdrant, &collection, doc.id).await {
            Ok(Some(emb)) => {
                let _ = minerva_db::queries::documents::set_pooled_embedding(ctx.db, doc.id, &emb)
                    .await;
                out.insert(doc.id, emb);
            }
            Ok(None) => {
                tracing::debug!(
                    "linker: no embedding available for doc {} -- skipping similarity channel",
                    doc.id,
                );
            }
            Err(e) => {
                tracing::warn!(
                    "linker: qdrant pool fallback failed for doc {}: {}",
                    doc.id,
                    e
                );
            }
        }
    }
    Ok(out)
}

/// Cold-path: scroll the doc's existing chunk vectors out of Qdrant and
/// mean-pool them. Replaces the old "re-extract PDF, re-chunk, re-embed"
/// path -- the vectors are already there, just pool them. Returns None
/// if the collection has no chunks for this doc.
async fn pool_from_qdrant(
    qdrant: &Qdrant,
    collection: &str,
    doc_id: Uuid,
) -> Result<Option<Vec<f32>>, String> {
    let filter = Filter::must([Condition::matches("document_id", doc_id.to_string())]);
    let result = qdrant
        .scroll(
            ScrollPointsBuilder::new(collection)
                .filter(filter)
                .with_payload(false)
                .with_vectors(true)
                // Cap at 1000 chunks per doc -- our chunker's default
                // produces far fewer than this even on the biggest
                // course material; this is a sanity ceiling.
                .limit(1000),
        )
        .await
        .map_err(|e| format!("qdrant scroll failed: {}", e))?;

    let vectors: Vec<Vec<f32>> = result
        .result
        .into_iter()
        .filter_map(|p| {
            // VectorOutput went through a deprecation cycle in 1.16:
            // `data` is gone; the supported path is `into_vector()`
            // which returns the typed `Vector` enum we then match on
            // for the Dense variant. Sparse/multi-dense aren't a
            // shape this collection produces (we only ever upsert
            // single dense vectors via the ingest pipeline), so we
            // safely ignore them.
            let v = p.vectors?;
            match v.vectors_options? {
                qdrant_client::qdrant::vectors_output::VectorsOptions::Vector(vec) => {
                    match vec.into_vector() {
                        qdrant_client::qdrant::vector_output::Vector::Dense(d) => Some(d.data),
                        _ => None,
                    }
                }
                _ => None,
            }
        })
        .collect();

    Ok(mean_pool_normalized(&vectors))
}

/// Mean-pool + L2-normalise. Same shape as the ingest pipeline's
/// version but pulled into the linker module so backfill doesn't need
/// to re-export from minerva-ingest.
fn mean_pool_normalized(embeddings: &[Vec<f32>]) -> Option<Vec<f32>> {
    if embeddings.is_empty() {
        return None;
    }
    let dim = embeddings[0].len();
    if dim == 0 {
        return None;
    }
    let mut sum = vec![0.0f32; dim];
    for e in embeddings {
        if e.len() != dim {
            return None;
        }
        for (i, v) in e.iter().enumerate() {
            sum[i] += v;
        }
    }
    let n = embeddings.len() as f32;
    for v in sum.iter_mut() {
        *v /= n;
    }
    let norm: f32 = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in sum.iter_mut() {
            *v /= norm;
        }
    }
    Some(sum)
}

/// Cosine similarity for L2-normalised vectors is just the dot
/// product. Returns 0 if either vector is missing or empty.
fn cosine_normalised(a: Option<&Vec<f32>>, b: Option<&Vec<f32>>) -> f32 {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) if a.len() == b.len() && !a.is_empty() => (a, b),
        _ => return 0.0,
    };
    let mut dot = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
    }
    dot.clamp(-1.0, 1.0)
}

/// Build a (sorted-pair-key, similarity) map and populate the
/// candidates set with the top-K most-similar OTHER doc per source
/// doc, gated by `floor`. Returns the full pair-similarity map so the
/// post-filters can reuse the numbers without recomputing.
fn build_similarity_matrix(
    docs: &[&DocumentRow],
    embeddings: &HashMap<Uuid, Vec<f32>>,
    top_k: usize,
    floor: f32,
    candidates: &mut HashSet<(Uuid, Uuid)>,
) -> HashMap<(Uuid, Uuid), f32> {
    let mut sims: HashMap<(Uuid, Uuid), f32> = HashMap::new();
    for (i, di) in docs.iter().enumerate() {
        let mut neighbors: Vec<(Uuid, f32)> = Vec::with_capacity(docs.len());
        for (j, dj) in docs.iter().enumerate() {
            if i == j {
                continue;
            }
            let s = cosine_normalised(embeddings.get(&di.id), embeddings.get(&dj.id));
            // Cache for every pair we compute, regardless of floor.
            let key = if di.id < dj.id {
                (di.id, dj.id)
            } else {
                (dj.id, di.id)
            };
            sims.entry(key).or_insert(s);
            if s >= floor {
                neighbors.push((dj.id, s));
            }
        }
        neighbors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (other, _) in neighbors.into_iter().take(top_k) {
            let pair = if di.id < other {
                (di.id, other)
            } else {
                (other, di.id)
            };
            candidates.insert(pair);
        }
    }
    sims
}

/// Read the first `EXCERPT_CHARS` of each in-candidate doc's text
/// from disk. URL/awaiting-transcript/unsupported docs may have no
/// readable file -- those simply get an empty excerpt.
///
/// **Concurrency**: each doc's text extraction runs on the blocking
/// thread pool via `spawn_blocking`, fanned out concurrently via
/// `join_all`. Sync PDF parsing on the main runtime previously blocked
/// the worker thread for hundreds of ms per doc -- a large course
/// (~30 docs) could stall request handling for several seconds during
/// every relink. Now the runtime thread just awaits a join_all of
/// blocking-pool tasks; HTTP handlers stay responsive.
/// Pull a content excerpt for each in-candidate doc out of Qdrant.
/// We grab a few chunks per doc (sorted by chunk_index ascending) and
/// concatenate their text up to `EXCERPT_CHARS`. No file I/O, no PDF
/// re-parsing -- the chunks are already there from ingest.
///
/// Sample-solution docs aren't in Qdrant (they're embedded into the
/// doc-row pooled vector but their chunks are deliberately excluded
/// from retrieval). Those simply get an empty excerpt; the linker
/// then leans on `kind=sample_solution` + the assignment doc's
/// excerpt + embedding similarity to decide solution_of edges.
async fn load_excerpts(
    qdrant: &Qdrant,
    course_id: Uuid,
    docs: &[&DocumentRow],
    only_for: &HashSet<Uuid>,
) -> HashMap<Uuid, String> {
    let collection = format!("course_{}", course_id);
    if !qdrant.collection_exists(&collection).await.unwrap_or(false) {
        // Collection hasn't been created yet (e.g. brand-new course
        // with all-sample_solution uploads): nothing to scroll.
        return docs
            .iter()
            .filter(|d| only_for.contains(&d.id))
            .map(|d| (d.id, String::new()))
            .collect();
    }

    let target_ids: Vec<Uuid> = docs
        .iter()
        .filter(|d| only_for.contains(&d.id))
        .map(|d| d.id)
        .collect();

    // Per-doc Qdrant scroll, fanned out concurrently. Each scroll is
    // a single round-trip pulling at most a handful of chunks, so we
    // can launch them all in parallel without overwhelming Qdrant.
    // The async blocks borrow `qdrant` for the duration of join_all,
    // and clone the collection name per-task (cheap -- a small
    // String, no allocations on the hot path of the await chain).
    let tasks: Vec<_> = target_ids
        .into_iter()
        .map(|id| {
            let collection = collection.clone();
            async move {
                let filter = Filter::must([Condition::matches("document_id", id.to_string())]);
                let scroll = qdrant
                    .scroll(
                        ScrollPointsBuilder::new(&collection)
                            .filter(filter)
                            .with_payload(true)
                            .with_vectors(false)
                            // Pull a small head: enough to assemble
                            // EXCERPT_CHARS even if the first chunk is
                            // unusually small. Chunks come back in
                            // unspecified order so we sort client-side.
                            .limit(5),
                    )
                    .await;
                let mut chunks: Vec<(i64, String)> = match scroll {
                    Ok(r) => r
                        .result
                        .iter()
                        .filter_map(|p| {
                            let text = payload_string(&p.payload, "text")?;
                            let idx = payload_int(&p.payload, "chunk_index").unwrap_or(0);
                            Some((idx, text))
                        })
                        .collect(),
                    Err(e) => {
                        tracing::debug!("linker: excerpt scroll failed for doc {}: {}", id, e);
                        return (id, String::new());
                    }
                };
                chunks.sort_by_key(|(i, _)| *i);
                let mut buf = String::with_capacity(EXCERPT_CHARS + 32);
                for (_, text) in chunks {
                    if buf.chars().count() >= EXCERPT_CHARS {
                        break;
                    }
                    if !buf.is_empty() {
                        buf.push(' ');
                    }
                    // Normalise whitespace as we go (some chunkers
                    // preserve newlines / tabs that don't help the LLM).
                    let normalised: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    buf.push_str(&normalised);
                }
                let excerpt: String = buf.chars().take(EXCERPT_CHARS).collect();
                (id, excerpt)
            }
        })
        .collect();

    let results = futures::future::join_all(tasks).await;
    results.into_iter().collect()
}

/// Build the LLM prompt body and call Cerebras. Returns parsed
/// edges; empty on no-edges, error on JSON or transport failure.
async fn call_linker_llm(
    http: &reqwest::Client,
    api_key: &str,
    docs: &[&DocumentRow],
    candidates: &HashSet<(Uuid, Uuid)>,
    similarity: &HashMap<(Uuid, Uuid), f32>,
    excerpts: &HashMap<Uuid, String>,
) -> Result<Vec<ProposedEdge>, String> {
    // Documents block: only include docs that appear in some candidate.
    // No filename in the payload -- filenames are unreliable and the
    // linker is content-only.
    let mut in_candidates: HashSet<Uuid> = HashSet::new();
    for (a, b) in candidates {
        in_candidates.insert(*a);
        in_candidates.insert(*b);
    }
    let docs_array: Vec<serde_json::Value> = docs
        .iter()
        .filter(|d| in_candidates.contains(&d.id))
        .map(|d| {
            serde_json::json!({
                "id": d.id.to_string(),
                "kind": d.kind.as_deref().unwrap_or("unknown"),
                "classifier_rationale": d.kind_rationale.as_deref().unwrap_or(""),
                "excerpt": excerpts.get(&d.id).cloned().unwrap_or_default(),
            })
        })
        .collect();

    // Candidates block: stable sort so prompt-cache hits are more
    // likely on re-runs of the same course.
    let mut sorted_candidates: Vec<(Uuid, Uuid)> = candidates.iter().copied().collect();
    sorted_candidates.sort();
    let candidates_array: Vec<serde_json::Value> = sorted_candidates
        .iter()
        .map(|(a, b)| {
            let sim = similarity.get(&(*a, *b)).copied().unwrap_or(0.0);
            serde_json::json!({
                "src_id": a.to_string(),
                "dst_id": b.to_string(),
                "similarity": sim,
            })
        })
        .collect();

    let user_payload = serde_json::json!({
        "documents": docs_array,
        "candidates": candidates_array,
    });

    let body = serde_json::json!({
        "model": LINKER_MODEL,
        "temperature": 0.0,
        "reasoning_effort": "medium",
        "messages": [
            { "role": "system", "content": LINKER_SYSTEM_PROMPT },
            { "role": "user", "content": user_payload.to_string() },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "course_kg_edges",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["edges"],
                    "properties": {
                        "edges": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["src_id", "dst_id", "relation", "confidence", "rationale"],
                                "properties": {
                                    "src_id": { "type": "string" },
                                    "dst_id": { "type": "string" },
                                    "relation": {
                                        "type": "string",
                                        "enum": ["solution_of", "part_of_unit"],
                                    },
                                    "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                                    "rationale": { "type": "string" },
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let response = cerebras_request_with_retry(http, api_key, &body).await?;
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("linker: response not JSON: {e}"))?;
    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| format!("linker: missing message content; got: {payload}"))?;

    let id_set: HashSet<Uuid> = docs.iter().map(|d| d.id).collect();
    let edges = parse_edges(raw, &id_set)?;

    // Operator visibility: when the model returns nothing, dump the
    // raw response (truncated) so we can tell whether it was a
    // genuinely empty `{"edges": []}`, a refusal narrative, or
    // something else weird. This is the single most useful signal
    // for tuning the prompt -- without it "0 edges" is opaque.
    if edges.is_empty() {
        let preview: String = raw.chars().take(500).collect();
        tracing::info!(
            "linker: model returned 0 edges over {} candidates -- raw response head: {}",
            candidates.len(),
            preview,
        );
    }

    // Ignore any edge the model emitted for a non-candidate pair.
    Ok(edges
        .into_iter()
        .filter(|e| {
            let key = if e.src_id < e.dst_id {
                (e.src_id, e.dst_id)
            } else {
                (e.dst_id, e.src_id)
            };
            candidates.contains(&key)
        })
        .collect())
}

fn parse_edges(
    raw: &str,
    valid_ids: &std::collections::HashSet<Uuid>,
) -> Result<Vec<ProposedEdge>, String> {
    let trimmed = raw.trim();
    let json_str = if let Some(stripped) = trimmed.strip_prefix("```json") {
        stripped
            .trim_start()
            .strip_suffix("```")
            .unwrap_or(stripped)
            .trim()
    } else if let Some(stripped) = trimmed.strip_prefix("```") {
        stripped
            .trim_start()
            .strip_suffix("```")
            .unwrap_or(stripped)
            .trim()
    } else {
        trimmed
    };

    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("linker: invalid JSON: {e}"))?;

    let raw_edges = parsed["edges"]
        .as_array()
        .ok_or_else(|| "linker: missing 'edges' array".to_string())?;

    let mut out: Vec<ProposedEdge> = Vec::with_capacity(raw_edges.len());
    let mut dedup: HashMap<(Uuid, Uuid, String), f32> = HashMap::new();
    // Per-reason drop tally so the operator can see why a "0 edges
    // written" run actually produced 0 -- model emitting low-conf
    // guesses, hallucinated IDs, malformed relations are all
    // observably distinct here.
    let mut dropped_low_conf = 0usize;
    let mut dropped_self_loop = 0usize;
    let mut dropped_hallucinated = 0usize;
    let mut dropped_bad_relation = 0usize;

    for e in raw_edges {
        let src_id = match e["src_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) {
            Some(id) => id,
            None => continue,
        };
        let dst_id = match e["dst_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) {
            Some(id) => id,
            None => continue,
        };
        if src_id == dst_id {
            dropped_self_loop += 1;
            continue;
        }
        if !valid_ids.contains(&src_id) || !valid_ids.contains(&dst_id) {
            // Model hallucinated an id we never sent in; drop.
            dropped_hallucinated += 1;
            continue;
        }
        let relation = match e["relation"].as_str() {
            Some(r @ ("solution_of" | "part_of_unit")) => r.to_string(),
            _ => {
                dropped_bad_relation += 1;
                continue;
            }
        };
        let confidence = e["confidence"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
        if confidence < MIN_EDGE_CONFIDENCE {
            dropped_low_conf += 1;
            continue;
        }
        let rationale = e["rationale"].as_str().map(str::to_string);

        // For undirected `part_of_unit`, normalise direction by id
        // so duplicates collapse on the unique constraint.
        let (src, dst) = if relation == "part_of_unit" && src_id > dst_id {
            (dst_id, src_id)
        } else {
            (src_id, dst_id)
        };

        let key = (src, dst, relation.clone());
        let entry = dedup.entry(key).or_insert(0.0);
        if confidence > *entry {
            *entry = confidence;
            out.retain(|edge| {
                !(edge.src_id == src && edge.dst_id == dst && edge.relation == relation)
            });
            out.push(ProposedEdge {
                src_id: src,
                dst_id: dst,
                relation,
                confidence,
                rationale,
            });
        }
    }

    if dropped_low_conf + dropped_self_loop + dropped_hallucinated + dropped_bad_relation > 0 {
        tracing::info!(
            "linker: parse_edges -- raw {}, kept {}, dropped low_conf={} self_loop={} hallucinated={} bad_relation={}",
            raw_edges.len(),
            out.len(),
            dropped_low_conf,
            dropped_self_loop,
            dropped_hallucinated,
            dropped_bad_relation,
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    #[test]
    fn parses_well_formed_response() {
        let a = id(1);
        let b = id(2);
        let raw = format!(
            r#"{{"edges":[{{"src_id":"{a}","dst_id":"{b}","relation":"solution_of","confidence":0.9,"rationale":"Restates the problem and provides the answer."}}]}}"#
        );
        let mut valid = std::collections::HashSet::new();
        valid.insert(a);
        valid.insert(b);
        let edges = parse_edges(&raw, &valid).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, a);
        assert_eq!(edges[0].dst_id, b);
        assert_eq!(edges[0].relation, "solution_of");
        assert!((edges[0].confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn drops_low_confidence_edges() {
        let a = id(1);
        let b = id(2);
        let raw = format!(
            r#"{{"edges":[{{"src_id":"{a}","dst_id":"{b}","relation":"part_of_unit","confidence":0.4,"rationale":"weak"}}]}}"#
        );
        let mut valid = std::collections::HashSet::new();
        valid.insert(a);
        valid.insert(b);
        let edges = parse_edges(&raw, &valid).unwrap();
        assert!(edges.is_empty());
    }

    #[test]
    fn drops_self_loops() {
        let a = id(1);
        let raw = format!(
            r#"{{"edges":[{{"src_id":"{a}","dst_id":"{a}","relation":"solution_of","confidence":0.9,"rationale":"x"}}]}}"#
        );
        let mut valid = std::collections::HashSet::new();
        valid.insert(a);
        let edges = parse_edges(&raw, &valid).unwrap();
        assert!(edges.is_empty());
    }

    #[test]
    fn drops_hallucinated_ids() {
        let a = id(1);
        let b = id(2);
        let c = id(99); // not in valid set
        let raw = format!(
            r#"{{"edges":[{{"src_id":"{a}","dst_id":"{c}","relation":"solution_of","confidence":0.9,"rationale":"x"}},{{"src_id":"{a}","dst_id":"{b}","relation":"solution_of","confidence":0.85,"rationale":"y"}}]}}"#
        );
        let mut valid = std::collections::HashSet::new();
        valid.insert(a);
        valid.insert(b);
        let edges = parse_edges(&raw, &valid).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].dst_id, b);
    }

    #[test]
    fn normalises_part_of_unit_direction() {
        let a = id(5);
        let b = id(2); // smaller -- should become src
        let raw = format!(
            r#"{{"edges":[{{"src_id":"{a}","dst_id":"{b}","relation":"part_of_unit","confidence":0.85,"rationale":"shared topic"}}]}}"#
        );
        let mut valid = std::collections::HashSet::new();
        valid.insert(a);
        valid.insert(b);
        let edges = parse_edges(&raw, &valid).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, b);
        assert_eq!(edges[0].dst_id, a);
    }

    #[test]
    fn solution_of_direction_preserved() {
        // solution_of is directional; do NOT swap by id ordering.
        let solution = id(7);
        let assignment = id(3);
        let raw = format!(
            r#"{{"edges":[{{"src_id":"{solution}","dst_id":"{assignment}","relation":"solution_of","confidence":0.92,"rationale":"x"}}]}}"#
        );
        let mut valid = std::collections::HashSet::new();
        valid.insert(solution);
        valid.insert(assignment);
        let edges = parse_edges(&raw, &valid).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, solution);
        assert_eq!(edges[0].dst_id, assignment);
    }

    // ── Embedding similarity ──────────────────────────────────────

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_normalised(Some(&a), Some(&b)).abs() < 1e-6);
    }

    #[test]
    fn cosine_identical_is_one() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_normalised(Some(&a), Some(&b)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_handles_dim_mismatch_and_missing() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_normalised(Some(&a), Some(&b)), 0.0);
        assert_eq!(cosine_normalised(None, Some(&b)), 0.0);
        assert_eq!(cosine_normalised(Some(&a), None), 0.0);
    }

    #[test]
    fn build_similarity_matrix_picks_top_k_above_floor() {
        fn mk(id: u8) -> DocumentRow {
            DocumentRow {
                id: Uuid::from_bytes([id; 16]),
                course_id: Uuid::nil(),
                filename: format!("doc{}.pdf", id),
                mime_type: "application/pdf".into(),
                size_bytes: 0,
                status: "ready".into(),
                chunk_count: Some(1),
                error_msg: None,
                uploaded_by: Uuid::nil(),
                displayable: true,
                created_at: chrono::Utc::now(),
                processed_at: None,
                source_url: None,
                kind: Some("lecture".into()),
                kind_confidence: Some(0.9),
                kind_rationale: None,
                kind_locked_by_teacher: false,
                classified_at: None,
                pooled_embedding: None,
            }
        }
        let docs_owned = [mk(1), mk(2), mk(3), mk(4)];
        let docs: Vec<&DocumentRow> = docs_owned.iter().collect();
        let mut embeddings: HashMap<Uuid, Vec<f32>> = HashMap::new();
        // doc1 and doc2 are almost identical, doc3 is orthogonal to both,
        // doc4 is identical to doc1 (so doc1<->doc4 sim = 1.0).
        embeddings.insert(docs[0].id, vec![1.0, 0.0]);
        embeddings.insert(docs[1].id, vec![0.99, 0.0]);
        embeddings.insert(docs[2].id, vec![0.0, 1.0]);
        embeddings.insert(docs[3].id, vec![1.0, 0.0]);

        let mut candidates = HashSet::new();
        let sims = build_similarity_matrix(&docs, &embeddings, 2, 0.5, &mut candidates);

        // Every pair has a similarity entry.
        assert_eq!(sims.len(), 6);

        // doc1<->doc2 above 0.5 -> candidate.
        let key12 = if docs[0].id < docs[1].id {
            (docs[0].id, docs[1].id)
        } else {
            (docs[1].id, docs[0].id)
        };
        assert!(candidates.contains(&key12));
        // doc1<->doc4 above 0.5 -> candidate.
        let key14 = if docs[0].id < docs[3].id {
            (docs[0].id, docs[3].id)
        } else {
            (docs[3].id, docs[0].id)
        };
        assert!(candidates.contains(&key14));
        // doc1<->doc3 below 0.5 -> NOT a candidate.
        let key13 = if docs[0].id < docs[2].id {
            (docs[0].id, docs[2].id)
        } else {
            (docs[2].id, docs[0].id)
        };
        assert!(!candidates.contains(&key13));
    }

    #[test]
    fn dedups_repeated_edges_keeping_highest_confidence() {
        let a = id(1);
        let b = id(2);
        let raw = format!(
            r#"{{"edges":[
                {{"src_id":"{a}","dst_id":"{b}","relation":"solution_of","confidence":0.7,"rationale":"first"}},
                {{"src_id":"{a}","dst_id":"{b}","relation":"solution_of","confidence":0.95,"rationale":"second"}}
            ]}}"#
        );
        let mut valid = std::collections::HashSet::new();
        valid.insert(a);
        valid.insert(b);
        let edges = parse_edges(&raw, &valid).unwrap();
        assert_eq!(edges.len(), 1);
        assert!((edges[0].confidence - 0.95).abs() < 1e-6);
    }
}
