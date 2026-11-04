use std::path::Path;
use std::process::Command;

/// Extract text from a PDF file.
/// Tries pdf-extract first, falls back to pdftotext (poppler-utils).
pub fn extract_text(path: &Path) -> Result<String, String> {
    // Try pdf-extract first (native Rust)
    match pdf_extract::extract_text(path) {
        Ok(text) if !text.trim().is_empty() => {
            tracing::debug!("pdf-extract succeeded for {:?}", path);
            return Ok(text);
        }
        Ok(_) => {
            tracing::debug!(
                "pdf-extract returned empty text for {:?}, trying pdftotext",
                path
            );
        }
        Err(e) => {
            tracing::debug!("pdf-extract failed for {:?}: {}, trying pdftotext", path, e);
        }
    }

    // Fallback to pdftotext
    let output = Command::new("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output()
        .map_err(|e| format!("failed to run pdftotext: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("pdftotext failed: {}", stderr));
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        return Err("both pdf-extract and pdftotext returned empty text".to_string());
    }

    tracing::debug!("pdftotext succeeded for {:?}", path);
    Ok(text)
}

/// Extract a PDF's text one page at a time, with the layout kept (columns
/// and table rows stay on their lines), for callers that cite pages or
/// read tabular fields. `pdftotext -layout` separates pages with a form
/// feed; pdf-extract is the fallback when poppler is missing or fails.
pub fn extract_pages(path: &Path) -> Result<Vec<String>, String> {
    let layout = Command::new("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output();
    if let Ok(output) = layout {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let pages = split_pages(&text);
            if pages.iter().any(|p| !p.trim().is_empty()) {
                return Ok(pages);
            }
        }
    }
    let pages = pdf_extract::extract_text_by_pages(path)
        .map_err(|e| format!("page extraction failed: {}", e))?;
    if pages.iter().all(|p| p.trim().is_empty()) {
        return Err("the PDF has no extractable text".to_string());
    }
    Ok(pages)
}

/// Join pages into one text, each opened by a `[Page N]` marker, so a model
/// reading it can cite the page a passage is on.
pub fn mark_pages(pages: &[String]) -> String {
    let mut out = String::new();
    for (i, page) in pages.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format!("[Page {}]\n", i + 1));
        out.push_str(page.trim_end());
    }
    out
}

/// Split `pdftotext` output on its form feeds. The output ends with one
/// after the last page, which would otherwise count as an empty page.
fn split_pages(text: &str) -> Vec<String> {
    let mut pages: Vec<String> = text.split('\u{c}').map(str::to_string).collect();
    if pages.len() > 1 && pages.last().is_some_and(|p| p.trim().is_empty()) {
        pages.pop();
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::{mark_pages, split_pages};

    #[test]
    fn marks_each_page_with_its_number() {
        let pages = vec!["first\n".to_string(), "second".to_string()];
        assert_eq!(mark_pages(&pages), "[Page 1]\nfirst\n\n[Page 2]\nsecond");
    }

    #[test]
    fn splits_on_form_feeds_without_a_trailing_empty_page() {
        assert_eq!(split_pages("one\u{c}two\u{c}"), vec!["one", "two"]);
        assert_eq!(split_pages("only"), vec!["only"]);
        // An empty page in the middle is a real page and keeps its number.
        assert_eq!(split_pages("a\u{c}\u{c}c\u{c}"), vec!["a", "", "c"]);
    }
}
