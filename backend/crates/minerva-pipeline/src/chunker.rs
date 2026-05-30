/// A chunk of text with metadata about its origin.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub index: usize,
    pub page_number: Option<usize>,
}

/// Configuration for the chunking strategy.
pub struct ChunkerConfig {
    /// Target chunk size in characters (~4 chars per token).
    pub chunk_size: usize,
    /// Overlap between consecutive chunks in characters.
    pub overlap: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            chunk_size: 2000, // ~500 tokens
            overlap: 250,     // ~64 tokens
        }
    }
}

/// Snap a byte offset to the nearest char boundary (rounding down).
fn snap_to_char_boundary(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    let mut p = pos;
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Chunk text using a sliding window approach that respects paragraph boundaries.
pub fn chunk_text(text: &str, config: &ChunkerConfig) -> Vec<Chunk> {
    let cleaned = clean_text(text);
    if cleaned.trim().is_empty() {
        return Vec::new();
    }

    let paragraphs = split_paragraphs(&cleaned);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut chunk_index = 0;

    for paragraph in &paragraphs {
        // If adding this paragraph would exceed chunk_size and we have content,
        // finalize the current chunk
        if !current.is_empty() && current.len() + paragraph.len() > config.chunk_size {
            chunks.push(Chunk {
                text: current.trim().to_string(),
                index: chunk_index,
                page_number: None,
            });
            chunk_index += 1;

            // Start new chunk with overlap from the end of current
            let overlap_start =
                snap_to_char_boundary(&current, current.len().saturating_sub(config.overlap));
            current = current[overlap_start..].to_string();
        }

        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(paragraph);

        // If a single paragraph exceeds chunk_size, split it by sentences
        while current.len() > config.chunk_size {
            let split_point = find_split_point(&current, config.chunk_size);
            let piece = current[..split_point].trim().to_string();
            if !piece.is_empty() {
                chunks.push(Chunk {
                    text: piece,
                    index: chunk_index,
                    page_number: None,
                });
                chunk_index += 1;
            }

            let overlap_start =
                snap_to_char_boundary(&current, split_point.saturating_sub(config.overlap));
            current = current[overlap_start..].to_string();
        }
    }

    // Don't forget the last chunk
    let remaining = current.trim().to_string();
    if !remaining.is_empty() {
        chunks.push(Chunk {
            text: remaining,
            index: chunk_index,
            page_number: None,
        });
    }

    chunks
}

/// Clean extracted PDF text by removing common artifacts.
fn clean_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();

    // Detect repeated headers/footers (lines that appear on many "pages")
    let mut line_counts = std::collections::HashMap::new();
    for line in &lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed.len() < 80 {
            *line_counts.entry(trimmed).or_insert(0) += 1;
        }
    }

    let repeated: std::collections::HashSet<&str> = line_counts
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .map(|(line, _)| line)
        .collect();

    lines
        .iter()
        .filter(|line| {
            let trimmed = line.trim();
            !repeated.contains(trimmed)
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split text into paragraphs (separated by double newlines).
fn split_paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Find a good split point near the target position.
/// Prefers splitting at paragraph breaks, then sentence boundaries, then word boundaries.
fn find_split_point(text: &str, target: usize) -> usize {
    let target = snap_to_char_boundary(text, target);

    // Look for paragraph break near target
    if let Some(pos) = text[..target].rfind("\n\n") {
        if pos > target / 2 {
            return pos + 2;
        }
    }

    // Look for sentence boundary (". " or ".\n")
    if let Some(pos) = text[..target].rfind(". ") {
        if pos > target / 2 {
            return pos + 2;
        }
    }

    // Look for word boundary
    if let Some(pos) = text[..target].rfind(' ') {
        return pos + 1;
    }

    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_chunking() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunk_text(
            text,
            &ChunkerConfig {
                chunk_size: 30,
                overlap: 5,
            },
        );
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(!chunk.text.is_empty());
        }
    }

    #[test]
    fn test_empty_text() {
        let chunks = chunk_text("", &ChunkerConfig::default());
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_single_small_paragraph() {
        let chunks = chunk_text("Hello world.", &ChunkerConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello world.");
    }

    #[test]
    fn test_multibyte_text() {
        let text = "Hej alla studenter! Vi ska prata om AI och maskininlarning.\n\nForsta avsnittet handlar om neurala natverk. Det ar ett spannande amne som har forandrat varlden.";
        let chunks = chunk_text(
            text,
            &ChunkerConfig {
                chunk_size: 60,
                overlap: 10,
            },
        );
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(!chunk.text.is_empty());
        }
    }
}
