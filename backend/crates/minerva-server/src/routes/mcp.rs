//! Remote MCP server at `/mcp` (Streamable HTTP, MCP 2026-07-28).
//!
//! Stateless: every request carries its protocol version and client
//! capabilities in `_meta`, there are no sessions, and GET/DELETE answer
//! 405. Clients still on 2025-11-25 can run the legacy `initialize`
//! handshake on the same endpoint; their requests are then served with
//! that revision's semantics (the spec allows a dual-era endpoint).
//!
//! Every request needs a bearer token from `routes::oauth` whose audience
//! is this endpoint. Tools are read-only and only see what the token's
//! user can see as staff in the web UI: courses they teach (owner, teacher
//! or TA) and the staff tools they have access to. Students never get here
//! (`oauth::may_connect`), so the student guardrails in chat need no
//! counterpart.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use minerva_core::models::{User, WorkspaceKind};
use serde_json::{json, Value};
use uuid::Uuid;

use super::oauth;
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "2026-07-28";
const LEGACY_VERSION: &str = "2025-11-25";
const SUPPORTED: [&str; 2] = [PROTOCOL_VERSION, LEGACY_VERSION];

/// Document text returned by `get_document` / `get_plan` is capped so one
/// call cannot flood the client's context window.
const MAX_DOCUMENT_CHARS: usize = 100_000;
const MAX_SEARCH_RESULTS: u64 = 20;

/// Tool calls per grant per minute. Generous for an assistant working a
/// question, tight enough that a runaway loop cannot hammer the embedder.
const CALLS_PER_MINUTE: u32 = 60;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/mcp",
        post(handle)
            .get(method_not_allowed)
            .delete(method_not_allowed),
    )
}

async fn method_not_allowed() -> Response {
    let mut resp = StatusCode::METHOD_NOT_ALLOWED.into_response();
    resp.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    resp
}

fn server_info() -> Value {
    json!({ "name": "Minerva", "version": env!("CARGO_PKG_VERSION") })
}

fn rpc_error(
    status: StatusCode,
    id: &Value,
    code: i64,
    message: &str,
    data: Option<Value>,
) -> Response {
    let mut err = json!({ "code": code, "message": message });
    if let Some(d) = data {
        err["data"] = d;
    }
    let mut body = json!({ "jsonrpc": "2.0", "error": err });
    if !id.is_null() {
        body["id"] = id.clone();
    }
    (status, Json(body)).into_response()
}

fn rpc_result(id: &Value, mut result: Value) -> Response {
    result["resultType"] = json!("complete");
    result["_meta"] = json!({ "io.modelcontextprotocol/serverInfo": server_info() });
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn unauthorized(state: &AppState, invalid_token: bool) -> Response {
    let mut resp = StatusCode::UNAUTHORIZED.into_response();
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        oauth::www_authenticate(&state.config.base_url, invalid_token),
    );
    resp
}

