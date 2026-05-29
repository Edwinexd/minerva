/// Display metadata for the cross-encoder re-ranker catalog. Keys are
/// the canonical model ids the backend uses (mirrors
/// `VALID_RERANKER_MODELS` in `crates/minerva-ingest/src/reranker.rs`).
/// `descKey` points at an i18n string under `config.rerankerModels.*`.
/// `multilingual` drives a small badge so a teacher on Swedish content
/// can tell at a glance which models actually cover it. An unknown id
/// falls back to the raw model id with no description.
export const RERANKER_MODEL_DISPLAY: Record<
  string,
  { name: string; descKey: string; multilingual: boolean }
> = {
  "jinaai/jina-reranker-v2-base-multilingual": {
    name: "Jina Reranker v2 Base (multilingual)",
    descKey: "config.rerankerModels.jinaV2MultilingualDesc",
    multilingual: true,
  },
  "rozgo/bge-reranker-v2-m3": {
    name: "BGE Reranker v2 m3 (multilingual)",
    descKey: "config.rerankerModels.bgeV2M3Desc",
    multilingual: true,
  },
  "BAAI/bge-reranker-base": {
    name: "BGE Reranker Base (EN/ZH)",
    descKey: "config.rerankerModels.bgeBaseDesc",
    multilingual: false,
  },
  "jinaai/jina-reranker-v1-turbo-en": {
    name: "Jina Reranker v1 Turbo (EN)",
    descKey: "config.rerankerModels.jinaV1TurboDesc",
    multilingual: false,
  },
}

/// Friendly display name for a re-ranker id, falling back to the raw id.
export function rerankerDisplayName(modelId: string): string {
  return RERANKER_MODEL_DISPLAY[modelId]?.name ?? modelId
}
