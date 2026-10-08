//! Figure retrieval for a chat turn (see "Visual extraction pipeline" in
//! docs/ARCHITECTURE.md). Two searches run for every seed retrieval and
//! are fused:
//!
//! - the figures' **context** (caption, title, nearby text, speech) with the
//!   course's own text model, which carries Swedish and course vocabulary;
//! - their **pixels** through CLIP, querying with CLIP's text encoder, which
//!   finds a figure by what it shows even when no words on the slide say it.
//!
//! The top few become prompt context and image citations on the reply.

use std::collections::{HashMap, HashSet};

use minerva_db::queries::visual_extraction::{self as queue, ChatFigureRow};
use minerva_pipeline::figures as figure_index;
use qdrant_client::qdrant::{Condition, Filter, SearchPointsBuilder};
use uuid::Uuid;

use super::GenerationContext;

/// Figures offered per reply; more crowd the answer without helping.
const MAX_FIGURES: usize = 3;
/// Candidates taken from each search before fusion.
const CANDIDATES: u64 = 10;
/// CLIP cosine below which a text-to-image match is noise. ViT-B/32 scores
/// matching captions around 0.28-0.33 and unrelated ones around 0.15-0.22.
const VISUAL_MIN_SCORE: f32 = 0.25;
/// Reciprocal rank fusion constant (the usual 60).
const RRF_K: f32 = 60.0;

/// One figure picked for a reply.
#[derive(Debug, Clone)]
pub struct FigureHit {
    pub row: ChatFigureRow,
}

/// Where a figure comes from, the way a student would say it: a slide of a
/// lecture (pages with a time on screen) or a page of a PDF.
pub fn origin(filename: &str, page_number: Option<i32>, start_seconds: Option<f32>) -> String {
    // A lecture's figures hang off its `.url` stub; the extension is noise.
    let filename = filename.strip_suffix(".url").unwrap_or(filename);
    match (page_number, start_seconds) {
        (Some(n), Some(_)) => format!("{filename}, slide {n}"),
        (Some(n), None) => format!("{filename}, page {n}"),
        _ => filename.to_string(),
    }
}

impl FigureHit {
    pub fn origin(&self) -> String {
        origin(
            &self.row.filename,
            self.row.page_number,
            self.row.start_seconds,
        )
    }
}

fn figure_ids(points: Vec<qdrant_client::qdrant::ScoredPoint>) -> Vec<Uuid> {
    points
        .into_iter()
        .filter_map(|p| {
            let value = p.payload.get("figure_id")?.as_str()?.to_string();
            Uuid::parse_str(&value).ok()
        })
        .collect()
}

/// Reciprocal rank fusion over ranked id lists, best first.
fn fuse(lists: &[Vec<Uuid>]) -> Vec<Uuid> {
    let mut scores: HashMap<Uuid, f32> = HashMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            *scores.entry(*id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
    }
    let mut ranked: Vec<(Uuid, f32)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.into_iter().map(|(id, _)| id).collect()
}

async fn context_search(
    ctx: &GenerationContext,
    client: &reqwest::Client,
    query: &str,
    orphaned: &HashSet<String>,
) -> Vec<Uuid> {
    let collection = figure_index::context_collection_name(ctx.course_id, ctx.embedding_version);
    let threshold = (ctx.min_score > 0.0).then_some(ctx.min_score);
    match super::common::embedding_search(
        client,
        &ctx.openai_api_key,
        &ctx.fastembed,
        &ctx.qdrant,
        &collection,
        query,
        CANDIDATES,
        threshold,
        &ctx.embedding_provider,
        &ctx.embedding_model,
        orphaned,
    )
    .await
    {
        Ok(points) => figure_ids(points),
        Err(e) => {
            // No figures indexed for this course yet is the common case.
            tracing::debug!("figures: context search skipped: {e}");
            Vec::new()
        }
    }
}

async fn visual_search(
    ctx: &GenerationContext,
    query: &str,
    orphaned: &HashSet<String>,
) -> Vec<Uuid> {
    let vector = match ctx
        .fastembed
        .embed_query(figure_index::VISUAL_QUERY_MODEL, vec![query.to_string()])
        .await
    {
        Ok(mut vectors) if !vectors.is_empty() => vectors.swap_remove(0),
        Ok(_) => return Vec::new(),
        Err(e) => {
            tracing::warn!("figures: CLIP query embedding failed: {e}");
            return Vec::new();
        }
    };
    let excluded: Vec<String> = orphaned.iter().cloned().collect();
    let mut filter = Filter::must([Condition::matches("course_id", ctx.course_id.to_string())]);
    if !excluded.is_empty() {
        filter.must_not = vec![Condition::matches("document_id", excluded)];
    }
    let search = SearchPointsBuilder::new(figure_index::COLLECTION, vector, CANDIDATES)
        .with_payload(true)
        .score_threshold(VISUAL_MIN_SCORE)
        .filter(filter);
    match ctx.qdrant.search_points(search).await {
        Ok(response) => figure_ids(response.result),
        Err(e) => {
            tracing::debug!("figures: visual search skipped: {e}");
            Vec::new()
        }
    }
}

