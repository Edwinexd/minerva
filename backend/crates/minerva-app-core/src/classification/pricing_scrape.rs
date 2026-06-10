//! Best-effort "scrape price" helper for the chat-model admin UI.
//!
//! Fetches a provider's public pricing page server-side, strips it to
//! text, and asks the configured utility model to extract the per-Mtok
//! input/output rates for one model id. The result is a *suggestion*
//! only: it is never persisted automatically. The admin reviews, edits
//! if needed, and saves through the normal price PUT. Pricing pages are
//! frequently JS-rendered, so a plain fetch may return thin HTML; in
//! that case the suggestion comes back empty with a note and the admin
//! enters the numbers by hand.

use std::time::Duration;

use crate::llm::{ChatRequest, LlmRegistry};

/// Hard caps so a hostile / huge pricing page can't blow up memory or
/// the utility-model prompt.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PAGE_BYTES: usize = 512 * 1024;
const MAX_TEXT_CHARS: usize = 16_000;

/// Admin-reviewed price suggestion. Serialized straight to the scrape
/// route response; nothing here is written to the DB.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PriceSuggestion {
    pub input_usd_per_mtok: Option<f64>,
    pub output_usd_per_mtok: Option<f64>,
    /// Model's self-reported confidence in [0, 1], if it gave one.
    pub confidence: Option<f64>,
    /// Free-text caveat from the extractor (e.g. "page listed tiered
    /// pricing; used the standard tier").
    pub note: Option<String>,
    pub source_url: String,
    /// False when the page fetch yielded little usable text; the admin
    /// should treat any numbers with extra suspicion (or enter manually).
    pub page_fetched: bool,
}

/// Shape the utility model is asked to return. All fields optional so a
/// failed / partial extraction degrades gracefully.
#[derive(Debug, Default, serde::Deserialize)]
struct ExtractedPrice {
    input_usd_per_mtok: Option<f64>,
    output_usd_per_mtok: Option<f64>,
    confidence: Option<f64>,
    note: Option<String>,
}

/// Fetch `pricing_url`, extract rates for `target_model` via the utility
/// model, and return a suggestion. Errors only for setup problems (no
/// utility model configured, its provider key absent); a failed page
/// fetch or extraction returns an empty-but-valid suggestion.
pub async fn scrape_price(
    registry: &LlmRegistry,
    db: &sqlx::PgPool,
    http: &reqwest::Client,
    target_model: &str,
    pricing_url: &str,
) -> Result<PriceSuggestion, String> {
    // 1. Fetch + strip the pricing page (best-effort).
    let (page_text, page_fetched) = match fetch_page_text(http, pricing_url).await {
        Ok(text) if !text.trim().is_empty() => (text, true),
        _ => (String::new(), false),
    };

    if !page_fetched {
        return Ok(PriceSuggestion {
            input_usd_per_mtok: None,
            output_usd_per_mtok: None,
            confidence: None,
            note: Some(
                "Could not fetch usable text from the pricing page (it may be \
                 JavaScript-rendered). Enter the rates manually."
                    .to_string(),
            ),
            source_url: pricing_url.to_string(),
            page_fetched: false,
        });
    }

    // 2. Resolve the utility model + its provider.
    let utility_model = minerva_db::queries::chat_models::current_utility_default(db)
        .await
        .map_err(|e| format!("utility model lookup failed: {e}"))?
        .ok_or_else(|| "no utility model configured".to_string())?;
    let provider_id = minerva_db::queries::chat_models::provider_of(db, &utility_model)
        .await
        .map_err(|e| format!("provider lookup failed: {e}"))?
        .ok_or_else(|| "utility model not in catalog".to_string())?;
    let provider = registry
        .get(&provider_id)
        .ok_or_else(|| format!("utility model provider '{provider_id}' is not configured"))?;

    // 3. Ask the utility model to extract the rates as JSON.
    let system = "You extract LLM API prices from a pricing page. Given the page \
        text and a target model id, return ONLY a JSON object with keys \
        input_usd_per_mtok, output_usd_per_mtok (USD per 1,000,000 tokens, numbers), \
        confidence (0 to 1), and note (short string). If the page does not list the \
        model, return nulls for the prices and explain in note. Do not include any \
        text outside the JSON object.";
    let user = format!(
        "Target model id: {target_model}\n\nPricing page text:\n{}",
        page_text
    );
    let messages = vec![
        serde_json::json!({ "role": "system", "content": system }),
        serde_json::json!({ "role": "user", "content": user }),
    ];
    let req = ChatRequest {
        model: &utility_model,
        messages: &messages,
        temperature: 0.0,
        max_tokens: Some(300),
        stream: false,
        logprobs: false,
        response_format: None,
        extra: None,
    };

    let (reply, _usage) = provider
        .complete(req)
        .await
        .map_err(|e| format!("price extraction call failed: {e}"))?;

    // 4. Parse defensively: pull the first {...} block out of the reply.
    let extracted = parse_json_object(&reply).unwrap_or_default();

    Ok(PriceSuggestion {
        input_usd_per_mtok: extracted.input_usd_per_mtok,
        output_usd_per_mtok: extracted.output_usd_per_mtok,
        confidence: extracted.confidence,
        note: extracted.note,
        source_url: pricing_url.to_string(),
        page_fetched: true,
    })
}

/// GET the page (with a timeout + size cap) and strip HTML to plain text.
async fn fetch_page_text(http: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = http
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let capped = &bytes[..bytes.len().min(MAX_PAGE_BYTES)];
    let html = String::from_utf8_lossy(capped);
    Ok(strip_html(&html))
}

/// Crude HTML-to-text: drop script/style blocks, remove tags, collapse
/// whitespace, truncate. Good enough to feed an extractor; we are not
/// trying to render the page.
fn strip_html(html: &str) -> String {
    let no_scripts = regex::Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>")
        .expect("static regex")
        .replace_all(html, " ");
    let no_tags = regex::Regex::new(r"(?s)<[^>]*>")
        .expect("static regex")
        .replace_all(&no_scripts, " ");
    let collapsed = regex::Regex::new(r"\s+")
        .expect("static regex")
        .replace_all(&no_tags, " ");
    collapsed.trim().chars().take(MAX_TEXT_CHARS).collect()
}

/// Extract the first balanced-ish `{...}` JSON object from a model reply
/// and deserialize it. Returns `None` if no object parses.
fn parse_json_object(reply: &str) -> Option<ExtractedPrice> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&reply[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_drops_tags_and_scripts() {
        let html = "<html><head><style>.a{color:red}</style></head>\
            <body><script>var x=1</script><p>Input $0.35 / 1M</p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Input $0.35 / 1M"));
        assert!(!text.contains("color:red"));
        assert!(!text.contains("var x"));
    }

    #[test]
    fn parse_json_object_pulls_object_from_prose() {
        let reply = "Here you go: {\"input_usd_per_mtok\": 0.35, \
            \"output_usd_per_mtok\": 0.75, \"confidence\": 0.9} thanks";
        let p = parse_json_object(reply).unwrap();
        assert_eq!(p.input_usd_per_mtok, Some(0.35));
        assert_eq!(p.output_usd_per_mtok, Some(0.75));
        assert_eq!(p.confidence, Some(0.9));
    }

    #[test]
    fn parse_json_object_none_when_absent() {
        assert!(parse_json_object("no json here").is_none());
    }
}
