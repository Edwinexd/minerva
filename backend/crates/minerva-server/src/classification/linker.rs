//! Cross-document linking pass: builds the edges of the course
//! knowledge graph after every doc has a `kind`.
//!
//! The pass is course-scoped. It takes the (id, filename, kind,
//! rationale) of every classified document in a course, sends them
//! all to gpt-oss-120b in a single call, and asks the model to
//! propose typed edges:
//!
//!   * `solution_of(src=sample_solution, dst=assignment_brief|lab_brief|exam)`
//!   * `part_of_unit(src, dst)` -- two docs that belong to the same
//!     week / module / topic, regardless of kind.
//!
//! Why one big call rather than per-pair: pairwise scales O(n^2) and
//! drowns context in noise. A single call lets the model do simple
//! sanity checks across the corpus (numbering coherence, cluster the
//! lab+brief+solution+rubric for a unit together) and emit only the
//! confident edges. For courses bigger than the model can fit in one
//! turn we paginate by unit_hint -- but in practice DSV courses are
//! 20-200 docs, well within budget.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use minerva_db::queries::document_relations::RejectedPairKey;
use minerva_db::queries::documents::DocumentRow;
use minerva_ingest::fastembed_embedder::FastEmbedder;
use sqlx::PgPool;
use uuid::Uuid;

use crate::strategy::common::cerebras_request_with_retry;

/// Tokens that, when followed by digits in a filename, indicate "this
/// doc belongs to <something> #N" -- a unit/week/lab/chapter/exercise
/// numbering. If two filenames carry the SAME prefix with DIFFERENT
/// numbers, they're in different units (lab2 vs lab3 are not the same
/// unit), and any `part_of_unit` edge between them should be dropped.
///
/// We list both English and Swedish prefixes since DSV courses have
/// significant Swedish material (övning, uppgift, vecka, kapitel,
/// inlämning, etc.). Lowercase comparison; the regex below applies
/// after lowercasing the filename.
const UNIT_NUMBER_PREFIXES: &[&str] = &[
    // English
    "week",
    "lab",
    "lecture",
    "chapter",
    "ch",
    "module",
    "assignment",
    "exercise",
    "ex",
    "homework",
    "hw",
    "session",
    "unit",
    "part",
    // Swedish
    "vecka",
    "lektion",
    "kapitel",
    "modul",
    "uppgift",
    "ovning", // for "övning" after diacritic stripping
    "ovningsuppgift",
    "inlamning", // for "inlämning"
    "laboration",
    "lab", // already above; harmless duplicate keeps logic order obvious
    "del",
    "avsnitt",
    // DSV-specific lecture-numbering convention: F01, F02, ... is
    // "Föreläsning 01" etc. The single letter is too generic on its
    // own but the extractor requires digits IMMEDIATELY after the
    // prefix (with optional separators), so "f01" matches but
    // "first" / "fax" / "fri-day" don't.
    "f",
];

const LINKER_MODEL: &str = "gpt-oss-120b";

/// Drop edges the model emits below this confidence.
const MIN_EDGE_CONFIDENCE: f32 = 0.6;

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
/// document text -- the linker already has filename + kind +
/// classification rationale, this is the additional grounding that
/// stops it inventing relationships from filenames alone.
const EXCERPT_CHARS: usize = 400;

const LINKER_SYSTEM_PROMPT: &str = r#"You label edges in a course-document knowledge graph.

You will receive:

1. A "documents" array. Each document has:
   - "id": opaque identifier
   - "filename"
   - "kind": one of "lecture", "reading", "assignment_brief", "sample_solution", "lab_brief", "exam", "syllabus", "unknown"
   - "classifier_rationale": short note from the per-document classifier
   - "excerpt": the first few hundred characters of the document text

2. A "candidates" array of pre-selected pairs that are SEMANTICALLY similar
   (high embedding similarity) or share filename markers. Each candidate has:
   - "src_id", "dst_id": the two document ids
   - "similarity": cosine similarity in [0, 1] of the docs' content embeddings
   - "shared_filename_marker": optional concrete shared marker like "lab2",
     "F18", "module-trees" -- present when both filenames carry the same
     unit token, absent otherwise

