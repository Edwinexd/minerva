/// Display metadata for every catalog model the UI might surface.
/// Keys are the canonical HuggingFace-style ids the backend uses; the
/// `descKey` points at an i18n string under `config.embeddingModels.*`
/// (only used by the teacher config dropdown). `dims` is a fallback
/// used when the API response somehow doesn't carry a dimension (e.g.
/// a legacy model dropped from the catalog that's still attached to
/// a course). Keep this list in sync with `pipeline::VALID_LOCAL_MODELS`
/// on the backend; an unknown id falls back to the raw HF id with no
/// description, which is ugly but not broken.
///
/// Also covers the OpenAI canonical model so the admin courses table
/// can render a friendly name for openai-provider rows without a
/// special-case branch at the call site.
export const MODEL_DISPLAY: Record<
  string,
  { name: string; dims: number; descKey: string }
> = {
  "sentence-transformers/all-MiniLM-L6-v2": { name: "all-MiniLM-L6-v2", dims: 384, descKey: "config.embeddingModels.miniLmDesc" },
  "BAAI/bge-small-en-v1.5": { name: "BGE Small EN v1.5", dims: 384, descKey: "config.embeddingModels.bgeSmallDesc" },
  "BAAI/bge-base-en-v1.5": { name: "BGE Base EN v1.5", dims: 768, descKey: "config.embeddingModels.bgeBaseDesc" },
  "nomic-ai/nomic-embed-text-v1.5": { name: "Nomic Embed Text v1.5", dims: 768, descKey: "config.embeddingModels.nomicDesc" },
  "intfloat/multilingual-e5-small": { name: "Multilingual E5 Small", dims: 384, descKey: "config.embeddingModels.e5SmallDesc" },
  "intfloat/multilingual-e5-base": { name: "Multilingual E5 Base", dims: 768, descKey: "config.embeddingModels.e5BaseDesc" },
  "intfloat/multilingual-e5-large": { name: "Multilingual E5 Large", dims: 1024, descKey: "config.embeddingModels.e5LargeDesc" },
  "BAAI/bge-m3": { name: "BGE M3", dims: 1024, descKey: "config.embeddingModels.bgeM3Desc" },
  "google/embeddinggemma-300m": { name: "EmbeddingGemma 300M", dims: 768, descKey: "config.embeddingModels.gemmaDesc" },
  "mixedbread-ai/mxbai-embed-large-v1": { name: "Mxbai Embed Large v1", dims: 1024, descKey: "config.embeddingModels.mxbaiDesc" },
  "Alibaba-NLP/gte-large-en-v1.5": { name: "GTE Large EN v1.5", dims: 1024, descKey: "config.embeddingModels.gteDesc" },
  "snowflake/snowflake-arctic-embed-l": { name: "Arctic Embed L", dims: 1024, descKey: "config.embeddingModels.arcticDesc" },
  "Qwen/Qwen3-Embedding-0.6B": { name: "Qwen3 Embedding 0.6B", dims: 1024, descKey: "config.embeddingModels.qwen3Desc" },
  "text-embedding-3-small": { name: "OpenAI text-embedding-3-small", dims: 1536, descKey: "" },
}

/// Friendly display name for a model id, falling back to the raw id if
/// it's not in the catalog (e.g. a model that's been removed from
/// `VALID_LOCAL_MODELS` but is still attached to a course).
export function modelDisplayName(modelId: string): string {
  return MODEL_DISPLAY[modelId]?.name ?? modelId
}
