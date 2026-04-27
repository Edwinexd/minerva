//! Adversarial pre-retrieval filter: a per-chunk yes/no check that runs
//! at chat time, after RAG retrieval but before chunks are pasted into
//! the prompt context. Catches the rare case where a `sample_solution`
//! chunk slipped past the per-doc classifier (or where a `lecture` doc
//! happens to contain a worked solution); the per-doc kind is right
//! at the document level but a single chunk inside it might still leak.
//!
//! This is the belt-and-suspenders layer. The primary defense is the
//! ingest-time classifier which excludes whole documents. This layer
//! catches per-chunk leaks within otherwise-safe documents.
//!
//! Cost / latency budget:
//! * Per-chunk: one cheap llama3.1-8b call, ~100 tokens in / 5 tokens
//!   out, target round-trip ~150-250ms each.
//! * The strategy fans out concurrently across all retrieved chunks via
//!   `futures::future::join_all`, so total wall-clock is roughly the
//!   slowest single call (not the sum).
//! * A wrapping `tokio::time::timeout` keeps the whole filter under
//!   `MAX_FILTER_LATENCY`; if we time out, we fail OPEN; pass all
//!   chunks through. This is intentional: the primary defense already
//!   ran, and blocking student replies for a defensive secondary is
//!   worse than the small leak risk.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::future::join_all;

use sqlx::PgPool;
use uuid::Uuid;

use crate::strategy::common::{cerebras_request_with_retry, record_cerebras_usage, RagChunk};
use minerva_db::queries::course_token_usage::CATEGORY_ADVERSARIAL_FILTER;

/// Atomic counters bumped by every filter invocation. Surfaced via a
/// dedicated read API so the admin/telemetry dashboard can show how
/// often the secondary defense fires vs how often the primary
/// (per-doc kind) catches everything. All counters are monotonic
/// across a single server instance; reset on restart.
static CHUNKS_INSPECTED: AtomicU64 = AtomicU64::new(0);
static CHUNKS_DROPPED: AtomicU64 = AtomicU64::new(0);
static CHUNKS_PASSED: AtomicU64 = AtomicU64::new(0);
static CHUNKS_PER_CHECK_FAILED: AtomicU64 = AtomicU64::new(0);
static FILTER_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

/// Snapshot of the filter's lifetime counters for telemetry. Used by
/// the admin dashboard via `crate::routes::admin::adversarial_stats`.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // fields read via Debug/serde from a planned admin endpoint
pub struct AdversarialStats {
    pub chunks_inspected: u64,
    pub chunks_dropped: u64,
    pub chunks_passed: u64,
    pub per_check_failures: u64,
    pub filter_timeouts: u64,
}

#[allow(dead_code)] // referenced from a planned admin telemetry endpoint
pub fn snapshot_stats() -> AdversarialStats {
    AdversarialStats {
        chunks_inspected: CHUNKS_INSPECTED.load(Ordering::Relaxed),
        chunks_dropped: CHUNKS_DROPPED.load(Ordering::Relaxed),
        chunks_passed: CHUNKS_PASSED.load(Ordering::Relaxed),
        per_check_failures: CHUNKS_PER_CHECK_FAILED.load(Ordering::Relaxed),
        filter_timeouts: FILTER_TIMEOUTS.load(Ordering::Relaxed),
    }
}

/// Cerebras model used for the per-chunk check. Binary
/// classification with a 4-token output cap; exactly the kind of
/// small-model task where llama3.1-8b is the right tool. Cheaper,
/// faster (matters here: this filter runs per-chunk and fans out
/// across all retrieved chunks every chat turn, against an 800ms
/// total budget). The body deliberately omits `reasoning_effort`:
/// the parameter is gpt-oss-only and Cerebras 400s the request
/// when llama sees it; the previous body included it and was
/// silently failing on every call (4xx -> Err -> fail-open without
/// recording usage, so the dashboard didn't even surface the calls).
const ADVERSARIAL_MODEL: &str = "llama3.1-8b";

/// Total wall-clock budget for the whole filter (across all chunks
/// fanned out concurrently). On timeout the filter fails open.
const MAX_FILTER_LATENCY: Duration = Duration::from_millis(800);

/// Tiny excerpt cap for latency. The chunker already produces ~1000
/// char chunks; this is just a sanity guard against oversized outliers.
const MAX_EXCERPT_CHARS: usize = 4_000;

/// Single tight prompt. Asks for a strict yes/no. We don't use the
/// structured-output JSON schema here; the response is a single token
/// and the latency saving matters.
const ADVERSARIAL_SYSTEM_PROMPT: &str = "You are a strict classifier. Decide whether the given excerpt is a worked-out solution to a graded exercise (an answer key, model solution, walkthrough labelled \"solution\"/\"answer\"). Examples in lectures, derivations of definitions, and demonstrations of techniques are NOT solutions to graded exercises; those are teaching material. Reply with exactly one word: \"yes\" or \"no\". No punctuation, no explanation.";