For EACH candidate pair, decide whether there is a typed relation between
the two documents. You may also decide there is NONE (omit it from output).

Filename markers are the STRONGEST signal in this task -- much stronger
than topic-word overlap or embedding similarity. Each candidate may carry
a `shared_filename_marker` (e.g. "lab2", "f18", "week3"). If present, the
two documents almost certainly belong to the same unit. Treat the marker
as a hard prior and lean toward emitting `part_of_unit` unless their
content disagrees outright (e.g. one is a syllabus and one is a lab).

Conversely, if a candidate has NO shared filename marker, you need much
stronger evidence to emit `part_of_unit`: either a clear "this is the
solution to that" / "this is the lab for that lecture" relationship in
the content, or a unique-and-specific shared subtopic (not a course-wide
topic word like "inheritance" / "OO" / "loops"). Topic-word overlap
ALONE is never sufficient.

Possible relations:
- "solution_of": src is the sample_solution; dst is the assignment_brief /
  lab_brief / exam it answers. Use when one doc is `kind=sample_solution`
  AND the OTHER is an assessment kind AND the content/filenames clearly
  pair them. A shared filename marker (e.g. "lab2") is the strongest
  evidence. Do NOT emit solution_of without explicit kind=sample_solution
  on one side.
- "part_of_unit": both documents belong to the same week / module /
  chapter / topic / unit. Strong path: shared_filename_marker is set,
  emit with confidence ~0.9 unless content disagrees. Weak path: no
  filename marker; emit only if both excerpts share a SPECIFIC subtopic
  AND there is a structural relationship (lab + brief + solution +
  rubric all in the same unit, etc.). NOTE: two sequential lectures on
  the same broad subject (F01_Arv_I and F02_Arv_II, week3 and week4) are
  NOT in the same unit -- they are in adjacent units of a course module.
  Do not link them. A topic word like "Arv" or "inheritance" appearing
  in both filenames is NOT sufficient evidence; you need either matching
  numeric markers, or a clear "this is a summary of that" / "this is
  the lab for that lecture" relationship visible in the content.

Output format -- JSON, nothing else:
{
  "edges": [
    {
      "src_id": <id from candidate>,
      "dst_id": <id from candidate>,
      "relation": "solution_of" | "part_of_unit",
      "confidence": float in [0, 1],
      "rationale": short specific string. CITE concrete evidence visible
        in the inputs: filename markers, words from both excerpts, the
        classifier rationale. Do NOT invent shared tokens.
    }
  ]
}

HARD RULES:
- ONLY emit edges for pairs in the candidates list. Do NOT propose pairs
  that are not candidates.
- AT MOST ONE edge per candidate pair.
- Do NOT emit self-loops (same src_id and dst_id).
- DO NOT emit a part_of_unit edge between docs with conflicting numeric
  markers (e.g. "lab2" vs "lab3", "uppgift3" vs "uppgift4", "F01" vs
  "F02" with completely different topics). The application post-filter
  drops these regardless, but emitting them wastes effort.
- DO NOT emit any edge for pairs whose excerpts are essentially identical
  (likely duplicate uploads of the same document). These belong nowhere.
- The "rationale" must be grounded. "Both are exercises" or "both contain
  the topic word" are NOT sufficient unless you can name the concrete
  shared word verbatim from the inputs.
- Confidence < 0.6 will be dropped server-side; aim higher or skip the
  edge entirely.
- For solution_of, prefer kind+filename evidence over similarity alone.
- For part_of_unit, you need both content overlap AND at least one
  concrete grounding signal.

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

