//! Staging downloads: one claimed lecture at a time, earliest token expiry
//! first, written to a claim-unique `.part` file and renamed only once the
//! bytes match the reservation and look like an MP4.

use chrono::Utc;
use minerva_app_core::visual_extraction as ve;
use minerva_db::queries::visual_extraction as queue;
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;

const MAX_VIDEO_BYTES: u64 = ve::FETCH_ESTIMATE_BYTES as u64;

pub(crate) fn media_client() -> Result<reqwest::Client, &'static str> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| "download client")
}

/// Enforce the reservation before writing each chunk, including when GET
/// omits Content-Length. A changed or truncated source cannot be published.
async fn write_video(
    response: reqwest::Response,
    partial: &str,
    expected_bytes: Option<i64>,
) -> Result<u64, &'static str> {
    use futures::StreamExt;
    let byte_limit = expected_bytes.map_or(MAX_VIDEO_BYTES, |n| n as u64);
    if response.status() != reqwest::StatusCode::OK {
        return Err("Play response");
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return Err("encoded media response");
    }
    if response.content_length().is_some_and(|n| n > byte_limit) {
        return Err("video too large");
    }
    let expected = response.content_length();
    if expected_bytes.is_some_and(|n| expected.is_some_and(|m| m != n as u64)) {
        return Err("media changed after sizing");
    }
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(|_| "staging write")?;
    let mut bytes = 0u64;
    let mut header = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Play stream")?;
        bytes += chunk.len() as u64;
        if bytes > byte_limit {
            return Err("video exceeded reservation");
        }
        if header.len() < 12 {
            header.extend(chunk.iter().take(12 - header.len()));
        }
        file.write_all(&chunk).await.map_err(|_| "staging write")?;
    }
    if header.get(4..8) != Some(b"ftyp")
        || expected.is_some_and(|n| n != bytes)
        || expected_bytes.is_some_and(|n| n as u64 != bytes)
    {
        return Err("invalid or truncated MP4");
    }
    file.flush().await.map_err(|_| "staging flush")?;
    drop(file);
    Ok(bytes)
}

/// A separate loop so a slow download never delays Slurm reconciliation.
pub async fn download_tick(db: &PgPool, staging_path: &str) -> Result<bool, sqlx::Error> {
    let Some(claim) = queue::claim_download(db).await? else {
        return Ok(false);
    };
    let started = std::time::Instant::now();
    // Unique per claim: a process returning after its reservation expired
    // cannot overwrite a newer attempt's file.
    let filename = format!("{}.{}.mp4", claim.id, claim.started_at.timestamp_micros());
    let path = format!("{staging_path}/{filename}");
    let partial = format!("{path}.part");
    let seconds = (claim.deadline - Utc::now()).num_seconds().clamp(0, 1800) as u64;
    let result = tokio::time::timeout(std::time::Duration::from_secs(seconds), async {
        ve::validate_download_url(&claim.url, claim.deadline).map_err(str::to_string)?;
        tokio::fs::create_dir_all(staging_path)
            .await
            .map_err(|_| "staging directory")?;
        let client = media_client()?;
        // Never format reqwest errors: their Display can contain the bearer URL.
        let response = client
            .get(&claim.url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|_| "Play request")?;
        let bytes = write_video(response, &partial, claim.expected_bytes)
            .await
            .map_err(str::to_string)?;
        tokio::fs::rename(&partial, &path)
            .await
            .map_err(|_| "staging rename")?;
        Ok::<u64, String>(bytes)
    })
    .await;
    let bytes = match result {
        Ok(Ok(bytes)) => Some(bytes),
        _ => {
            tracing::warn!(job_id = %claim.id, "Play download failed or expired");
            None
        }
    };
    let published = queue::finish_download(
        db,
        &claim,
        bytes.map(|_| filename.as_str()),
        bytes.unwrap_or(0) as i64,
    )
    .await;
    let _ = tokio::fs::remove_file(&partial).await;
    if bytes.is_none() || !matches!(published, Ok(true)) {
        let _ = tokio::fs::remove_file(&path).await;
    }
    let published = published?;
    let outcome = if !published {
        "stale"
    } else if bytes.is_some() {
        "succeeded"
    } else {
        "failed"
    };
    metrics::counter!("visual_extraction_downloads_total", "outcome" => outcome).increment(1);
    metrics::histogram!("visual_extraction_download_duration_seconds", "outcome" => outcome)
        .record(started.elapsed().as_secs_f64());
    Ok(true)
}

pub async fn cleanup_download_files(db: &PgPool, staging_path: &str) -> Result<(), sqlx::Error> {
    let retained: std::collections::HashSet<_> = queue::retained_staging_paths(db)
        .await?
        .into_iter()
        .collect();
    let Ok(mut files) = tokio::fs::read_dir(staging_path).await else {
        return Ok(());
    };
    while let Ok(Some(file)) = files.next_entry().await {
        let name = file.file_name().to_string_lossy().into_owned();
        let final_name = name.strip_suffix(".part").unwrap_or(&name);
        // Only pipeline-owned MP4s, and only well past the 30-minute download
        // timeout. Preserve every active claim and published source.
        if !final_name.ends_with(".mp4") || retained.contains(final_name) {
            continue;
        }
        if let Ok(metadata) = file.metadata().await {
            if metadata.is_file()
                && metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 2400)
            {
                let _ = tokio::fs::remove_file(file.path()).await;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            assert!(request.len() < 4096);
        }
        String::from_utf8(request).unwrap()
    }

    #[tokio::test]
    async fn streams_cannot_outgrow_or_undershoot_the_reserved_size() {
        let mp4 = b"\0\0\0\x20ftypisom01234567890123456789";
        assert_eq!(mp4.len(), 32);
        for (length, chunked, expected, success) in [
            (32, false, Some(32), true),
            (32, true, None, true),
            (32, true, Some(32), true),
            (32, false, Some(24), false),
            (32, true, Some(24), false),
            (24, true, Some(32), false),
            (24, false, Some(32), false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_request(&mut stream).await;
                let body = &mp4[..length];
                let reply = if chunked {
                    format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{length:x}\r\n{}\r\n0\r\n\r\n", String::from_utf8_lossy(body))
                } else {
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{}", String::from_utf8_lossy(body))
                };
                stream.write_all(reply.as_bytes()).await.unwrap();
            });
            let response = media_client()
                .unwrap()
                .get(format!("http://{address}/video.mp4"))
                .send()
                .await
                .unwrap();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("video.part");
            let result = write_video(response, path.to_str().unwrap(), expected).await;
            assert_eq!(
                result.is_ok(),
                success,
                "length={length} chunked={chunked} expected={expected:?}"
            );
            if success {
                assert_eq!(result.unwrap(), 32);
            }
            if let Ok(metadata) = tokio::fs::metadata(path).await {
                assert!(metadata.len() <= expected.unwrap_or(32) as u64);
            }
            server.await.unwrap();
        }
    }
}
