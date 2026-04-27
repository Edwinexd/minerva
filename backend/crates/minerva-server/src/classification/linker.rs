//! Cross-document linking pass: builds the edges of the course
//! knowledge graph after every doc has a `kind`.
//!
//! Pipeline (all course-scoped):
//!
//!   1. **Aggressive embedding filter.** Per-doc top-K nearest
//!      neighbours above `MIN_EMBEDDING_SIMILARITY` (0.65). The
//!      embeddings do the heavy lifting of "do these two docs
//!      share substantive content?"; a course of 50 docs typically
//!      yields ~20-50 surviving pairs, never anywhere near N^2.
//!   2. **Per-pair LLM dispatch in parallel.** Each surviving pair
//!      gets its own focused Cerebras call (`classify_one_pair`),
//!      with a tight prompt; two docs + their kinds + content
//!      excerpts + similarity, three-way decision
//!      (solution_of / part_of_unit / none). Calls run via
//!      `buffer_unordered(PAIR_CALL_CONCURRENCY)` so an N-pair
//!      course finishes in O(N / concurrency) wall-clock.
//!   3. **Post-filter.** Confidence floor, similarity floor per
//!      relation type, teacher vetoes (`rejected_edge_pairs`).
//!
//! Why per-pair instead of "one mega-call with all candidates": at
//! medium reasoning effort gpt-oss-120b spent 40K completion tokens
//! enumerating doc IDs in chain-of-thought before reaching JSON, ran
//! out of budget, returned empty. Per-pair gives the model a tiny
//! decision space, runs in parallel, and isolates each pair from
//! sibling failures.
//!
//! **Filenames are NOT used anywhere in this module.** Real DSV
//! course filenames are unreliable (stale templates, copy/paste,
//! names that contradict content), so the candidate generator looks
//! at embeddings only and the LLM prompt sees `kind` +
//! `classifier_rationale` + `excerpt` per doc; never the filename.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use minerva_db::queries::document_relations::RejectedPairKey;
use minerva_db::queries::documents::DocumentRow;
use qdrant_client::qdrant::{Condition, Filter, ScrollPointsBuilder};
use qdrant_client::Qdrant;
use sqlx::PgPool;
use uuid::Uuid;

use crate::strategy::common::{
    cerebras_request_with_retry, payload_int, payload_string, record_cerebras_usage,
};
use minerva_db::queries::course_token_usage::CATEGORY_LINKER;

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

/// Embedding-similarity floor for candidate generation. Pairs below
/// this are NOT shown to the LLM at all; the embeddings are doing
/// the heavy lifting of "are these two docs about the same thing?".
///
/// Calibration:
///   * pairs from different units mostly score 0.2-0.5
///   * pairs in the same unit (lecture + section summary, assignment
///     + tutorial version, etc.) score 0.65-0.85
///   * duplicate uploads score 0.98+
///
/// 0.70 puts the floor squarely inside the same-unit band so the
/// LLM only spends its budget on pairs the embedding model is
/// already confident overlap. The previous 0.65 leaked too many
/// borderline pairs through (146 candidates on a 50-doc course),
/// which forced an arbitrary post-hoc cap. Tightening the floor
/// keeps the candidate set proportional to course content.
const MIN_EMBEDDING_SIMILARITY: f32 = 0.70;

/// `solution_of` requires a tighter floor than `part_of_unit` because
/// solutions are essentially restatements of their assignments.
const MIN_SOLUTION_OF_SIMILARITY: f32 = 0.75;

/// Cosine similarity above which we treat two docs as effectively
/// identical (duplicate uploads). Such pairs are dropped: they're
/// not "in the same unit", they're the same document.
const DUPLICATE_SIMILARITY: f32 = 0.985;

/// Per-doc top-K when generating embedding-similarity candidates.
/// Combined with the symmetric edge dedup this caps total candidates
/// at roughly N * TOP_K / 2. With the 0.70 similarity floor most
/// docs don't even have 4 neighbours that qualify, so the cap
/// rarely binds; for ones that do it picks the strongest matches.
const EMBEDDING_TOP_K: usize = 4;

/// Hard cap on docs sent to the linker in one call. Keeps the prompt
/// token cost bounded; courses larger than this would need pagination
/// (not implemented in V1; DSV courses are well under the cap).
const MAX_DOCS_PER_CALL: usize = 300;

/// Per-doc content excerpt size for the LLM prompt. Head of the
/// document text; the linker's only grounding signal beyond
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