/// Inputs the linker needs that aren't on `DocumentRow` itself: a
/// way to lazily backfill missing pooled embeddings (Qdrant scroll +
/// re-pool), and the path to read content excerpts from disk. Bundling
/// these into a struct keeps `link_course`'s signature manageable.
pub struct LinkContext<'a> {
    pub http: &'a reqwest::Client,
    pub api_key: &'a str,
    pub db: &'a PgPool,
    pub fastembed: &'a Arc<FastEmbedder>,
    pub openai_api_key: &'a str,
    pub docs_path: &'a str,
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
/// Pipeline (the "proper KG" shape):
///   1. **Embeddings**: every doc has a pooled embedding from the
///      ingest pipeline. For docs missing one (older data), lazily
///      backfill by re-embedding the doc text. Embeddings are
///      L2-normalised so cosine similarity is just a dot product.
///   2. **Candidate generation** (blocking): the candidate set is the
///      union of two channels. (a) Embedding similarity: per doc, top-K
///      most similar OTHER docs above `MIN_EMBEDDING_SIMILARITY`.
///      (b) Filename markers: pairs whose filenames share a unit token
///      with matching numbers ("lab2_brief" + "lab2_solution",
///      "F18 OO" + "F18_section_summary"). Filename-marker pairs are
///      kept regardless of embedding similarity -- a shared marker is
///      strong positive evidence even if surface vocabulary differs.
///   3. **Content excerpts**: for each doc that appears in any
///      candidate, read the first EXCERPT_CHARS from disk so the LLM
///      grounds its decisions in actual content, not just metadata.
///   4. **LLM labelling** (matching): single Cerebras call labels each
///      candidate as solution_of / part_of_unit / nothing.
///   5. **Post-filters**: confidence floor, similarity floors per
///      relation type, duplicate detection (cosine ~ 1), and the
///      filename numeric-marker disagreement filter.
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

    // Step 2a: embedding-similarity candidates.
    let mut candidates: HashSet<(Uuid, Uuid)> = HashSet::new();
    let similarity_by_pair: HashMap<(Uuid, Uuid), f32> = build_similarity_matrix(
        truncated,
        &embeddings,
        EMBEDDING_TOP_K,
        MIN_EMBEDDING_SIMILARITY,
        &mut candidates,
    );

    // Step 2b: filename-marker candidates (matching prefix+number).
    add_filename_marker_candidates(truncated, &mut candidates);

    // Drop probable-duplicate pairs (cosine ~ 1) before we even
    // bother sending them to the model -- they're not "in the same
    // unit", they're the same document re-uploaded.
    candidates.retain(|pair| {
        let sim = similarity_by_pair.get(pair).copied().unwrap_or(0.0);
        if sim >= DUPLICATE_SIMILARITY {
            tracing::info!(
                "linker: dropping likely-duplicate candidate {:?}<->{:?} (similarity {:.3})",
                pair.0,
                pair.1,
                sim
            );
            return false;
        }
        true
    });

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
        load_excerpts(ctx.docs_path, course_id, truncated, &docs_in_candidates).await;

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

    // Step 5: post-filters.
    let filenames_by_id: HashMap<Uuid, &str> = truncated
        .iter()
        .map(|d| (d.id, d.filename.as_str()))
        .collect();
    let mut kept = Vec::with_capacity(edges.len());
    let mut dropped_marker = 0usize;
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
        // Allow filename-marker-grounded part_of_unit edges to bypass
        // the embedding floor: docs in the same unit may have
        // surface-different content but matching markers are strong
        // positive evidence ("lab2_brief.pdf" + "lab2_helper.pdf").
        let a_name = filenames_by_id.get(&edge.src_id).copied().unwrap_or("");
        let b_name = filenames_by_id.get(&edge.dst_id).copied().unwrap_or("");
        let has_filename_grounding =
            edge.relation == "part_of_unit" && shares_matching_marker(a_name, b_name);

        if sim < floor && !has_filename_grounding {
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

        // Filename numeric-marker disagreement filter for part_of_unit.
        if edge.relation == "part_of_unit" && !part_of_unit_passes_filename_check(a_name, b_name) {
            tracing::info!(
                "linker: post-filter dropped part_of_unit between {:?} ({}) and {:?} ({}) -- numeric markers disagree",
                edge.src_id,
                a_name,
                edge.dst_id,
                b_name,
            );
            dropped_marker += 1;
            continue;
        }

        kept.push(edge);
    }
    if dropped_marker > 0 || dropped_sim > 0 || dropped_rejected > 0 {
        tracing::info!(
            "linker: post-filter summary -- {} marker mismatch, {} similarity floor, {} teacher-vetoed",
            dropped_marker,
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
async fn gather_embeddings(
    ctx: &LinkContext<'_>,
    course_id: Uuid,
    docs: &[&DocumentRow],
) -> Result<HashMap<Uuid, Vec<f32>>, String> {
    let mut out: HashMap<Uuid, Vec<f32>> = HashMap::new();
    for doc in docs {
        if let Some(emb) = &doc.pooled_embedding {
            if !emb.is_empty() {
                out.insert(doc.id, emb.clone());
                continue;
            }
        }

        // Lazy backfill: read text, re-embed, mean-pool, persist.
        match recompute_pooled_embedding(ctx, course_id, doc).await {
            Ok(Some(emb)) => {
                let _ = minerva_db::queries::documents::set_pooled_embedding(ctx.db, doc.id, &emb)
                    .await;
                out.insert(doc.id, emb);
            }
            Ok(None) => {
                tracing::debug!(
                    "linker: no embedding available for doc {} ({}) -- skipping similarity channel for it",
                    doc.id,
                    doc.filename
                );
            }
            Err(e) => {
                tracing::warn!(
                    "linker: failed to backfill embedding for doc {} ({}): {}",
                    doc.id,
                    doc.filename,
                    e
                );
            }
        }
    }
    Ok(out)
}

async fn recompute_pooled_embedding(
    ctx: &LinkContext<'_>,
    course_id: Uuid,
    doc: &DocumentRow,
) -> Result<Option<Vec<f32>>, String> {
    let ext = doc
        .filename
        .rsplit('.')
        .next()
        .filter(|e| *e != doc.filename.as_str())
        .unwrap_or("bin");
    let path_buf = format!("{}/{}/{}.{}", ctx.docs_path, course_id, doc.id, ext);
    let path = Path::new(&path_buf);
    if !path.exists() {
        return Ok(None);
    }
    let text = match minerva_ingest::pipeline::extract_document_text(path) {
        Ok(t) if !t.trim().is_empty() => t,
        _ => return Ok(None),
    };

    // Look up the course's embedding config.
    let course = match minerva_db::queries::courses::find_by_id(ctx.db, course_id).await {
        Ok(Some(c)) => c,
        _ => return Ok(None),
    };
    // Chunk the text the same way ingest does, embed each chunk,
    // mean-pool. We don't upsert to Qdrant here -- this is a one-off
    // computation to populate the doc-level vector.
    let chunks = minerva_ingest::chunker::chunk_text(
        &text,
        &minerva_ingest::chunker::ChunkerConfig::default(),
    );
    if chunks.is_empty() {
        return Ok(None);
    }
    let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let embeddings: Vec<Vec<f32>> = match course.embedding_provider.as_str() {
        "local" => ctx
            .fastembed
            .embed(&course.embedding_model, chunk_texts)
            .await
            .map_err(|e| format!("fastembed failed: {}", e))?,
        _ => {
            let result =
                minerva_ingest::embedder::embed_texts(ctx.http, ctx.openai_api_key, &chunk_texts)
                    .await
                    .map_err(|e| format!("openai embed failed: {}", e))?;
            result.embeddings
        }
    };
    Ok(mean_pool_normalized(&embeddings))
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

/// Add candidate pairs whose filenames share a unit-prefix marker
/// with matching numbers ("lab2_brief" + "lab2_solution"). Two docs
/// with the same prefix+number are nearly always related even if
/// their content embeddings happen to score below the floor.
fn add_filename_marker_candidates(docs: &[&DocumentRow], candidates: &mut HashSet<(Uuid, Uuid)>) {
    for i in 0..docs.len() {
        for j in (i + 1)..docs.len() {
            if shares_matching_marker(&docs[i].filename, &docs[j].filename) {
                let pair = if docs[i].id < docs[j].id {
                    (docs[i].id, docs[j].id)
                } else {
                    (docs[j].id, docs[i].id)
                };
                candidates.insert(pair);
            }
        }
    }
}

/// True iff the two filenames share at least one unit-prefix marker
/// where the NUMBERS match. ("lab2_brief.pdf" + "lab2_solution.pdf"
/// -> true; "lab2.pdf" + "lab3.pdf" -> false; "lab2.pdf" +
/// "intro.pdf" -> false because no shared prefix.)
fn shares_matching_marker(a: &str, b: &str) -> bool {
    let ma = extract_unit_markers(a);
    let mb = extract_unit_markers(b);
    for (pa, na) in &ma {
        for (pb, nb) in &mb {
            if pa == pb && na == nb {
                return true;
            }
        }
    }
    false
}

/// Read the first `EXCERPT_CHARS` of each in-candidate doc's text
/// from disk. URL/awaiting-transcript/unsupported docs may have no
/// readable file -- those simply get an empty excerpt.
async fn load_excerpts(
    docs_path: &str,
    course_id: Uuid,
    docs: &[&DocumentRow],
    only_for: &HashSet<Uuid>,
) -> HashMap<Uuid, String> {
    let mut out: HashMap<Uuid, String> = HashMap::new();
    for doc in docs {
        if !only_for.contains(&doc.id) {
            continue;
        }
        let ext = doc
            .filename
            .rsplit('.')
            .next()
            .filter(|e| *e != doc.filename.as_str())
            .unwrap_or("bin");
        let path = format!("{}/{}/{}.{}", docs_path, course_id, doc.id, ext);
        let path_obj = Path::new(&path);
        if !path_obj.exists() {
            out.insert(doc.id, String::new());
            continue;
        }
        match minerva_ingest::pipeline::extract_document_text(path_obj) {
            Ok(text) => {
                let normalised: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let excerpt: String = normalised.chars().take(EXCERPT_CHARS).collect();
                out.insert(doc.id, excerpt);
            }
            Err(e) => {
                tracing::debug!("linker: excerpt for {} unavailable: {}", doc.filename, e);
                out.insert(doc.id, String::new());
            }
        }
    }
    out
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
                "filename": d.filename,
                "kind": d.kind.as_deref().unwrap_or("unknown"),
                "classifier_rationale": d.kind_rationale.as_deref().unwrap_or(""),
                "excerpt": excerpts.get(&d.id).cloned().unwrap_or_default(),
            })
        })
        .collect();

    // Candidates block: stable sort so prompt-cache hits are more
    // likely on re-runs of the same course.
    let filenames_by_id: HashMap<Uuid, &str> =
        docs.iter().map(|d| (d.id, d.filename.as_str())).collect();
    let mut sorted_candidates: Vec<(Uuid, Uuid)> = candidates.iter().copied().collect();
    sorted_candidates.sort();
    let candidates_array: Vec<serde_json::Value> = sorted_candidates
        .iter()
        .map(|(a, b)| {
            let sim = similarity.get(&(*a, *b)).copied().unwrap_or(0.0);
            let na = filenames_by_id.get(a).copied().unwrap_or("");
            let nb = filenames_by_id.get(b).copied().unwrap_or("");
            let shared = matched_marker_token(na, nb);
            let mut obj = serde_json::json!({
                "src_id": a.to_string(),
                "dst_id": b.to_string(),
                "similarity": sim,
            });
            if let Some(token) = shared {
                obj["shared_filename_marker"] = serde_json::Value::String(token);
            }
            obj
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

/// First shared (prefix, number) pair as a "lab2"-style token, for
/// surfacing to the LLM as a `shared_filename_marker` hint. Returns
/// the lowercased+folded token if any matches, else None.
fn matched_marker_token(a: &str, b: &str) -> Option<String> {
    let ma = extract_unit_markers(a);
    let mb = extract_unit_markers(b);
    for (pa, na) in &ma {
        for (pb, nb) in &mb {
            if pa == pb && na == nb {
                return Some(format!("{}{}", pa, na));
            }
        }
    }
    None
}

/// Strip Latin diacritics so "övningsuppgift3" and "ovningsuppgift3"
/// compare the same. Hand-rolled because pulling in unicode-normalization
/// for one function would be overkill; we only need NFD-style fold for
/// the small Swedish/English alphabet we care about.
fn fold_diacritics(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'å' | 'ä' | 'à' | 'á' | 'â' => 'a',
            'ö' | 'ø' | 'ò' | 'ó' | 'ô' => 'o',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'ü' | 'ù' | 'ú' | 'û' => 'u',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ý' => 'y',
            'ñ' => 'n',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

/// Pull "<unit-prefix><digits>" markers out of a filename. Returns the
/// (prefix, number) pairs found, lowercased and diacritic-folded. If
/// the same prefix appears multiple times with different numbers we
/// keep all of them so the comparison sees every signal.
///
/// Examples (post-fold):
///   "ovningsuppgift3_vt25.pdf" -> [("ovningsuppgift", 3)]
///     (note: "vt25" doesn't match any prefix, so 25 is ignored.)
///   "lab2_solution.pdf"        -> [("lab", 2)]
///   "week3_lecture5.pdf"       -> [("week", 3), ("lecture", 5)]
///   "intro.pdf"                -> []
///
/// Implementation: lowercase + fold diacritics, then walk char-by-char
/// looking for any prefix substring followed immediately by digits.
/// We do NOT use the `regex` crate to keep this trivially testable
/// and avoid reaching across the existing prefix list at runtime.
fn extract_unit_markers(filename: &str) -> Vec<(String, u32)> {
    let s = fold_diacritics(&filename.to_lowercase());
    let bytes = s.as_bytes();
    let mut out: Vec<(String, u32)> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        // Try every prefix at position i; pick the longest match so
        // "ovningsuppgift" wins over "ovning" when both could match.
        let mut best_prefix_len = 0usize;
        let mut best_prefix: Option<&'static str> = None;
        for p in UNIT_NUMBER_PREFIXES {
            let pb = p.as_bytes();
            if i + pb.len() > bytes.len() {
                continue;
            }
            if &bytes[i..i + pb.len()] == pb && pb.len() > best_prefix_len {
                best_prefix_len = pb.len();
                best_prefix = Some(p);
            }
        }
        if let Some(p) = best_prefix {
            let mut j = i + best_prefix_len;
            // Skip optional separator between word and number ("lab-2",
            // "module_4", "week 3", "kapitel.5").
            while j < bytes.len() && matches!(bytes[j], b'-' | b'_' | b' ' | b'.') {
                j += 1;
            }
            // Read digits.
            let digits_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > digits_start {
                if let Ok(n) = s[digits_start..j].parse::<u32>() {
                    // Reject the boundary case where the prefix is the
                    // tail of a longer word (e.g. "lab" inside "labour").
                    // Require a non-letter on the left of the prefix
                    // (or it's at position 0).
                    let left_ok = i == 0 || !s.as_bytes()[i - 1].is_ascii_alphabetic();
                    if left_ok {
                        out.push((p.to_string(), n));
                    }
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Decide whether a `part_of_unit` edge between two filenames should
/// survive the deterministic post-filter.
///
/// Rule: if BOTH filenames carry at least one unit-marker with the
/// SAME prefix, the numbers must MATCH. Differing numbers under the
/// same prefix mean different units -- the model is wrong, drop.
///
/// If the filenames don't share a prefix in common, we have no
/// deterministic signal either way and let the model's confidence
/// stand (the calling code already enforces a 0.6 confidence floor).
///
/// `solution_of` edges are NOT subject to this filter -- there a
/// shared prefix WITH DIFFERENT NUMBERS is exactly wrong (lab2 +
/// lab3-solution), but a shared prefix WITH MATCHING NUMBER is the
/// strongest possible positive signal (lab2 + lab2-solution).
pub fn part_of_unit_passes_filename_check(a: &str, b: &str) -> bool {
    let ma = extract_unit_markers(a);
    let mb = extract_unit_markers(b);
    if ma.is_empty() || mb.is_empty() {
        // No deterministic signal; let the LLM confidence stand.
        return true;
    }
    // For every shared prefix, numbers must match in at least one
    // pairing. If the prefix is shared but no number matches, that's
    // strong evidence they're different units.
    for (pa, na) in &ma {
        for (pb, nb) in &mb {
            // Different prefix -> no signal at this pair, move on.
            // Same prefix but different number -> docs are in
            // different units, drop the edge.
            // Same prefix and same number -> positive signal, but
            // other prefixes might still disagree, so keep iterating
            // rather than short-circuit on a single pair.
            if pa == pb && na != nb {
                return false;
            }
        }
    }
    true
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
            continue;
        }
        if !valid_ids.contains(&src_id) || !valid_ids.contains(&dst_id) {
            // Model hallucinated an id we never sent in; drop.
            continue;
        }
        let relation = match e["relation"].as_str() {
            Some(r @ ("solution_of" | "part_of_unit")) => r.to_string(),
            _ => continue,
        };
        let confidence = e["confidence"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
        if confidence < MIN_EDGE_CONFIDENCE {
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
            r#"{{"edges":[{{"src_id":"{a}","dst_id":"{b}","relation":"solution_of","confidence":0.9,"rationale":"Solution to lab 2"}}]}}"#
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
            r#"{{"edges":[{{"src_id":"{a}","dst_id":"{b}","relation":"part_of_unit","confidence":0.85,"rationale":"week 3"}}]}}"#
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

    // ── Numeric-marker post-filter ────────────────────────────────

    #[test]
    fn unit_marker_extractor_finds_swedish_ovningsuppgift() {
        // The filename that triggered the original bug report:
        // model linked these two with rationale "Both filenames
        // contain the number 4" (which is wrong AND the numbers
        // didn't even match).
        let a = extract_unit_markers("ovningsuppgift3_vt25.pdf");
        let b = extract_unit_markers("ovningsuppgift4_vt25.pdf");
        // Each filename should yield exactly one marker, on the
        // longest-prefix-wins ovningsuppgift token. "vt25" doesn't
        // match any prefix.
        assert!(a.iter().any(|(p, n)| p == "ovningsuppgift" && *n == 3));
        assert!(b.iter().any(|(p, n)| p == "ovningsuppgift" && *n == 4));
    }

    #[test]
    fn unit_marker_extractor_handles_diacritics() {
        let a = extract_unit_markers("övning3.pdf");
        // Should fold ö -> o and find the marker.
        assert!(a.iter().any(|(p, n)| p == "ovning" && *n == 3));
    }

    #[test]
    fn unit_marker_extractor_handles_separators() {
        for fname in &["lab-2.pdf", "lab_2.pdf", "lab 2.pdf", "lab.2.pdf"] {
            let m = extract_unit_markers(fname);
            assert!(
                m.iter().any(|(p, n)| p == "lab" && *n == 2),
                "should extract lab 2 from {}",
                fname
            );
        }
    }

    #[test]
    fn unit_marker_extractor_skips_word_boundary_misses() {
        // "labour" contains "lab" but isn't a unit marker.
        // Currently we accept "lab1" but reject "labour1" because
        // there's no digit immediately after "lab" in "labour".
        // But we DO need to handle "lab" as a substring of e.g.
        // "lablab2" -- we still want to find lab2 there.
        let m = extract_unit_markers("labour.pdf");
        assert!(
            !m.iter().any(|(p, _)| p == "lab"),
            "labour shouldn't yield a lab marker: got {:?}",
            m
        );
    }

    #[test]
    fn unit_marker_extractor_finds_multiple_markers() {
        // "week3_lecture5.pdf" -> [("week", 3), ("lecture", 5)]
        let m = extract_unit_markers("week3_lecture5.pdf");
        assert!(m.iter().any(|(p, n)| p == "week" && *n == 3));
        assert!(m.iter().any(|(p, n)| p == "lecture" && *n == 5));
    }

    #[test]
    fn part_of_unit_filter_drops_the_user_reported_bug() {
        // The exact case from the user's bug report. Model emitted:
        //   ovningsuppgift3_vt25.pdf part_of_unit ovningsuppgift4_vt25.pdf
        //   (confidence 0.75, rationale "Both filenames contain the number 4")
        // Expected: post-filter drops it because the numeric markers
        // differ under the shared "ovningsuppgift" prefix.
        assert!(!part_of_unit_passes_filename_check(
            "ovningsuppgift3_vt25.pdf",
            "ovningsuppgift4_vt25.pdf"
        ));
        // Diacritic-bearing variant must also drop.
        assert!(!part_of_unit_passes_filename_check(
            "övningsuppgift3_vt25.pdf",
            "övningsuppgift4_vt25.pdf"
        ));
    }

    #[test]
    fn part_of_unit_filter_drops_adjacent_labs() {
        assert!(!part_of_unit_passes_filename_check(
            "lab2_brief.pdf",
            "lab3_brief.pdf"
        ));
        assert!(!part_of_unit_passes_filename_check(
            "week3.pdf",
            "week6.pdf"
        ));
        assert!(!part_of_unit_passes_filename_check(
            "chapter-04.pdf",
            "chapter-05.pdf"
        ));
    }

    #[test]
    fn part_of_unit_filter_keeps_matching_numbers() {
        // Same number, different role within the unit (lecture +
        // exercises, brief + handout): keep.
        assert!(part_of_unit_passes_filename_check(
            "week3_lecture.pdf",
            "week3_exercises.pdf"
        ));
        assert!(part_of_unit_passes_filename_check(
            "lab2_brief.pdf",
            "lab2_helper.pdf"
        ));
    }

    #[test]
    fn part_of_unit_filter_keeps_when_no_shared_prefix() {
        // No deterministic signal -- one has a marker, one doesn't.
        // Trust the model's confidence; don't over-filter.
        assert!(part_of_unit_passes_filename_check(
            "lecture3_intro.pdf",
            "syllabus.pdf"
        ));
        // Both have markers, but DIFFERENT prefixes (week vs lab).
        // No shared prefix means no filename-level disagreement;
        // could still be the same unit (week3 lecture + lab2 in same
        // course module, e.g.) -- not for this filter to decide.
        assert!(part_of_unit_passes_filename_check(
            "week3_intro.pdf",
            "lab2_brief.pdf"
        ));
    }

    #[test]
    fn part_of_unit_filter_passes_when_neither_has_markers() {
        assert!(part_of_unit_passes_filename_check(
            "intro.pdf",
            "course-overview.pdf"
        ));
    }

    // ── Embedding similarity / candidate generation ───────────────

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
    fn shares_matching_marker_user_reported_f18_case() {
        // F18_OO + F18_section_summary -> matching F18 marker -> true.
        // This is one of the cases the user pointed out the linker
        // got right; our marker detector should agree.
        assert!(shares_matching_marker(
            "F18 OO.pdf",
            "section-F18_-_Programspraak_objektorientering_section_summary.html"
        ));
    }

    #[test]
    fn shares_matching_marker_handles_f01_f02_disagreement() {
        // F01_Arv_I and F02_Arv_II have different "F" numbers -- the
        // marker detector should NOT report them as sharing a marker.
        // (The model can still propose part_of_unit on content
        // grounds, but filename markers are NOT positive evidence.)
        assert!(!shares_matching_marker("F01_Arv_I.pdf", "F02_Arv_II.pdf"));
    }

    #[test]
    fn part_of_unit_filter_drops_f01_f02_lecture_pair() {
        // The user's other reported false-positive: F02_Arv_II linked
        // to F01_Arv_I "by shared 'Arv'". With the F-prefix in the
        // unit-marker list, the post-filter now drops these even
        // though both filenames contain the "Arv" topic word.
        assert!(!part_of_unit_passes_filename_check(
            "F01_Arv_I.pdf",
            "F02_Arv_II.pdf"
        ));
    }

    #[test]
    fn matched_marker_token_returns_lowercased_concat() {
        let tok = matched_marker_token("Lab2_brief.pdf", "lab2_solution.pdf");
        assert_eq!(tok.as_deref(), Some("lab2"));
    }

    #[test]
    fn build_similarity_matrix_picks_top_k_above_floor() {
        // Mocked DocumentRow -- we only need id + status + kind to
        // get past the linker's gating, but build_similarity_matrix
        // doesn't filter, just iterates docs. So we can pass tiny
        // shells.
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
