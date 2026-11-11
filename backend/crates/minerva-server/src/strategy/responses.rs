//! The research phase on OpenAI's Responses API.
//!
//! OpenAI's reasoning models (the `gpt-6` family among them) refuse
//! function tools on `/v1/chat/completions` unless reasoning is off, and
//! reasoning is what makes them worth using for research. So a research
//! round served by OpenAI goes to `/v1/responses`, which takes tools and
//! reasoning together. Every other provider keeps chat completions.
//!
//! The research loop keeps its history in chat-completions shape. This
//! module converts it into Responses input items per round and parses the
//! stream's events into the same `(content, tool calls, usage)` the chat
//! path produces. Requests are stateless (`store: false`): the reasoning of a
//! round comes back encrypted, rides on the assistant history entry under
//! [`OUTPUT_ITEMS_KEY`], and is replayed verbatim next round, so the model
//! keeps its chain of thought across tool calls without OpenAI keeping
//! the conversation. [`strip_output_items`] removes it again before any
//! chat-completions request, which would reject the unknown field.

use serde_json::{json, Value};

use minerva_app_core::llm::provider::PROVIDER_OPENAI;

/// Key on an assistant history entry holding the raw output items of the
/// Responses round that produced it (reasoning, function calls, text).
pub const OUTPUT_ITEMS_KEY: &str = "minerva_responses_output";

/// Reasoning effort for research rounds.
pub const REASONING_EFFORT: &str = "medium";

/// Whether a research round served by `provider_id` uses the Responses API.
pub fn uses_responses_api(provider_id: &str) -> bool {
    provider_id == PROVIDER_OPENAI
}

/// The Responses endpoint beside a chat-completions URL.
pub fn responses_url(chat_url: &str) -> String {
    let base = chat_url
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions");
    format!("{base}/responses")
}

/// History in chat-completions shape, without the Responses output items.
pub fn strip_output_items(history: &[Value]) -> Vec<Value> {
    history
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if let Some(obj) = m.as_object_mut() {
                obj.remove(OUTPUT_ITEMS_KEY);
            }
            m
        })
        .collect()
}

/// A research round's request body. `model` is the catalog id; the caller's
/// transport applies the `@flex` service tier.
pub fn build_body(
    model: &str,
    instructions: &str,
    history: &[Value],
    catalog: &[Value],
    max_output_tokens: i32,
) -> Value {
    json!({
        "model": model,
        "instructions": instructions,
        "input": to_input(history),
        "tools": to_tools(catalog),
        "tool_choice": "auto",
        "reasoning": { "effort": REASONING_EFFORT },
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true,
        "max_output_tokens": max_output_tokens,
    })
}

/// Chat-completions history as Responses input items. An assistant entry
/// that came from a Responses round is replayed as that round's own output
/// items (its encrypted reasoning included); any other entry is converted.
fn to_input(history: &[Value]) -> Vec<Value> {
    let mut input = Vec::new();
    for m in history {
        if let Some(items) = m.get(OUTPUT_ITEMS_KEY).and_then(Value::as_array) {
            input.extend(items.iter().cloned());
            continue;
        }
        match m["role"].as_str().unwrap_or_default() {
            "tool" => input.push(json!({
                "type": "function_call_output",
                "call_id": m["tool_call_id"],
                "output": m["content"].as_str().unwrap_or_default(),
            })),
            "assistant" => {
                let text = m["content"].as_str().unwrap_or_default();
                if !text.is_empty() {
                    input.push(json!({ "role": "assistant", "content": text }));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call["id"],
                        "name": call["function"]["name"],
                        "arguments": call["function"]["arguments"],
                    }));
                }
            }
            role => input.push(json!({
                "role": if role == "system" { "developer" } else { role },
                "content": m["content"].as_str().unwrap_or_default(),
            })),
        }
    }
    input
}

/// Chat-completions tool specs (`{type, function: {...}}`) as Responses
/// function tools. Not strict: the specs have optional parameters, which
/// strict mode forbids.
fn to_tools(catalog: &[Value]) -> Vec<Value> {
    catalog
        .iter()
        .map(|t| {
            let f = &t["function"];
            json!({
                "type": "function",
                "name": f["name"],
                "description": f["description"],
                "parameters": f["parameters"],
                "strict": false,
            })
        })
        .collect()
}

/// One function call the model asked for.
#[derive(Debug, PartialEq)]
pub struct FunctionCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// What one streamed round produced.
#[derive(Debug, Default)]
pub struct RoundOutput {
    pub content: String,
    pub calls: Vec<FunctionCall>,
    /// The round's output items, to replay next round.
    pub items: Vec<Value>,
    pub input_tokens: i32,
    pub output_tokens: i32,
}

/// One event of a streamed Responses round, as the research loop needs it.
#[derive(Debug)]
pub enum StreamEvent {
    /// Visible text, streamed as the live thinking trace.
    Text(String),
    /// The round finished (or hit its output cap): the function calls, the
    /// items to replay and the usage.
    Done(RoundOutput),
    Failed(String),
    Other,
}

/// Read one SSE `data:` payload of a Responses stream.
pub fn parse_event(event: &Value) -> StreamEvent {
    match event["type"].as_str().unwrap_or_default() {
        "response.output_text.delta" => match event["delta"].as_str() {
            Some(delta) if !delta.is_empty() => StreamEvent::Text(delta.to_string()),
            _ => StreamEvent::Other,
        },
        "response.completed" | "response.incomplete" => {
            let mut out = RoundOutput::default();
            finish(&mut out, &event["response"]);
            StreamEvent::Done(out)
        }
        "response.failed" => StreamEvent::Failed(
            event["response"]["error"]["message"]
                .as_str()
                .unwrap_or("response failed")
                .to_string(),
        ),
        "error" => StreamEvent::Failed(
            event["message"]
                .as_str()
                .or_else(|| event["error"]["message"].as_str())
                .unwrap_or("unknown error")
                .to_string(),
        ),
        _ => StreamEvent::Other,
    }
}

