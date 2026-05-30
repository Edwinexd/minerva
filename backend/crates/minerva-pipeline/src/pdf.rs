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