/// The figures to offer with this turn's reply, best first. Never fails
/// the turn: any error just means no figures.
pub async fn figure_lookup(
    ctx: &GenerationContext,
    client: &reqwest::Client,
    query: &str,
    orphaned: &HashSet<String>,
) -> Vec<FigureHit> {
    // A course nothing has been extracted for yet costs one indexed lookup,
    // not two searches plus the CLIP encoder loaded in the embedder.
    match queue::course_has_indexed_figures(&ctx.db, ctx.course_id).await {
        Ok(true) => {}
        Ok(false) => return Vec::new(),
        Err(e) => {
            tracing::warn!("figures: checking for indexed figures failed: {e}");
            return Vec::new();
        }
    }
    let (by_context, by_pixels) = tokio::join!(
        context_search(ctx, client, query, orphaned),
        visual_search(ctx, query, orphaned)
    );
    tracing::debug!(
        course_id = %ctx.course_id,
        context_hits = by_context.len(),
        visual_hits = by_pixels.len(),
        "figures: searched"
    );
    let ranked = fuse(&[by_context, by_pixels]);
    if ranked.is_empty() {
        return Vec::new();
    }
    // Over-fetch rows: some may be filtered out by the kind and visibility
    // rules, which only the database can apply.
    let candidates: Vec<Uuid> = ranked.into_iter().take(MAX_FIGURES * 3).collect();
    match queue::find_chat_figures(&ctx.db, ctx.course_id, &candidates, ctx.kg_enabled).await {
        Ok(rows) => {
            let hits: Vec<FigureHit> = rows
                .into_iter()
                .take(MAX_FIGURES)
                .map(|row| FigureHit { row })
                .collect();
            tracing::debug!(
                picked = ?hits.iter().map(|h| h.origin()).collect::<Vec<_>>(),
                "figures: offered"
            );
            hits
        }
        Err(e) => {
            tracing::warn!("figures: loading figure rows failed: {e}");
            Vec::new()
        }
    }
}

/// The prompt section describing the figures available to the reply. The
/// model places one by writing its marker, `[Figure N]`, on a line of its
/// own; the client renders the image there. Figures it does not place are
/// still shown under the reply.
pub fn prompt_section(figures: &[FigureHit]) -> Option<String> {
    if figures.is_empty() {
        return None;
    }
    let mut section = String::from(
        "\n\n## Figures you can show\n\
         These images from the course material can be displayed inside your \
         answer. To show one, write its marker, for example [Figure 1], on a \
         line of its own where the image helps the explanation; refer to it in \
         the surrounding text by what it shows. Use only the markers listed \
         here, each at most once, and only when the figure is relevant. You \
         cannot see the images: rely on the description below and do not claim \
         details beyond it.\n",
    );
    for (index, figure) in figures.iter().enumerate() {
        section.push_str(&format!("\n[Figure {}] {}", index + 1, figure.origin()));
        if let Some(caption) = figure.row.caption.as_deref().filter(|c| !c.is_empty()) {
            section.push_str(&format!(": {caption}"));
        }
        section.push('\n');
        section.push_str(figure.row.context.trim());
        section.push('\n');
    }
    Some(section)
}

/// Every reply's figures in a conversation, as the client receives them:
/// labels plus an image URL signed for whoever is reading. Keyed by message.
pub async fn for_conversation(
    db: &sqlx::PgPool,
    secret: &str,
    conversation_id: Uuid,
) -> Result<HashMap<Uuid, serde_json::Value>, sqlx::Error> {
    let mut by_message: HashMap<Uuid, Vec<serde_json::Value>> = HashMap::new();
    for row in queue::list_conversation_figures(db, conversation_id).await? {
        by_message
            .entry(row.message_id)
            .or_default()
            .push(serde_json::json!({
                "id": row.figure_id,
                "origin": origin(&row.filename, row.page_number, row.start_seconds),
                "caption": row.caption,
                "image_url": minerva_app_core::visual_extraction::figure_url(secret, row.figure_id),
            }));
    }
    Ok(by_message
        .into_iter()
        .map(|(id, list)| (id, serde_json::Value::Array(list)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fusion_rewards_agreement_between_searches() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        // `b` is second in both lists; `a` and `c` top one list each.
        let fused = fuse(&[vec![a, b], vec![c, b]]);
        assert_eq!(fused[0], b);
        assert_eq!(fused.len(), 3);
    }

    #[test]
    fn origin_names_slides_and_pages() {
        assert_eq!(
            origin("Intro.url", Some(20), Some(1500.0)),
            "Intro, slide 20"
        );
        assert_eq!(origin("guide.pdf", Some(3), None), "guide.pdf, page 3");
        assert_eq!(origin("guide.pdf", None, None), "guide.pdf");
    }

    #[test]
    fn prompt_lists_markers_in_offer_order() {
        let hit = |n: u128, page: i32, caption: Option<&str>| FigureHit {
            row: ChatFigureRow {
                id: Uuid::from_u128(n),
                document_id: Uuid::from_u128(100),
                filename: "Lecture 3.url".to_string(),
                caption: caption.map(str::to_string),
                context: format!("context {n}"),
                page_number: Some(page),
                start_seconds: Some(1.0),
            },
        };
        let section =
            prompt_section(&[hit(1, 7, Some("The 2017 architecture")), hit(2, 9, None)]).unwrap();
        assert!(section.contains("[Figure 1] Lecture 3, slide 7: The 2017 architecture\ncontext 1"));
        assert!(section.contains("[Figure 2] Lecture 3, slide 9\ncontext 2"));
        assert!(prompt_section(&[]).is_none());
    }

    #[test]
    fn fusion_of_nothing_is_nothing() {
        assert!(fuse(&[vec![], vec![]]).is_empty());
    }
}
