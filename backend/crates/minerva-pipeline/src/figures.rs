//! Figures and slide-aligned lecture text from the visual extraction
//! pipeline (see "Visual extraction pipeline" in docs/ARCHITECTURE.md).
//!
//! Each figure is searchable two ways, and every query runs both:
//!
//! - its **context** (caption, title, nearby text, and for lectures what was
//!   said) embedded with the course's text model, in a per-course collection
//!   because the dimension follows the course's model;
//! - its **pixels** embedded with CLIP on Olympus, in one global collection
//!   filtered by course, searched with the paired CLIP text encoder.

use std::collections::HashMap;

use minerva_core::rpc::EmbedderClient;
use qdrant_client::qdrant::{CreateFieldIndexCollectionBuilder, FieldType, PointStruct};
use qdrant_client::Qdrant;
use serde::Deserialize;
use uuid::Uuid;

use crate::pipeline::{
    ensure_collection, ensure_document_id_index, local_model_dimensions, upsert_batched,
    OPENAI_EMBEDDING_DIMENSIONS,
};

/// Global collection of CLIP image vectors, one point per figure.
pub const COLLECTION: &str = "figures_clip_vit_b32";
/// Image encoder the Olympus worker runs (FastEmbed model code).
pub const VISUAL_MODEL: &str = "Qdrant/clip-ViT-B-32-vision";
/// Text encoder that embeds queries into the same space, in
/// `minerva-embedder`.
pub const VISUAL_QUERY_MODEL: &str = "Qdrant/clip-ViT-B-32-text";
pub const VISUAL_DIMENSIONS: u64 = 512;

/// Per-course collection of figure context vectors, alongside (and
/// versioned with) the course's chunk collection.
pub fn context_collection_name(course_id: Uuid, embedding_version: i32) -> String {
    format!(
        "{}_figures",
        crate::pipeline::collection_name(course_id, embedding_version)
    )
}

/// Where a document's slide frames and figure crops live on disk.
pub fn assets_dir(docs_path: &str, course_id: Uuid, document_id: Uuid) -> String {
    format!("{docs_path}/{course_id}/visual/{document_id}")
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cue {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

/// A slide's place on the lecture timeline.
pub trait Timed {
    fn start(&self) -> f32;
}

impl<T: Timed> Timed for &T {
    fn start(&self) -> f32 {
        (**self).start()
    }
}

/// Assign each cue to the slide on screen at the cue's midpoint. Slides
/// must be sorted by start; a cue before the first slide goes to the first.
/// Returns one cue list per slide.
pub fn align_cues<'c, S: Timed>(slides: &[S], cues: &'c [Cue]) -> Vec<Vec<&'c Cue>> {
    let mut spoken = vec![Vec::new(); slides.len()];
    if slides.is_empty() {
        return spoken;
    }
    for cue in cues {
        let middle = (cue.start + cue.end) / 2.0;
        let index = slides
            .partition_point(|s| s.start() <= middle)
            .saturating_sub(1);
        spoken[index].push(cue);
    }
    spoken
}

