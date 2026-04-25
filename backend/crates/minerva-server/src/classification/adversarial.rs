//! Adversarial pre-retrieval filter: a per-chunk yes/no check that runs
//! at chat time, after RAG retrieval but before chunks are pasted into
//! the prompt context. Catches the rare case where a `sample_solution`
//! chunk slipped past the per-doc classifier (or where a `lecture` doc
//! happens to contain a worked solution) -- the per-doc kind is right
//! at the document level but a single chunk inside it might still leak.
//!
//! This is the belt-and-suspenders layer. The primary defense is the
//! ingest-time classifier which excludes whole documents. This layer
//! catches per-chunk leaks within otherwise-safe documents.
//!
//! Cost / latency budget:
//! * Per-chunk: one cheap gpt-oss-120b call, very low effort, ~100 tokens
//!   in / 5 tokens out, target round-trip ~150-250ms each.
//! * The strategy fans out concurrently across all retrieved chunks via
//!   `futures::future::join_all`, so total wall-clock is roughly the
//!   slowest single call (not the sum).
//! * A wrapping `tokio::time::timeout` keeps the whole filter under
//!   `MAX_FILTER_LATENCY`; if we time out, we fail OPEN -- pass all
//!   chunks through. This is intentional: the primary defense already
//!   ran, and blocking student replies for a defensive secondary is
//!   worse than the small leak risk.

use std::time::Duration;

use futures::future::join_all;

use crate::strategy::common::{cerebras_request_with_retry, RagChunk};

/// Cerebras model used for the per-chunk check. Same family as the
/// document classifier so we benefit from a single warmed-up cache.
const ADVERSARIAL_MODEL: &str = "gpt-oss-120b";

/// Total wall-clock budget for the whole filter (across all chunks
/// fanned out concurrently). On timeout the filter fails open.
const MAX_FILTER_LATENCY: Duration = Duration::from_millis(800);

/// Tiny excerpt cap for latency. The chunker already produces ~1000
/// char chunks; this is just a sanity guard against oversized outliers.
const MAX_EXCERPT_CHARS: usize = 4_000;

/// Single tight prompt. Asks for a strict yes/no. We don't use the
/// structured-output JSON schema here -- the response is a single token
/// and the latency saving matters.
const ADVERSARIAL_SYSTEM_PROMPT: &str = "You are a strict classifier. Decide whether the given excerpt is a worked-out solution to a graded exercise (an answer key, model solution, walkthrough labelled \"solution\"/\"answer\"). Examples in lectures, derivations of definitions, and demonstrations of techniques are NOT solutions to graded exercises -- those are teaching material. Reply with exactly one word: \"yes\" or \"no\". No punctuation, no explanation.";

/// Per-chunk check. Returns true iff the model says this chunk is a
/// worked solution (and so should be excluded from the prompt context).
/// Errors fail open (return false) -- defense in depth, not the
/// primary gate.
async fn is_solution_chunk(http: &reqwest::Client, api_key: &str, chunk_text: &str) -> bool {
    let excerpt: String = chunk_text.chars().take(MAX_EXCERPT_CHARS).collect();

    let body = serde_json::json!({
        "model": ADVERSARIAL_MODEL,
        "temperature": 0.0,
        "reasoning_effort": "low",
        "max_tokens": 4,
        "messages": [
            { "role": "system", "content": ADVERSARIAL_SYSTEM_PROMPT },
            { "role": "user", "content": excerpt },
        ],
    });

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("adversarial: request failed, failing open: {e}");
            return false;
        }
    };

    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("adversarial: response not JSON, failing open: {e}");
            return false;
        }
    };

    let raw = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .trim_matches(|c: char| !c.is_alphabetic())
        .to_lowercase();

    raw == "yes"
}

/// Filter chunks through the adversarial check, dropping any flagged
/// `yes`. Runs concurrently across chunks with a single wall-clock
/// timeout for the whole batch; on timeout returns the input unchanged
/// (fails open).
pub async fn filter_solution_chunks(
    http: &reqwest::Client,
    api_key: &str,
    chunks: Vec<RagChunk>,
) -> Vec<RagChunk> {
    if chunks.is_empty() {
        return chunks;
    }

    // Empty key (e.g. tests / dev without CEREBRAS_API_KEY): skip the
    // filter rather than make a guaranteed-401 call per chunk.
    if api_key.is_empty() {
        return chunks;
    }

    let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let checks = texts
        .iter()
        .map(|t| is_solution_chunk(http, api_key, t))
        .collect::<Vec<_>>();

    let verdicts = match tokio::time::timeout(MAX_FILTER_LATENCY, join_all(checks)).await {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "adversarial: filter exceeded {}ms budget across {} chunks; passing all through",
                MAX_FILTER_LATENCY.as_millis(),
                chunks.len(),
            );
            return chunks;
        }
    };

    let mut kept = Vec::with_capacity(chunks.len());
    for (chunk, is_solution) in chunks.into_iter().zip(verdicts) {
        if is_solution {
            tracing::warn!(
                "adversarial: dropping chunk from doc {} (filename {}) flagged as solution",
                chunk.document_id,
                chunk.filename,
            );
            continue;
        }
        kept.push(chunk);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(doc: &str, name: &str, text: &str) -> RagChunk {
        RagChunk {
            document_id: doc.to_string(),
            filename: name.to_string(),
            text: text.to_string(),
            kind: None,
            score: 0.5,
        }
    }

    #[tokio::test]
    async fn empty_input_returns_empty() {
        let http = reqwest::Client::new();
        let out = filter_solution_chunks(&http, "key", vec![]).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn empty_api_key_skips_filter() {
        // No HTTP traffic should be attempted when the key is empty;
        // the filter must pass chunks through untouched. Sanity-check
        // for dev/test setups where CEREBRAS_API_KEY is unset.
        let http = reqwest::Client::new();
        let chunks = vec![
            chunk("d1", "foo.pdf", "Lecture content"),
            chunk("d2", "bar.pdf", "More lecture content"),
        ];
        let out = filter_solution_chunks(&http, "", chunks.clone()).await;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].document_id, "d1");
        assert_eq!(out[1].document_id, "d2");
    }
}