const LINKER_SYSTEM_PROMPT: &str = r#"You evaluate ONE pair of course documents and decide if they're related.

You're given Document A and Document B. For each:
- kind: one of "lecture", "lecture_transcript", "reading", "tutorial_exercise", "assignment_brief", "sample_solution", "lab_brief", "exam", "syllabus", "unknown"
- classifier_rationale: short note from the per-document classifier
- excerpt: the first ~1500 chars of the document text

You're also given the embedding cosine similarity between A and B.

You are NOT given filenames; in real courses they're unreliable.
Decide from kind + rationale + excerpt + similarity only.

Possible relations (pick ONE):

UNDIRECTED:
- "part_of_unit": A and B are paired course material from the same
  week / module / unit. Reserved for tight pairings; different
  formats of the same content, a lecture + its dedicated overview /
  section summary, an assignment + its submission-instructions page.
  NOT for: two lectures on adjacent topics, two docs that just share
  a course-wide topic word, "this might be the same week".

DIRECTED (the relation has a source and destination, you pick which
side is "a" and which is "b"):
- "a_solution_of_b" / "b_solution_of_a": the source side is a
  sample_solution and the dest side is its assignment_brief /
  lab_brief / exam. The solution's excerpt should plainly answer
  the problem the assignment poses. Requires kinds to line up
  (one side MUST be sample_solution).

- "a_prerequisite_of_b" / "b_prerequisite_of_a": the source doc
  introduces concepts the destination doc builds on. Source is
  earlier / more foundational; destination assumes the source's
  material is understood. Pick this when one excerpt explicitly
  uses concepts (terms, theorems, code abstractions) the other
  introduces, OR when both are teaching content (lecture / reading
  / lecture_transcript) and one is a clear stepping stone for the
  other.

- "a_applied_in_b" / "b_applied_in_a": the source is theoretical /
  expository content (lecture, reading, lecture_transcript) and the
  destination applies it in practice (tutorial_exercise,
  assignment_brief, lab_brief, exam). The destination's excerpt
  references concepts / terms the source introduces. Source MUST be
  one of {lecture, reading, lecture_transcript}; destination MUST
  be one of {tutorial_exercise, assignment_brief, lab_brief, exam}.

- "none": no clear relation; the candidate similarity made this
  pair worth checking but the content doesn't actually pair them.

Calibration: this pair already passed an embedding similarity
threshold (>=0.70), so they share substantive content. Your job
is to identify the relation, not whether ANY relation exists.
A meaningful fraction of candidates SHOULD be one of the four
non-"none" relations; be willing to commit when the pairing is
visible.

Confidence guidance:
  * 0.85+ : unambiguous (problem stated in one, answered in the
    other; concept introduced in lecture, exercised in tutorial).
  * 0.7-0.84 : confident but not certain.
  * 0.5-0.69 : a reasonable guess from visible content overlap.
  * Below 0.5 : use "none".

Output JSON only, matching the schema exactly:
{
  "relation": one of the eight enum values above,
  "confidence": float in [0, 1],
  "rationale": short specific string citing concrete evidence visible
    in the excerpts (a phrase appearing in both, a problem and its
    answer, a concept introduced and applied). Do NOT invent shared
    tokens. Do NOT cite filenames; you don't have them.
}

No prose."#;

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
    /// Docs the linker considered (for telemetry / log lines).
    pub considered: usize,
    /// Candidate pairs whose cached decision was still current --
    /// the LLM was NOT called for these.
    pub cached_hits: usize,
    /// Candidate pairs the LLM was called on this run (cache miss
    /// OR an endpoint had been re-classified since the prior cache).
    pub re_evaluated: usize,
    /// Total edges currently in `document_relations` for the course
    /// after the writeback (positive relations only).
    pub edges_written: usize,
}