pub fn spoken_text(cues: &[&Cue]) -> String {
    cues.iter()
        .map(|c| c.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn timestamp(seconds: f32) -> String {
    let s = seconds.max(0.0) as u32;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// One slide's section of the lecture document: its heading with the time
/// it was on screen, what it says, and what was said while it was up.
pub fn lecture_section(
    label: &str,
    start: f32,
    end: f32,
    slide_text: &str,
    spoken: &str,
) -> String {
    let mut section = format!(
        "## {label} ({} to {})\n\n",
        timestamp(start),
        timestamp(end)
    );
    if !slide_text.trim().is_empty() {
        section.push_str(slide_text.trim());
        section.push_str("\n\n");
    }
    if !spoken.is_empty() {
        section.push_str("Spoken: ");
        section.push_str(spoken);
        section.push_str("\n\n");
    }
    section
}

/// A figure ready to index: its stored CLIP vector and its context text.
pub struct FigureToIndex {
    pub id: Uuid,
    pub document_id: Uuid,
    pub context: String,
    pub visual_vector: Vec<f32>,
}

/// How a course embeds text, as the ingest pipeline does it.
pub struct CourseEmbedding<'a> {
    pub course_id: Uuid,
    pub provider: &'a str,
    pub model: &'a str,
    pub version: i32,
}

/// Put one course's figures into both collections: the context through the
/// course's own text model, the stored CLIP vector as is. Point ids are the
/// figure ids, so a retry overwrites instead of duplicating.
pub async fn index_figures(
    qdrant: &Qdrant,
    embedder: &dyn EmbedderClient,
    http_client: &reqwest::Client,
    openai_api_key: &str,
    course: &CourseEmbedding<'_>,
    figures: &[FigureToIndex],
) -> Result<(), String> {
    if figures.is_empty() {
        return Ok(());
    }
    let contexts: Vec<String> = figures.iter().map(|f| f.context.clone()).collect();
    let (dimensions, context_vectors) = if course.provider == "local" {
        let dimensions = local_model_dimensions(course.model)
            .ok_or_else(|| format!("unsupported local embedding model: {}", course.model))?;
        (dimensions, embedder.embed(course.model, contexts).await?)
    } else {
        let result = crate::embedder::embed_texts(http_client, openai_api_key, &contexts).await?;
        (OPENAI_EMBEDDING_DIMENSIONS, result.embeddings)
    };

    let payload = |figure: &FigureToIndex| -> HashMap<String, qdrant_client::qdrant::Value> {
        HashMap::from([
            ("course_id".to_string(), course.course_id.to_string().into()),
            (
                "document_id".to_string(),
                figure.document_id.to_string().into(),
            ),
            ("figure_id".to_string(), figure.id.to_string().into()),
        ])
    };

    let context_collection = context_collection_name(course.course_id, course.version);
    ensure_collection(qdrant, &context_collection, dimensions).await?;
    let context_points = figures
        .iter()
        .zip(context_vectors)
        .map(|(figure, vector)| PointStruct::new(figure.id.to_string(), vector, payload(figure)))
        .collect();
    upsert_batched(qdrant, &context_collection, context_points).await?;

    ensure_collection(qdrant, COLLECTION, VISUAL_DIMENSIONS).await?;
    ensure_document_id_index(qdrant, COLLECTION).await;
    // Every visual search filters by course.
    if let Err(e) = qdrant
        .create_field_index(CreateFieldIndexCollectionBuilder::new(
            COLLECTION,
            "course_id",
            FieldType::Keyword,
        ))
        .await
    {
        tracing::debug!("qdrant: course_id index on {COLLECTION} not created (likely exists): {e}");
    }
    let visual_points = figures
        .iter()
        .map(|figure| {
            PointStruct::new(
                figure.id.to_string(),
                figure.visual_vector.clone(),
                payload(figure),
            )
        })
        .collect();
    upsert_batched(qdrant, COLLECTION, visual_points).await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Slide(f32);
    impl Timed for Slide {
        fn start(&self) -> f32 {
            self.0
        }
    }

    fn cue(start: f32, end: f32, text: &str) -> Cue {
        Cue {
            start,
            end,
            text: text.to_string(),
        }
    }

    #[test]
    fn cues_go_to_the_slide_on_screen_at_their_midpoint() {
        let slides = [Slide(0.0), Slide(60.0), Slide(120.0)];
        let cues = [
            cue(1.0, 5.0, "intro"),
            cue(55.0, 70.0, "straddles, midpoint 62.5"),
            cue(58.0, 61.0, "midpoint 59.5"),
            cue(130.0, 140.0, "last"),
        ];
        let aligned = align_cues(&slides, &cues);
        let texts: Vec<Vec<&str>> = aligned
            .iter()
            .map(|cs| cs.iter().map(|c| c.text.as_str()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![
                vec!["intro", "midpoint 59.5"],
                vec!["straddles, midpoint 62.5"],
                vec!["last"]
            ]
        );
    }

    #[test]
    fn cue_before_first_slide_goes_to_first() {
        let slides = [Slide(10.0), Slide(20.0)];
        let cues = [cue(0.0, 2.0, "early")];
        assert_eq!(align_cues(&slides, &cues)[0].len(), 1);
    }

    #[test]
    fn no_slides_means_no_alignment() {
        let slides: [Slide; 0] = [];
        assert!(align_cues(&slides, &[cue(0.0, 1.0, "x")]).is_empty());
    }

    #[test]
    fn section_omits_empty_parts() {
        assert_eq!(
            lecture_section("Slide 3", 61.0, 3725.0, "", ""),
            "## Slide 3 (1:01 to 1:02:05)\n\n"
        );
        assert_eq!(
            lecture_section("Slide 1", 0.0, 30.0, " Title\n", "hello"),
            "## Slide 1 (0:00 to 0:30)\n\nTitle\n\nSpoken: hello\n\n"
        );
    }
}