/// Fill the calls, items and usage from the final response object.
fn finish(out: &mut RoundOutput, response: &Value) {
    let items = response["output"].as_array().cloned().unwrap_or_default();
    for item in &items {
        if item["type"] == "function_call" {
            let arguments = item["arguments"].as_str().unwrap_or_default();
            out.calls.push(FunctionCall {
                call_id: item["call_id"].as_str().unwrap_or_default().to_string(),
                name: item["name"].as_str().unwrap_or_default().to_string(),
                arguments: if arguments.is_empty() {
                    "{}".to_string()
                } else {
                    arguments.to_string()
                },
            });
        }
    }
    out.calls
        .retain(|c| !c.call_id.is_empty() && !c.name.is_empty());
    out.items = items;
    out.input_tokens = response["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
    out.output_tokens = response["usage"]["output_tokens"].as_i64().unwrap_or(0) as i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_openai_uses_responses() {
        assert!(uses_responses_api("openai"));
        assert!(!uses_responses_api("cerebras"));
        assert!(!uses_responses_api("mistral"));
    }

    #[test]
    fn the_endpoint_sits_beside_chat_completions() {
        assert_eq!(
            responses_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            responses_url("http://127.0.0.1:9/v1"),
            "http://127.0.0.1:9/v1/responses"
        );
    }

    #[test]
    fn history_becomes_input_items_and_replays_reasoning() {
        let reasoning =
            json!({"type": "reasoning", "id": "rs_1", "encrypted_content": "x", "summary": []});
        let call = json!({"type": "function_call", "id": "fc_1", "call_id": "c1", "name": "search_plans", "arguments": "{}"});
        let history = vec![
            json!({"role": "user", "content": "Jämför"}),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "search_plans", "arguments": "{}"}}],
                OUTPUT_ITEMS_KEY: [reasoning.clone(), call.clone()],
            }),
            json!({"role": "tool", "tool_call_id": "c1", "content": "{\"plans\":[]}"}),
            json!({
                "role": "assistant",
                "content": "Earlier answer",
                "tool_calls": [{"id": "c0", "type": "function", "function": {"name": "get_plan", "arguments": "{\"code\":\"IB133N\"}"}}],
            }),
        ];
        let input = to_input(&history);
        assert_eq!(input[0], json!({"role": "user", "content": "Jämför"}));
        assert_eq!(input[1], reasoning);
        assert_eq!(input[2], call);
        assert_eq!(
            input[3],
            json!({"type": "function_call_output", "call_id": "c1", "output": "{\"plans\":[]}"})
        );
        assert_eq!(
            input[4],
            json!({"role": "assistant", "content": "Earlier answer"})
        );
        assert_eq!(input[5]["type"], "function_call");
        assert_eq!(input[5]["call_id"], "c0");

        let stripped = strip_output_items(&history);
        assert!(stripped[1].get(OUTPUT_ITEMS_KEY).is_none());
        assert!(stripped[1].get("tool_calls").is_some());
    }

    #[test]
    fn the_body_asks_for_reasoning_and_flat_tools() {
        let catalog = vec![json!({
            "type": "function",
            "function": {"name": "get_plan", "description": "d", "parameters": {"type": "object"}}
        })];
        let body = build_body("gpt-6-luna@flex", "sys", &[], &catalog, 4096);
        assert_eq!(body["reasoning"]["effort"], REASONING_EFFORT);
        assert_eq!(body["include"][0], "reasoning.encrypted_content");
        assert_eq!(body["store"], false);
        assert_eq!(body["tools"][0]["name"], "get_plan");
        assert_eq!(body["tools"][0]["strict"], false);
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn deltas_and_failures_are_read() {
        let text = parse_event(&json!({"type": "response.output_text.delta", "delta": "Hej"}));
        assert!(matches!(text, StreamEvent::Text(t) if t == "Hej"));
        let failed = parse_event(
            &json!({"type": "response.failed", "response": {"error": {"message": "boom"}}}),
        );
        assert!(matches!(failed, StreamEvent::Failed(m) if m == "boom"));
        let error = parse_event(&json!({"type": "error", "message": "rate limited"}));
        assert!(matches!(error, StreamEvent::Failed(m) if m == "rate limited"));
        assert!(matches!(
            parse_event(&json!({"type": "response.reasoning_summary_text.delta"})),
            StreamEvent::Other
        ));
    }

    #[test]
    fn the_final_response_gives_calls_items_and_usage() {
        let event = json!({
            "type": "response.completed",
            "response": {
                "output": [
                    {"type": "reasoning", "id": "rs_1", "encrypted_content": "x"},
                    {"type": "function_call", "call_id": "c1", "name": "get_plan", "arguments": "{\"code\":\"IB133N\"}"},
                    {"type": "function_call", "call_id": "c2", "name": "search_plans", "arguments": ""}
                ],
                "usage": {"input_tokens": 1200, "output_tokens": 340}
            }
        });
        let StreamEvent::Done(out) = parse_event(&event) else {
            panic!("a completed response is the end of the round");
        };
        assert_eq!(out.calls.len(), 2);
        assert_eq!(out.calls[1].arguments, "{}");
        assert_eq!(out.items.len(), 3);
        assert_eq!((out.input_tokens, out.output_tokens), (1200, 340));
    }
}