/// Per-chunk check. Returns true iff the model says this chunk is a
/// worked solution (and so should be excluded from the prompt context).
/// Errors fail open (return false); defense in depth, not the
/// primary gate.
///
/// Records the call's prompt/completion token usage against
/// `course_id` in the `adversarial_filter` category. Recording is
/// best-effort; a DB failure here never affects the chat path.
async fn is_solution_chunk(
    http: &reqwest::Client,
    api_key: &str,
    db: &PgPool,
    course_id: Uuid,
    chunk_text: &str,
) -> bool {
    let excerpt: String = chunk_text.chars().take(MAX_EXCERPT_CHARS).collect();

    let body = serde_json::json!({
        "model": ADVERSARIAL_MODEL,
        "temperature": 0.0,
        "max_tokens": 4,
        "messages": [
            { "role": "system", "content": ADVERSARIAL_SYSTEM_PROMPT },
            { "role": "user", "content": excerpt },
        ],
    });

    let response = match cerebras_request_with_retry(http, api_key, &body).await {
        Ok(r) => r,
        Err(e) => {
            CHUNKS_PER_CHECK_FAILED.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("adversarial: request failed, failing open: {e}");
            return false;
        }
    };

    let payload: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            CHUNKS_PER_CHECK_FAILED.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("adversarial: response not JSON, failing open: {e}");
            return false;
        }
    };

    record_cerebras_usage(
        db,
        course_id,
        CATEGORY_ADVERSARIAL_FILTER,
        ADVERSARIAL_MODEL,
        &payload,
    )
    .await;

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
    db: &PgPool,
    course_id: Uuid,
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
        .map(|t| is_solution_chunk(http, api_key, db, course_id, t))
        .collect::<Vec<_>>();

    let started = Instant::now();
    let chunks_count = chunks.len();
    CHUNKS_INSPECTED.fetch_add(chunks_count as u64, Ordering::Relaxed);

    let verdicts = match tokio::time::timeout(MAX_FILTER_LATENCY, join_all(checks)).await {
        Ok(v) => v,
        Err(_) => {
            FILTER_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            // Count timed-out chunks as "passed" for the bookkeeping
            // model: they reach the prompt context, just without
            // having been verified. The metric reader can subtract
            // FILTER_TIMEOUTS if they want a stricter view.
            CHUNKS_PASSED.fetch_add(chunks_count as u64, Ordering::Relaxed);
            tracing::warn!(
                "adversarial: filter exceeded {}ms budget across {} chunks; passing all through (elapsed {}ms)",
                MAX_FILTER_LATENCY.as_millis(),
                chunks_count,
                started.elapsed().as_millis(),
            );
            return chunks;
        }
    };

    let mut kept = Vec::with_capacity(chunks.len());
    let mut dropped_in_call = 0u64;
    for (chunk, is_solution) in chunks.into_iter().zip(verdicts) {
        if is_solution {
            CHUNKS_DROPPED.fetch_add(1, Ordering::Relaxed);
            dropped_in_call += 1;
            tracing::warn!(
                "adversarial: dropping chunk from doc {} (filename {}) flagged as solution",
                chunk.document_id,
                chunk.filename,
            );
            continue;
        }
        CHUNKS_PASSED.fetch_add(1, Ordering::Relaxed);
        kept.push(chunk);
    }
    if dropped_in_call > 0 {
        tracing::info!(
            "adversarial: filter pass dropped {} of {} chunks in {}ms",
            dropped_in_call,
            chunks_count,
            started.elapsed().as_millis(),
        );
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

    /// `connect_lazy` doesn't open a connection until the pool is
    /// actually used; the early-return paths in the tests below
    /// never touch the DB, so a bogus URL is fine. Keeps these
    /// tests free of a Postgres dependency.
    fn lazy_pool() -> PgPool {
        PgPool::connect_lazy("postgres://test:test@127.0.0.1:1/test").unwrap()
    }

    #[tokio::test]
    async fn empty_input_returns_empty() {
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let out = filter_solution_chunks(&http, "key", &db, Uuid::nil(), vec![]).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn empty_api_key_skips_filter() {
        // No HTTP traffic should be attempted when the key is empty;
        // the filter must pass chunks through untouched. Sanity-check
        // for dev/test setups where CEREBRAS_API_KEY is unset.
        let http = reqwest::Client::new();
        let db = lazy_pool();
        let chunks = vec![
            chunk("d1", "foo.pdf", "Lecture content"),
            chunk("d2", "bar.pdf", "More lecture content"),
        ];
        let out = filter_solution_chunks(&http, "", &db, Uuid::nil(), chunks.clone()).await;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].document_id, "d1");
        assert_eq!(out[1].document_id, "d2");
    }
}