/// Inputs the linker needs that aren't on `DocumentRow` itself.
///
/// Note what's NOT here: no fastembed, no OpenAI key, no docs_path.
/// The linker reads everything it needs (chunk text for excerpts,
/// chunk vectors for the lazy-pooled-embedding backfill) from
/// Qdrant. We never re-read PDFs from disk or call an embedder
/// during a relink; the ingest pipeline has already done that work
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
/// to drop candidates BEFORE the LLM call; if both relation types
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
            considered: classified.len(),
            cached_hits: 0,
            re_evaluated: 0,
            edges_written: 0,
        });
    }

    if ctx.api_key.is_empty() {
        // Dev / test env without CEREBRAS_API_KEY. Skip rather than
        // burn time on a guaranteed-401 call.
        return Ok(LinkerOutput {
            considered: classified.len(),
            cached_hits: 0,
            re_evaluated: 0,
            edges_written: 0,
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
            "linker: course {} has {} teacher-vetoed pair(s); those will be skipped",
            course_id,
            rejected.len(),
        );
    }

    // Step 1: gather pooled embeddings, lazily backfilling any that
    // are missing.
    let embeddings: HashMap<Uuid, Vec<f32>> = gather_embeddings(ctx, course_id, truncated).await?;

    // Step 2: embedding-similarity candidates (the only candidate
    // channel; no filename heuristics).
    let mut candidates: HashSet<(Uuid, Uuid)> = HashSet::new();
    let similarity_by_pair: HashMap<(Uuid, Uuid), f32> = build_similarity_matrix(
        truncated,
        &embeddings,
        EMBEDDING_TOP_K,
        MIN_EMBEDDING_SIMILARITY,
        &mut candidates,
    );

    // Drop probable-duplicate pairs (cosine ~ 1) before we even
    // bother sending them to the model; they're not "in the same
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
    // teacher; no LLM call needed. Pairs where SOME relations are
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

    // Built once, used by the kind-compat prefilter below AND the
    // cache lookup further down. Cheap (HashMap of references into
    // `truncated`).
    let docs_by_id: HashMap<Uuid, &&DocumentRow> = truncated.iter().map(|d| (d.id, d)).collect();

    // Kind-compatibility prefilter. For pairs where both kinds are
    // structurally low-signal (`unknown`: classifier had no signal,
    // typically a URL stub or unparseable PDF; `syllabus`: course
    // overview / admin doc that doesn't slot into any of the four
    // edge types in any useful way), no relation in our taxonomy
    // applies. Calling the LLM on these has been a tax we paid for
    // structural noise; a course with N URL-only docs that all
    // classify as `unknown` produces O(N^2) high-similarity pairs
    // that the model dutifully labels `none` at full prompt cost.
    //
    // Auto-none these locally and pin a `none` cache row with the
    // current classified_at snapshot, so subsequent sweeps short-
    // circuit through the cache lookup below. The pair re-enters
    // candidates only if a kind moves (e.g. teacher relabels an
    // `unknown` to a real kind), at which point classified_at
    // changes and the cache row is invalidated.
    const AUTO_NONE_KINDS: &[&str] = &["unknown", "syllabus"];
    let mut auto_none_pairs: Vec<(Uuid, Uuid)> = Vec::new();
    candidates.retain(|pair| {
        let key = if pair.0 < pair.1 {
            (pair.0, pair.1)
        } else {
            (pair.1, pair.0)
        };
        let (Some(da), Some(db_doc)) = (docs_by_id.get(&key.0), docs_by_id.get(&key.1)) else {
            return true;
        };
        let ka = da.kind.as_deref().unwrap_or("unknown");
        let kb = db_doc.kind.as_deref().unwrap_or("unknown");
        if AUTO_NONE_KINDS.contains(&ka) && AUTO_NONE_KINDS.contains(&kb) {
            auto_none_pairs.push(key);
            return false;
        }
        true
    });
    if !auto_none_pairs.is_empty() {
        tracing::info!(
            "linker: course {}; {} pair(s) auto-none'd by kind prefilter (both kinds in {{unknown, syllabus}}), no LLM call",
            course_id,
            auto_none_pairs.len(),
        );
        for key in &auto_none_pairs {
            let (Some(da), Some(db_doc)) = (docs_by_id.get(&key.0), docs_by_id.get(&key.1)) else {
                continue;
            };
            // Drop any prior edge for the unordered pair (handles
            // the kind-changed-from-real-to-unknown direction --
            // an old solution_of edge gets cleared when the doc is
            // relabelled `unknown`).
            let _ = minerva_db::queries::document_relations::delete_relations_for_pair(
                ctx.db, key.0, key.1,
            )
            .await;
            if let Err(e) = minerva_db::queries::linker_decisions::upsert(
                ctx.db,
                course_id,
                key.0,
                key.1,
                da.classified_at,
                db_doc.classified_at,
                None, // relation = none
                None, // confidence = N/A
            )
            .await
            {
                tracing::warn!(
                    "linker: failed to write auto-none cache row for {:?}<->{:?}: {}",
                    key.0,
                    key.1,
                    e
                );
            }
        }
    }

    if candidates.is_empty() {
        // Even if there are no fresh candidates, count what's
        // already in the table for the response.
        let edges_written =
            minerva_db::queries::document_relations::list_by_course(ctx.db, course_id)
                .await
                .map(|v| v.len())
                .unwrap_or(0);
        return Ok(LinkerOutput {
            considered: truncated.len(),
            cached_hits: 0,
            re_evaluated: 0,
            edges_written,
        });
    }

    // Step 3: cache lookup. Skip pairs whose prior decision is
    // still current (both endpoints' classified_at unchanged since
    // the snapshot stored at decision time). This is the big
    // optimisation: a one-doc reclassify only forces re-evaluation
    // of pairs involving that one doc; everyone else's edges are
    // preserved without an LLM round-trip.
    let cached = minerva_db::queries::linker_decisions::list_for_course(ctx.db, course_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                "linker: failed to load decision cache ({}); treating all pairs as fresh",
                e
            );
            HashMap::new()
        });
    let mut fresh_candidates: HashSet<(Uuid, Uuid)> = HashSet::new();
    let mut cached_hits: usize = 0;
    for pair in &candidates {
        let key = if pair.0 < pair.1 {
            (pair.0, pair.1)
        } else {
            (pair.1, pair.0)
        };
        let (Some(da), Some(db)) = (docs_by_id.get(&key.0), docs_by_id.get(&key.1)) else {
            // Shouldn't happen; candidate ids come from `truncated`.
            // Treat as fresh and let the LLM handle it.
            fresh_candidates.insert(*pair);
            continue;
        };
        match cached.get(&key) {
            Some(decision)
                if decision.a_classified_at == da.classified_at
                    && decision.b_classified_at == db.classified_at =>
            {
                cached_hits += 1;
            }
            _ => {
                fresh_candidates.insert(*pair);
            }
        }
    }
    if cached_hits > 0 {
        tracing::info!(
            "linker: course {}; {} candidate(s) reused from decision cache, {} fresh",
            course_id,
            cached_hits,
            fresh_candidates.len(),
        );
    }

    if fresh_candidates.is_empty() {
        let edges_written =
            minerva_db::queries::document_relations::list_by_course(ctx.db, course_id)
                .await
                .map(|v| v.len())
                .unwrap_or(0);
        return Ok(LinkerOutput {
            considered: truncated.len(),
            cached_hits,
            re_evaluated: 0,
            edges_written,
        });
    }

    // Step 4: content excerpts for every doc in any FRESH candidate
    // pair (cached pairs don't need them; we won't be sending them
    // to the LLM).
    let mut docs_in_candidates: HashSet<Uuid> = HashSet::new();
    for (a, b) in &fresh_candidates {
        docs_in_candidates.insert(*a);
        docs_in_candidates.insert(*b);
    }
    let excerpts: HashMap<Uuid, String> = load_excerpts(
        ctx.db,
        ctx.qdrant,
        course_id,
        truncated,
        &docs_in_candidates,
    )
    .await;

    // Step 5: LLM labels FRESH candidates only.
    let edges = call_linker_llm(
        ctx.http,
        ctx.api_key,
        ctx.db,
        course_id,
        truncated,
        &fresh_candidates,
        &similarity_by_pair,
        &excerpts,
    )
    .await?;
    tracing::info!(
        "linker: course {}; LLM proposed {} edge(s) over {} fresh candidate pair(s) ({} cached)",
        course_id,
        edges.len(),
        fresh_candidates.len(),
        cached_hits,
    );

    // Step 6: post-filters + per-pair writeback to both
    // `document_relations` (the live edge set) and `linker_decisions`
    // (the cache). Everything is keyed by the unordered pair so the
    // cache row collapses regardless of direction.
    //
    // For each FRESH pair:
    //   - delete any prior edge for the pair (handles direction
    //     change AND relation change AND "was something, now none")
    //   - if the LLM emitted a passing edge for this pair: upsert it
    //   - upsert the cache row with the snapshot timestamps so the
    //     next relink can short-circuit if nothing changes
    //
    // Edges keyed by their canonical unordered pair so we can find
    // the LLM's proposal (if any) for each fresh pair below.
    let mut edges_by_pair: HashMap<(Uuid, Uuid), ProposedEdge> = HashMap::new();
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

        edges_by_pair.insert(pair_key, edge);
    }
    if dropped_sim > 0 || dropped_rejected > 0 {
        tracing::info!(
            "linker: post-filter summary; {} similarity floor, {} teacher-vetoed",
            dropped_sim,
            dropped_rejected,
        );
    }

    // Step 7: writeback. For each fresh pair, drop any prior
    // edge for the unordered pair, optionally upsert the new one,
    // and record the cache row with the snapshot timestamps.
    for pair in &fresh_candidates {
        let key = if pair.0 < pair.1 {
            (pair.0, pair.1)
        } else {
            (pair.1, pair.0)
        };
        // Doc lookup; both should exist; if not, skip rather
        // than panic.
        let (Some(da), Some(db_doc)) = (docs_by_id.get(&key.0), docs_by_id.get(&key.1)) else {
            continue;
        };

        // Always remove the prior edge for this unordered pair.
        // Required even when the new decision is a relation, because
        // the direction may have flipped (e.g. solution_of source
        // moved to the other side after a kind change).
        let _ = minerva_db::queries::document_relations::delete_relations_for_pair(
            ctx.db, key.0, key.1,
        )
        .await;

        let (rel_for_cache, conf_for_cache) = if let Some(edge) = edges_by_pair.get(&key) {
            // Insert the fresh edge.
            if let Err(e) = minerva_db::queries::document_relations::upsert(
                ctx.db,
                course_id,
                edge.src_id,
                edge.dst_id,
                &edge.relation,
                edge.confidence,
                edge.rationale.as_deref(),
            )
            .await
            {
                tracing::warn!(
                    "linker: failed to upsert edge {:?}->{:?} ({}): {}",
                    edge.src_id,
                    edge.dst_id,
                    edge.relation,
                    e
                );
            }
            (Some(edge.relation.as_str()), Some(edge.confidence))
        } else {
            // LLM said `none` (or post-filter dropped). Cache the
            // null decision so the next relink doesn't re-ask.
            (None, None)
        };

        if let Err(e) = minerva_db::queries::linker_decisions::upsert(
            ctx.db,
            course_id,
            key.0,
            key.1,
            da.classified_at,
            db_doc.classified_at,
            rel_for_cache,
            conf_for_cache,
        )
        .await
        {
            tracing::warn!(
                "linker: failed to upsert decision cache for {:?}<->{:?}: {}",
                key.0,
                key.1,
                e
            );
        }
    }

    let edges_written = minerva_db::queries::document_relations::list_by_course(ctx.db, course_id)
        .await
        .map(|v| v.len())
        .unwrap_or(0);

    Ok(LinkerOutput {
        considered: truncated.len(),
        cached_hits,
        re_evaluated: fresh_candidates.len(),
        edges_written,
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
///    vectors), so this is just a clone out of the DB row; no
///    network, no recompute.
///
/// 2. **Cold path** (only when `pooled_embedding IS NULL`, i.e. data
///    that predates the column or a transient ingest failure): scroll
///    Qdrant for the doc's existing chunk VECTORS and mean-pool them.
///    No file re-read, no embedder call; vectors are already there.
///    The result is persisted back so the next relink hits the hot
///    path.
///
/// For docs without an entry in either source (e.g. a `sample_solution`
/// uploaded before pooled_embedding was added: chunks weren't upserted
/// to Qdrant AND the column was NULL), we just skip them; they won't
/// participate in the similarity matrix this run, but the next ingest
/// of a new doc will repopulate.
async fn gather_embeddings(
    ctx: &LinkContext<'_>,
    course_id: Uuid,
    docs: &[&DocumentRow],
) -> Result<HashMap<Uuid, Vec<f32>>, String> {
    let mut out: HashMap<Uuid, Vec<f32>> = HashMap::new();
    let collection = minerva_ingest::pipeline::collection_name_for_course(ctx.db, course_id)
        .await
        .unwrap_or_else(|_| minerva_ingest::pipeline::collection_name(course_id, 1));
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
                    "linker: no embedding available for doc {}; skipping similarity channel",
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
/// path; the vectors are already there, just pool them. Returns None
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
                // Cap at 1000 chunks per doc; our chunker's default
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
/// readable file; those simply get an empty excerpt.
///
/// **Concurrency**: each doc's text extraction runs on the blocking
/// thread pool via `spawn_blocking`, fanned out concurrently via
/// `join_all`. Sync PDF parsing on the main runtime previously blocked
/// the worker thread for hundreds of ms per doc; a large course
/// (~30 docs) could stall request handling for several seconds during
/// every relink. Now the runtime thread just awaits a join_all of
/// blocking-pool tasks; HTTP handlers stay responsive.
/// Pull a content excerpt for each in-candidate doc out of Qdrant.
/// We grab a few chunks per doc (sorted by chunk_index ascending) and
/// concatenate their text up to `EXCERPT_CHARS`. No file I/O, no PDF
/// re-parsing; the chunks are already there from ingest.
///
/// Sample-solution docs aren't in Qdrant (they're embedded into the
/// doc-row pooled vector but their chunks are deliberately excluded
/// from retrieval). Those simply get an empty excerpt; the linker
/// then leans on `kind=sample_solution` + the assignment doc's
/// excerpt + embedding similarity to decide solution_of edges.
async fn load_excerpts(
    db: &PgPool,
    qdrant: &Qdrant,
    course_id: Uuid,
    docs: &[&DocumentRow],
    only_for: &HashSet<Uuid>,
) -> HashMap<Uuid, String> {
    let collection = minerva_ingest::pipeline::collection_name_for_course(db, course_id)
        .await
        .unwrap_or_else(|_| minerva_ingest::pipeline::collection_name(course_id, 1));
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
    // and clone the collection name per-task (cheap; a small
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

/// How many per-pair Cerebras requests are in flight at once.
/// Tuned conservatively: each call is small and Cerebras is fast,
/// so we don't need a huge fan-out, but enough that an 80-pair
/// course finishes in well under a minute.
const PAIR_CALL_CONCURRENCY: usize = 12;

/// Per-pair LLM dispatch. Replaces the prior "send all candidates
/// in one mega-prompt" architecture, which broke when a 47-doc
/// course produced 266 candidates: at medium reasoning effort the
/// model spiralled into a 40K-token chain-of-thought enumerating
/// every doc and ran out of completion budget before emitting JSON.
///
/// Each surviving candidate now gets its own focused Cerebras call:
///   * tiny prompt (system + 2 docs + similarity)
///   * three-way decision (solution_of / part_of_unit / none)
///   * structured-output schema -> small, well-formed JSON
///
/// Calls run in parallel with a bounded concurrency cap so we don't
/// rate-limit ourselves against Cerebras. With ~30-50 surviving
/// candidates after the embedding-similarity floor and the final
/// MAX_CANDIDATES_PER_CALL guard, total wall-clock is on the order
/// of 5-10 seconds per relink.
#[allow(clippy::too_many_arguments)]
async fn call_linker_llm(
    http: &reqwest::Client,
    api_key: &str,
    db: &sqlx::PgPool,
    course_id: Uuid,
    docs: &[&DocumentRow],
    candidates: &HashSet<(Uuid, Uuid)>,
    similarity: &HashMap<(Uuid, Uuid), f32>,
    excerpts: &HashMap<Uuid, String>,
) -> Result<Vec<ProposedEdge>, String> {
    use futures::stream::{self, StreamExt};

    // Index docs by id for cheap lookup inside each per-pair task.
    let docs_by_id: HashMap<Uuid, &&DocumentRow> = docs.iter().map(|d| (d.id, d)).collect();

    // Stable sort so log lines are consistent across reruns and
    // so prompt-cache hits within a course (per-pair system prompt
    // is byte-identical, only user payload differs) reuse the
    // cached system prefix.
    let mut sorted_candidates: Vec<(Uuid, Uuid)> = candidates.iter().copied().collect();
    sorted_candidates.sort();

    // Build per-pair input bundles up-front so the async stream
    // doesn't have to clone the docs/excerpts hashes.
    let pair_inputs: Vec<PairInput> = sorted_candidates
        .iter()
        .filter_map(|(a, b)| {
            let doc_a = docs_by_id.get(a).copied().copied()?;
            let doc_b = docs_by_id.get(b).copied().copied()?;
            let sim = similarity.get(&(*a, *b)).copied().unwrap_or(0.0);
            Some(PairInput {
                a_id: *a,
                a_kind: doc_a.kind.clone().unwrap_or_else(|| "unknown".to_string()),
                a_rationale: doc_a.kind_rationale.clone().unwrap_or_default(),
                a_excerpt: excerpts.get(a).cloned().unwrap_or_default(),
                b_id: *b,
                b_kind: doc_b.kind.clone().unwrap_or_else(|| "unknown".to_string()),
                b_rationale: doc_b.kind_rationale.clone().unwrap_or_default(),
                b_excerpt: excerpts.get(b).cloned().unwrap_or_default(),
                similarity: sim,
            })
        })
        .collect();

    let total_pairs = pair_inputs.len();
    tracing::info!(
        "linker: dispatching {} per-pair Cerebras calls (concurrency {})",
        total_pairs,
        PAIR_CALL_CONCURRENCY,
    );

    // Bounded-parallel stream: at most PAIR_CALL_CONCURRENCY in
    // flight. `filter_map` drops `none` results and per-pair errors
    // (which are logged inside `classify_one_pair`) so the final
    // collected Vec only contains real edges.
    let edges: Vec<ProposedEdge> = stream::iter(pair_inputs)
        .map(|pair| async move { classify_one_pair(http, api_key, db, course_id, pair).await })
        .buffer_unordered(PAIR_CALL_CONCURRENCY)
        .filter_map(|r| async move { r })
        .collect()
        .await;

    tracing::info!(
        "linker: per-pair pass complete; {} edges proposed across {} pairs",
        edges.len(),
        total_pairs,
    );

    Ok(edges)
}

/// Inputs for a single per-pair Cerebras call. Owns its strings so
/// it can be moved into the async block under `buffer_unordered`.
struct PairInput {
    a_id: Uuid,
    a_kind: String,
    a_rationale: String,
    a_excerpt: String,
    b_id: Uuid,
    b_kind: String,
    b_rationale: String,
    b_excerpt: String,
    similarity: f32,
}

/// Single per-pair Cerebras request. Returns Some(edge) when the
/// model labels the pair `solution_of` or `part_of_unit` with
/// confidence above the floor; None for `none`, low-confidence
/// guesses, malformed responses, or transport errors. All failure
/// modes are logged at debug/info; the caller only cares about the
/// edge stream.
async fn classify_one_pair(
    http: &reqwest::Client,
    api_key: &str,
    db: &sqlx::PgPool,
    course_id: Uuid,
    p: PairInput,
) -> Option<ProposedEdge> {
    let user_payload = serde_json::json!({
        "document_a": {
            "kind": p.a_kind,
            "classifier_rationale": p.a_rationale,
            "excerpt": p.a_excerpt,
        },
        "document_b": {
            "kind": p.b_kind,
            "classifier_rationale": p.b_rationale,
            "excerpt": p.b_excerpt,
        },
        "embedding_similarity": p.similarity,
    });

    let body = serde_json::json!({
        "model": LINKER_MODEL,
        "temperature": 0.0,
        "reasoning_effort": "low",
        // Tight ceiling; the response is at most ~150 tokens of
        // JSON. Generous overhead for the model's brief CoT but
        // small enough to fail fast on a runaway.
        "max_completion_tokens": 1024,
        "messages": [
            { "role": "system", "content": LINKER_SYSTEM_PROMPT },
            { "role": "user", "content": user_payload.to_string() },
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "course_kg_pair_decision",
                "strict": true,
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["relation", "confidence", "rationale"],
                    "properties": {
                        "relation": {
                            "type": "string",
                            "enum": [
                                "part_of_unit",
                                "a_solution_of_b",
                                "b_solution_of_a",
                                "a_prerequisite_of_b",
                                "b_prerequisite_of_a",
                                "a_applied_in_b",
                                "b_applied_in_a",
                                "none",
                            ],
                        },
                        "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                        "rationale": { "type": "string" },
                    }
                }
            }
        }
    });

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                "linker: per-pair request failed for {}<->{}: {}",
                p.a_id,
                p.b_id,
                e
            );
            return None;
        }
    };
    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "linker: per-pair response not JSON for {}<->{}: {}",
                p.a_id,
                p.b_id,
                e
            );
            return None;
        }
    };
    // Best-effort token-spend bookkeeping. Records every per-pair
    // call against `course_id` in the `linker` category, regardless
    // of whether the call ultimately produced an edge; the cost
    // was paid either way.
    record_cerebras_usage(db, course_id, CATEGORY_LINKER, LINKER_MODEL, &payload).await;
    // Guard against finish_reason=length producing an empty content
    // field; caller would otherwise see this as "no edge" without
    // knowing the model was cut off mid-token.
    let finish = payload["choices"][0]["finish_reason"]
        .as_str()
        .unwrap_or("");
    if finish == "length" {
        tracing::warn!(
            "linker: per-pair {}<->{} hit completion-token cap; raising max_completion_tokens may help",
            p.a_id,
            p.b_id,
        );
        return None;
    }
    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    if raw.is_empty() {
        tracing::warn!(
            "linker: per-pair {}<->{} returned empty content (finish={})",
            p.a_id,
            p.b_id,
            finish
        );
        return None;
    }

    let parsed: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "linker: per-pair {}<->{} unparseable JSON: {} (raw: {})",
                p.a_id,
                p.b_id,
                e,
                raw.chars().take(200).collect::<String>(),
            );
            return None;
        }
    };

    let raw_relation = parsed["relation"].as_str().unwrap_or("none");
    if raw_relation == "none" {
        return None;
    }
    let confidence = parsed["confidence"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
    if confidence < MIN_EDGE_CONFIDENCE {
        return None;
    }
    let rationale = parsed["rationale"].as_str().map(str::to_string);

    // Decode the (relation, direction) pair from the model's enum
    // value. Directed relations are split into `a_X_of_b` /
    // `b_X_of_a` variants so the model commits to a direction
    // rather than emitting a separate optional field. Returns None
    // for any value we don't recognise (defensive against the
    // model going off-schema).
    let (canonical_relation, src_is_a) = match raw_relation {
        "part_of_unit" => ("part_of_unit", true), // direction normalised by id below
        "a_solution_of_b" => ("solution_of", true),
        "b_solution_of_a" => ("solution_of", false),
        "a_prerequisite_of_b" => ("prerequisite_of", true),
        "b_prerequisite_of_a" => ("prerequisite_of", false),
        "a_applied_in_b" => ("applied_in", true),
        "b_applied_in_a" => ("applied_in", false),
        _ => return None,
    };

    // Direction-derive the (src, dst) tuple per relation:
    //
    //   * `part_of_unit` is undirected; normalise by id ordering
    //     so duplicates collapse on the unique constraint at upsert
    //     time.
    //   * `solution_of` is directional and the kinds tell us which
    //     side is which (sample_solution -> dst). We override the
    //     model's a/b choice with the kind-derived direction since
    //     the kind is the ground truth and one less thing for the
    //     model to get wrong; drop the edge if neither side is a
    //     sample_solution (model picked the wrong relation).
    //   * `applied_in` is directional with a kind-based gate
    //     (theory -> practice). Same override-from-kinds logic;
    //     drop if neither side fits the theory/practice pattern.
    //   * `prerequisite_of` is directional with no kind-based
    //     gate; both sides are typically lecture / reading.
    //     Trust the model's a/b pick.
    let theory = ["lecture", "lecture_transcript", "reading"];
    let practice = ["tutorial_exercise", "assignment_brief", "lab_brief", "exam"];
    let (src, dst) = match canonical_relation {
        "part_of_unit" => {
            if p.a_id < p.b_id {
                (p.a_id, p.b_id)
            } else {
                (p.b_id, p.a_id)
            }
        }
        "solution_of" => {
            if p.a_kind == "sample_solution" {
                (p.a_id, p.b_id)
            } else if p.b_kind == "sample_solution" {
                (p.b_id, p.a_id)
            } else {
                tracing::info!(
                    "linker: dropping solution_of {}<->{}; neither side has kind=sample_solution",
                    p.a_id,
                    p.b_id,
                );
                return None;
            }
        }
        "applied_in" => {
            let a_theory = theory.contains(&p.a_kind.as_str());
            let b_theory = theory.contains(&p.b_kind.as_str());
            let a_practice = practice.contains(&p.a_kind.as_str());
            let b_practice = practice.contains(&p.b_kind.as_str());
            if a_theory && b_practice {
                (p.a_id, p.b_id)
            } else if b_theory && a_practice {
                (p.b_id, p.a_id)
            } else {
                tracing::info!(
                    "linker: dropping applied_in {}<->{}; kinds don't fit theory->practice (a={}, b={})",
                    p.a_id,
                    p.b_id,
                    p.a_kind,
                    p.b_kind,
                );
                return None;
            }
        }
        "prerequisite_of" => {
            if src_is_a {
                (p.a_id, p.b_id)
            } else {
                (p.b_id, p.a_id)
            }
        }
        _ => unreachable!("canonical_relation matched non-handled variant"),
    };

    Some(ProposedEdge {
        src_id: src,
        dst_id: dst,
        relation: canonical_relation.to_string(),
        confidence,
        rationale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
