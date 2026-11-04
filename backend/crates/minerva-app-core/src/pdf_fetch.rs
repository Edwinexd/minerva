//! Capped, magic-bytes-checked PDF download shared by the ingest worker
//! (GitHub PDF links) and the syllabus archive loop.

/// GET `url` and return the body if it is a PDF no larger than `max_bytes`.
///
/// The cap is enforced on `Content-Length` up front and again while
/// streaming, since the header can be absent or wrong. A body without the
/// `%PDF-` header is rejected: hosts commonly answer a missing file with an
/// HTML page and status 200.
pub async fn fetch_pdf(
    client: &reqwest::Client,
    url: &str,
    max_bytes: usize,
    user_agent: &str,
) -> Result<Vec<u8>, String> {
    let mut resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, user_agent)
        .header(reqwest::header::ACCEPT, "application/pdf, */*")
        .send()
        .await
        .map_err(|e| format!("request: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("http {}", status.as_u16()));
    }

    if let Some(len) = resp.content_length() {
        if len as usize > max_bytes {
            return Err(format!("response too large ({} bytes)", len));
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("body read: {}", e))?
    {
        if buf.len() + chunk.len() > max_bytes {
            return Err(format!("response exceeds {} byte cap", max_bytes));
        }
        buf.extend_from_slice(&chunk);
    }

    if !buf.starts_with(b"%PDF-") {
        return Err("response is not a PDF (missing %PDF- header)".to_string());
    }
    Ok(buf)
}