/// The user behind a valid access token for this endpoint, if they may
/// still use it (a demoted or suspended account loses access at once).
async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Uuid, User), Box<Response>> {
    let Some(token) = oauth::bearer(headers) else {
        return Err(Box::new(unauthorized(state, false)));
    };
    let access = minerva_db::queries::oauth::resolve_access(&state.db, &oauth::hash_token(token))
        .await
        .map_err(|e| {
            tracing::error!("mcp: token lookup failed: {e}");
            Box::new(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        })?;
    let Some(access) = access else {
        return Err(Box::new(unauthorized(state, true)));
    };
    // Audience check (RFC 8707): only tokens minted for this endpoint.
    if access.resource != oauth::mcp_resource(&state.config.base_url) {
        return Err(Box::new(unauthorized(state, true)));
    }
    let row = minerva_db::queries::users::find_by_id(&state.db, access.user_id)
        .await
        .ok()
        .flatten();
    let Some(row) = row else {
        return Err(Box::new(unauthorized(state, true)));
    };
    let user = crate::auth::user_from_row(row);
    if !oauth::may_connect(&user) {
        return Err(Box::new(unauthorized(state, true)));
    }
    Ok((access.grant_id, user))
}

/// Fixed-window limiter keyed by grant. In memory: one api replica.
fn rate_limited(grant_id: Uuid) -> bool {
    static WINDOWS: OnceLock<Mutex<HashMap<Uuid, (Instant, u32)>>> = OnceLock::new();
    let mut map = WINDOWS
        .get_or_init(Default::default)
        .lock()
        .expect("rate limiter poisoned");
    let now = Instant::now();
    map.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
    let entry = map.entry(grant_id).or_insert((now, 0));
    entry.1 += 1;
    entry.1 > CALLS_PER_MINUTE
}

/// Browsers send `Origin`; a page on another site must not drive the
/// endpoint with a user's token (DNS rebinding and the like).
fn origin_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    match headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        None => true,
        Some(origin) => origin.trim_end_matches('/') == state.config.base_url,
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !origin_allowed(&state, &headers) {
        return rpc_error(
            StatusCode::FORBIDDEN,
            &Value::Null,
            -32600,
            "Origin not allowed",
            None,
        );
    }
    let (grant_id, user) = match authenticate(&state, &headers).await {
        Ok(v) => v,
        Err(resp) => return *resp,
    };
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            &Value::Null,
            -32700,
            "Parse error",
            None,
        );
    };
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return rpc_error(
            StatusCode::BAD_REQUEST,
            &Value::Null,
            -32600,
            "Invalid Request",
            None,
        );
    };
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

    // Notifications (no id): accepted and ignored. The legacy
    // `notifications/initialized` lands here too.
    if id.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }

    let meta_version = params
        .pointer("/_meta/io.modelcontextprotocol~1protocolVersion")
        .and_then(Value::as_str);
    let header_version = header_str(&headers, "mcp-protocol-version");

    match meta_version {
        // Modern, stateless request.
        Some(version) => {
            if !SUPPORTED.contains(&version) || version == LEGACY_VERSION {
                return rpc_error(
                    StatusCode::BAD_REQUEST,
                    &id,
                    -32022,
                    "Unsupported protocol version",
                    Some(json!({ "supported": [PROTOCOL_VERSION], "requested": version })),
                );
            }
            if params
                .pointer("/_meta/io.modelcontextprotocol~1clientCapabilities")
                .is_none()
            {
                return rpc_error(
                    StatusCode::BAD_REQUEST,
                    &id,
                    -32602,
                    "Missing _meta io.modelcontextprotocol/clientCapabilities",
                    None,
                );
            }
            // Headers must mirror the body so intermediaries can route on
            // them without parsing JSON.
            let name = params.get("name").and_then(Value::as_str);
            let mismatch = header_version != Some(version)
                || header_str(&headers, "mcp-method") != Some(method)
                || (method == "tools/call" && header_str(&headers, "mcp-name") != name);
            if mismatch {
                return rpc_error(
                    StatusCode::BAD_REQUEST,
                    &id,
                    -32020,
                    "Header mismatch: MCP-Protocol-Version, Mcp-Method and Mcp-Name must match the body",
                    None,
                );
            }
            dispatch(&state, grant_id, &user, &id, method, &params, false).await
        }
        // Legacy (2025-11-25) client: handshake plus the old methods.
        None => {
            if header_version.is_some_and(|v| v != LEGACY_VERSION && v != PROTOCOL_VERSION)
                && method != "initialize"
            {
                return rpc_error(
                    StatusCode::BAD_REQUEST,
                    &id,
                    -32022,
                    "Unsupported protocol version",
                    Some(json!({ "supported": SUPPORTED, "requested": header_version })),
                );
            }
            match method {
                "initialize" => rpc_result(
                    &id,
                    json!({
                        "protocolVersion": LEGACY_VERSION,
                        "capabilities": { "tools": {} },
                        "serverInfo": server_info(),
                        "instructions": INSTRUCTIONS,
                    }),
                ),
                "ping" => rpc_result(&id, json!({})),
                _ => dispatch(&state, grant_id, &user, &id, method, &params, true).await,
            }
        }
    }
}

const INSTRUCTIONS: &str = "Minerva holds the teaching material of DSV courses at Stockholm \
University and DSV's course plans. Call list_workspaces first to see the courses and staff \
tools you can reach, then search or read documents in one of them by its id. In a credit \
transfer workspace, get_plan returns the full current course plan for a DSV course code.";

