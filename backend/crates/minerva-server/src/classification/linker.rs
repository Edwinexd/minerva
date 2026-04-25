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
  "rationale": short string explaining why
}

Edge semantics:
- "solution_of": src is a sample_solution document; dst is the assignment_brief / lab_brief / exam it answers. Only emit when both filename signal and the kinds line up clearly. Skip if uncertain.
- "part_of_unit": both documents belong to the same week / module / topic / unit. Use filename numbering ("week3", "lab2", "module-04"), unit hints in rationales, and explicit cross-references. Edges are stored once with the lower id as src and higher id as dst -- you do NOT need to enforce that, the application will normalise direction.

Rules:
- Be conservative. Edges with confidence < 0.6 will be dropped on the application side, but emitting many low-confidence edges still wastes tokens.
- Do NOT emit self-loops (src_id == dst_id).
- Do NOT emit edges that reference an id not present in the input.
- A single solution can be solution_of multiple things only when the same solution document actually covers multiple assignments; usually it's 1:1.
- For "part_of_unit", you do not need to emit every pair within a unit -- a sparse cover is fine, the application reads it as transitive grouping.

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

    parse_edges(raw, &id_set).map(|edges| LinkerOutput {
        edges,
        considered: truncated.len(),
    })
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
