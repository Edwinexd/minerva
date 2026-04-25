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

use std::collections::HashMap;

use minerva_db::queries::documents::DocumentRow;
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
];

const LINKER_MODEL: &str = "gpt-oss-120b";

/// Drop edges the model emits below this confidence. Tightening the
/// threshold here (vs. saving everything and filtering at read time)
/// keeps the relations table sparse, which keeps the graph viewer
/// readable.
const MIN_EDGE_CONFIDENCE: f32 = 0.6;

/// Hard cap on docs sent to the linker in one call. Keeps the prompt
/// token cost bounded; courses larger than this would need pagination
/// (not implemented in V1 -- DSV courses are well under the cap).
const MAX_DOCS_PER_CALL: usize = 300;

const LINKER_SYSTEM_PROMPT: &str = r#"You build the edge set of a small course-document knowledge graph.

You will receive a JSON array describing every classified document in a course. Each entry has:
- "id": opaque identifier (UUID-shaped string)
- "filename"
- "kind": one of "lecture", "reading", "assignment_brief", "sample_solution", "lab_brief", "exam", "syllabus", "unknown"
- "rationale": a short note from the per-document classifier

You return a JSON object with an "edges" array. Each edge is one object:
{
  "src_id": <id from the input>,
  "dst_id": <id from the input>,
  "relation": one of "solution_of" | "part_of_unit",
  "confidence": float in [0.0, 1.0],
  "rationale": short specific string explaining the evidence (filename overlap, shared topic word, etc.)
}

==============================
Edge semantics and HARD rules
==============================

"solution_of":
  src is a sample_solution document; dst is the assignment_brief / lab_brief / exam it answers.
  Strong signal: dst's filename appears as a stem/prefix inside src's filename
    (e.g. "lab2.pdf" + "lab2_solution.pdf", "uppgift3.pdf" + "uppgift3_facit.pdf").
  Emit only when filename signal AND kinds line up. Skip if uncertain.

"part_of_unit":
  Two documents belong to the same week / module / chapter / topic / unit.
  ONLY emit when there is a CONCRETE shared marker that ties them to the
  same unit. Examples of valid markers:
    * Identical week / lab / chapter / module number tokens
      ("week3" + "week3", "lab2_brief" + "lab2_solution", "ch04" + "ch04_exercises").
    * The same explicit unit name in both filenames
      ("module-trees" + "module-trees-quiz").
    * A shared topic word that is unambiguous AND specific
      ("recursion-lecture" + "recursion-exercises", NOT "lecture" + "exercises").
  HARD CONSTRAINT: if both filenames carry a numeric marker (week3, lab4,
  uppgift2, chapter-05, etc.) and the numbers DIFFER, the docs are NOT
  in the same unit -- they are in adjacent units. Do not link them.
  Example of what NOT to emit:
    bad: "ovningsuppgift3_vt25.pdf" part_of_unit "ovningsuppgift4_vt25.pdf"
         (different numbers -> different exercises -> different units)
    bad: "lab2.pdf" part_of_unit "lab3.pdf" (adjacent labs are NOT same unit)
    bad: "week5.pdf" part_of_unit "week6.pdf"
  When unsure, DO NOT emit the edge.

==============================
Anti-hallucination rules
==============================

The "rationale" field must describe REAL evidence visible in the inputs.
Do NOT invent shared tokens, do NOT claim numbers match when they don't,
do NOT cite cross-references that aren't in the data you were given.
A vague rationale like "both are exercises" is not sufficient evidence
for an edge -- skip the edge instead.

==============================
Other rules
==============================

- Be conservative. Edges with confidence < 0.6 will be dropped on the
  application side, but emitting many low-confidence edges still wastes
  tokens, and the application also runs a deterministic post-filter that
  drops "part_of_unit" edges between docs with differing numeric markers.
- Do NOT emit self-loops (src_id == dst_id).
- Do NOT emit edges that reference an id not present in the input.
- A single solution can be solution_of multiple things only when the
  same solution document actually covers multiple assignments; usually
  it's 1:1.
- For "part_of_unit", a sparse cover is fine -- you do not need to emit
  every pair within a unit. One edge per pair of clearly-related docs.

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

/// Run the cross-doc linker over a course's classified documents.
///
/// Empty / single-doc / unclassified-only courses return early with
/// no edges -- there's nothing useful for the linker to do.
pub async fn link_course(
    http: &reqwest::Client,
    api_key: &str,
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

    if api_key.is_empty() {
        // Dev / test env without CEREBRAS_API_KEY. Skip rather than
        // burn time on a guaranteed-401 call.
        return Ok(LinkerOutput {
            edges: Vec::new(),
            considered: classified.len(),
        });
    }

    let truncated = if classified.len() > MAX_DOCS_PER_CALL {
        tracing::warn!(
            "linker: course has {} classified docs, capping linker input at {} (V1 doesn't paginate)",
            classified.len(),
            MAX_DOCS_PER_CALL,
        );
        &classified[..MAX_DOCS_PER_CALL]
    } else {
        &classified[..]
    };

    let id_set: std::collections::HashSet<Uuid> = truncated.iter().map(|d| d.id).collect();

    let input_docs: Vec<serde_json::Value> = truncated
        .iter()
        .map(|d| {
            serde_json::json!({
                "id": d.id.to_string(),
                "filename": d.filename,
                "kind": d.kind.as_deref().unwrap_or("unknown"),
                "rationale": d.kind_rationale.as_deref().unwrap_or(""),
            })
        })
        .collect();

    let body = serde_json::json!({
        "model": LINKER_MODEL,
        "temperature": 0.0,
        "reasoning_effort": "medium",
        "messages": [
            { "role": "system", "content": LINKER_SYSTEM_PROMPT },
            {
                "role": "user",
                "content": serde_json::Value::Array(input_docs).to_string(),
            },
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

    let edges = parse_edges(raw, &id_set)?;

    // Deterministic post-filter for `part_of_unit`. The model
    // hallucinates rationales (e.g. claiming "both contain the
    // number 4" when filenames are "exercise3" and "exercise4"); a
    // pure-Rust filename check is the cheapest way to catch those
    // misfires. solution_of edges are NOT filtered here -- their
    // numeric-marker semantics are the OPPOSITE (a matching number
    // is positive signal).
    let filenames_by_id: HashMap<Uuid, &str> = truncated
        .iter()
        .map(|d| (d.id, d.filename.as_str()))
        .collect();
    let mut kept = Vec::with_capacity(edges.len());
    let mut dropped = 0usize;
    for edge in edges {
        if edge.relation == "part_of_unit" {
            let a = filenames_by_id.get(&edge.src_id).copied().unwrap_or("");
            let b = filenames_by_id.get(&edge.dst_id).copied().unwrap_or("");
            if !part_of_unit_passes_filename_check(a, b) {
                tracing::info!(
                    "linker: post-filter dropped part_of_unit between {:?} ({}) and {:?} ({}) -- numeric markers disagree",
                    edge.src_id,
                    a,
                    edge.dst_id,
                    b,
                );
                dropped += 1;
                continue;
            }
        }
        kept.push(edge);
    }
    if dropped > 0 {
        tracing::info!(
            "linker: post-filter dropped {} part_of_unit edge(s) for filename-marker mismatch",
            dropped
        );
    }

    Ok(LinkerOutput {
        edges: kept,
        considered: truncated.len(),
    })
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