async fn dispatch(
    state: &AppState,
    grant_id: Uuid,
    user: &User,
    id: &Value,
    method: &str,
    params: &Value,
    legacy: bool,
) -> Response {
    match method {
        "server/discover" if !legacy => rpc_result(
            id,
            json!({
                "supportedVersions": [PROTOCOL_VERSION],
                "capabilities": { "tools": {} },
                "instructions": INSTRUCTIONS,
                "ttlMs": 3_600_000,
                "cacheScope": "public",
            }),
        ),
        "tools/list" => {
            let mut result = json!({ "tools": tool_definitions() });
            if !legacy {
                result["ttlMs"] = json!(3_600_000);
                result["cacheScope"] = json!("public");
            }
            rpc_result(id, result)
        }
        "tools/call" => {
            if rate_limited(grant_id) {
                return rpc_result(
                    id,
                    tool_error(&format!(
                        "Rate limit: at most {CALLS_PER_MINUTE} tool calls a minute."
                    )),
                );
            }
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return rpc_error(
                    StatusCode::BAD_REQUEST,
                    id,
                    -32602,
                    "params.name is required",
                    None,
                );
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(state, user, name, &args).await {
                Ok(Some(result)) => rpc_result(id, result),
                Ok(None) => rpc_error(StatusCode::BAD_REQUEST, id, -32602, "Unknown tool", None),
                Err(message) => rpc_result(id, tool_error(&message)),
            }
        }
        _ => rpc_error(StatusCode::NOT_FOUND, id, -32601, "Method not found", None),
    }
}

/// A tool execution error: a normal result with `isError`, so the model
/// can read the message and correct its call.
fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// A successful result: structured content plus the same JSON as text for
/// clients that only read `content`.
fn tool_ok(value: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
        "structuredContent": value,
        "isError": false,
    })
}

fn tool_definitions() -> Value {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    json!([
        {
            "name": "list_workspaces",
            "title": "List workspaces",
            "description": "Courses you teach (as owner, teacher or TA) and staff tools you can use, with the id the other tools take.",
            "inputSchema": { "type": "object", "additionalProperties": false },
            "annotations": read_only,
        },
        {
            "name": "search",
            "title": "Search a workspace",
            "description": "Semantic search over one workspace's documents (course material, or DSV course plans in a credit transfer workspace). Returns the best matching passages with their document.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string", "format": "uuid" },
                    "query": { "type": "string", "minLength": 1 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SEARCH_RESULTS, "default": 8 }
                },
                "required": ["workspace_id", "query"],
                "additionalProperties": false
            },
            "annotations": read_only,
        },
        {
            "name": "list_documents",
            "title": "List documents",
            "description": "The documents in one workspace, with id, file name, kind and processing status.",
            "inputSchema": {
                "type": "object",
                "properties": { "workspace_id": { "type": "string", "format": "uuid" } },
                "required": ["workspace_id"],
                "additionalProperties": false
            },
            "annotations": read_only,
        },
        {
            "name": "get_document",
            "title": "Read a document",
            "description": "The full extracted text of one document in a workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string", "format": "uuid" },
                    "document_id": { "type": "string", "format": "uuid" }
                },
                "required": ["workspace_id", "document_id"],
                "additionalProperties": false
            },
            "annotations": read_only,
        },
        {
            "name": "get_plan",
            "title": "Read a DSV course plan",
            "description": "The full current course plan (kursplan) for a DSV course code such as IB440C, in Swedish and, when published, English. Available to users of a credit transfer workspace.",
            "inputSchema": {
                "type": "object",
                "properties": { "code": { "type": "string", "pattern": "^[A-Za-z0-9]{2,16}$" } },
                "required": ["code"],
                "additionalProperties": false
            },
            "annotations": read_only,
        }
    ])
}

/// Workspaces a staff user can reach over MCP: the teacher list (owner,
/// teacher or TA of a course, plus staff tools they can use). Admins get
/// the same view as anyone else here, not the whole instance.
async fn reachable(
    state: &AppState,
    user: &User,
) -> Result<Vec<minerva_db::queries::workspaces::WorkspaceRow>, String> {
    minerva_db::queries::workspaces::list_for_teacher(&state.db, user.id)
        .await
        .map_err(|e| {
            tracing::error!("mcp: listing workspaces failed: {e}");
            "Internal error listing workspaces.".to_string()
        })
}

async fn workspace_arg(
    state: &AppState,
    user: &User,
    args: &Value,
) -> Result<minerva_db::queries::workspaces::WorkspaceRow, String> {
    let id = args
        .get("workspace_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or("workspace_id must be a workspace id from list_workspaces.")?;
    reachable(state, user)
        .await?
        .into_iter()
        .find(|w| w.id == id)
        .ok_or_else(|| "No such workspace, or you do not have access to it.".to_string())
}

/// `Ok(None)` for an unknown tool name (a protocol error), `Err` for a
/// tool-level failure the model should see.
async fn call_tool(
    state: &AppState,
    user: &User,
    name: &str,
    args: &Value,
) -> Result<Option<Value>, String> {
    let value = match name {
        "list_workspaces" => {
            let rows = reachable(state, user).await?;
            let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
            let courses = minerva_db::queries::courses::find_many(&state.db, &ids)
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<Value> = rows
                .iter()
                .map(|w| {
                    let course = courses.get(&w.id);
                    json!({
                        "id": w.id,
                        "kind": w.kind,
                        "name": w.name,
                        "description": w.description,
                        "term": course.and_then(|c| c.semester_label.clone()),
                        "course_code": course.and_then(|c| c.course_code.clone()),
                    })
                })
                .collect();
            json!({ "workspaces": items })
        }
        "search" => {
            let ws = workspace_arg(state, user, args).await?;
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .ok_or("query must be a non-empty string.")?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(8)
                .clamp(1, MAX_SEARCH_RESULTS);
            json!({ "results": search(state, &ws, query, limit).await? })
        }
        "list_documents" => {
            let ws = workspace_arg(state, user, args).await?;
            let docs = minerva_db::queries::documents::list_by_course(&state.db, ws.id)
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<Value> = docs
                .into_iter()
                .filter(|d| d.orphaned_at.is_none())
                .map(|d| {
                    json!({
                        "id": d.id,
                        "filename": d.filename,
                        "kind": d.kind,
                        "status": d.status,
                        "source_url": d.source_url,
                    })
                })
                .collect();
            json!({ "documents": items })
        }
        "get_document" => {
            let ws = workspace_arg(state, user, args).await?;
            let doc_id = args
                .get("document_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or("document_id must be a document id from list_documents or search.")?;
            read_document(state, &ws, doc_id).await?
        }
        "get_plan" => {
            let code = args
                .get("code")
                .and_then(Value::as_str)
                .map(|c| c.trim().to_ascii_uppercase())
                .filter(|c| {
                    (2..=16).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric())
                })
                .ok_or("code must be a course code such as IB440C.")?;
            let can_use = reachable(state, user)
                .await?
                .iter()
                .any(|w| WorkspaceKind::parse(&w.kind) == Some(WorkspaceKind::CreditTransfer));
            if !can_use {
                return Err("get_plan needs access to a credit transfer workspace.".into());
            }
            read_plan(state, &code).await?
        }
        _ => return Ok(None),
    };
    Ok(Some(tool_ok(value)))
}

async fn search(
    state: &AppState,
    ws: &minerva_db::queries::workspaces::WorkspaceRow,
    query: &str,
    limit: u64,
) -> Result<Vec<Value>, String> {
    use crate::strategy::common::{payload_int, payload_string};
    let collection = minerva_pipeline::pipeline::collection_name(ws.id, ws.embedding_version);
    if !state
        .qdrant
        .collection_exists(&collection)
        .await
        .unwrap_or(false)
    {
        return Ok(Vec::new());
    }
    let orphaned = minerva_db::queries::documents::orphaned_doc_ids(&state.db, ws.id)
        .await
        .unwrap_or_default();
    let points = crate::strategy::common::embedding_search(
        &reqwest::Client::new(),
        &state.config.openai_api_key,
        &state.fastembed,
        &state.qdrant,
        &collection,
        query,
        limit,
        None,
        &ws.embedding_provider,
        &ws.embedding_model,
        &orphaned,
    )
    .await
    .map_err(|e| {
        tracing::warn!("mcp search in {}: {e}", ws.id);
        "Search is unavailable right now.".to_string()
    })?;
    Ok(points
        .iter()
        .filter_map(|p| {
            Some(json!({
                "score": p.score,
                "document_id": payload_string(&p.payload, "document_id")?,
                "filename": payload_string(&p.payload, "filename")?,
                "chunk_index": payload_int(&p.payload, "chunk_index"),
                "text": payload_string(&p.payload, "text")?,
            }))
        })
        .collect())
}

fn cap_text(text: &str) -> (String, bool) {
    match text.char_indices().nth(MAX_DOCUMENT_CHARS) {
        Some((end, _)) => (text[..end].to_string(), true),
        None => (text.to_string(), false),
    }
}

async fn extract(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        minerva_pipeline::pipeline::extract_document_text(std::path::Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn read_document(
    state: &AppState,
    ws: &minerva_db::queries::workspaces::WorkspaceRow,
    doc_id: Uuid,
) -> Result<Value, String> {
    use minerva_db::queries::documents;
    let doc = documents::find_by_id(&state.db, doc_id)
        .await
        .map_err(|e| e.to_string())?
        .filter(|d| d.workspace_id == ws.id && d.orphaned_at.is_none())
        .ok_or("No such document in this workspace.")?;
    // A URL stub's content lives in its child (the materialized PDF or
    // transcript); read that instead of the one-line stub.
    let doc = if doc.mime_type == "text/x-url" {
        documents::list_children(&state.db, doc.id)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|c| c.orphaned_at.is_none())
            .ok_or("This link has not been fetched yet.")?
    } else {
        doc
    };
    let path = format!(
        "{}/{}/{}.{}",
        state.config.docs_path,
        ws.id,
        doc.id,
        minerva_pipeline::pipeline::extension_from_filename(&doc.filename)
    );
    let text = extract(path)
        .await
        .map_err(|_| "This document has no readable text.".to_string())?;
    let (text, truncated) = cap_text(&text);
    Ok(json!({
        "id": doc.id,
        "filename": doc.filename,
        "kind": doc.kind,
        "text": text,
        "truncated": truncated,
    }))
}

async fn read_plan(state: &AppState, code: &str) -> Result<Value, String> {
    let current = minerva_db::queries::syllabus_archive::current_versions(&state.db)
        .await
        .map_err(|e| e.to_string())?;
    let versions: Vec<_> = current.into_iter().filter(|v| v.code == code).collect();
    if versions.is_empty() {
        return Err(format!("No current DSV course plan for {code}."));
    }
    let mut texts = Vec::new();
    for v in &versions {
        let path = minerva_app_core::syllabus_archive::syllabus_pdf_path(
            &state.config.docs_path,
            &v.content_hash,
        );
        let text = extract(path)
            .await
            .map_err(|_| format!("The {} plan for {code} could not be read.", v.language))?;
        let (text, truncated) = cap_text(&text);
        texts.push(json!({
            "language": v.language,
            "valid_from": v.valid_from,
            "text": text,
            "truncated": truncated,
        }));
    }
    let first = &versions[0];
    Ok(json!({
        "code": code,
        "name_sv": first.name_sv,
        "name_en": first.name_en,
        "versions": texts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_follow_the_spec_charset_and_are_unique() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        for n in names {
            assert!(
                n.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)),
                "{n}"
            );
        }
        for t in tools.as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn results_carry_result_type_and_server_info() {
        let resp = rpc_result(&json!(1), json!({ "tools": [] }));
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let body = rt
            .block_on(axum::body::to_bytes(resp.into_body(), usize::MAX))
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["result"]["resultType"], "complete");
        assert_eq!(
            v["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "Minerva"
        );
        assert_eq!(v["id"], 1);
    }

    #[test]
    fn document_text_is_capped() {
        let long = "x".repeat(MAX_DOCUMENT_CHARS + 5);
        let (t, truncated) = cap_text(&long);
        assert!(truncated);
        assert_eq!(t.chars().count(), MAX_DOCUMENT_CHARS);
        assert!(!cap_text("short").1);
    }
}
