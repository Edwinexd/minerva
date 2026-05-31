//! Detect GitHub-hosted PDF URLs and normalize them to a direct-download form.
//!
//! Used by the document worker: a `.url` stub whose payload is a GitHub PDF
//! link is downloaded inline (no external pipeline needed; public release
//! assets and repo-tracked PDFs are anonymously fetchable). The downloaded
//! bytes then re-enter the normal PDF ingest path.
//!
//! Patterns recognized (all require the path component to end in `.pdf`,
//! case-insensitive):
//!
//! ```text
//! https://github.com/{owner}/{repo}/raw/{ref...}/{path}.pdf
//! https://github.com/{owner}/{repo}/blob/{ref...}/{path}.pdf       -> rewrites blob -> raw
//! https://raw.githubusercontent.com/{owner}/{repo}/{ref...}/{path}.pdf
//! https://github.com/{owner}/{repo}/releases/download/{tag}/{file}.pdf
//! https://github.com/{owner}/{repo}/releases/latest/download/{file}.pdf
//! ```
//!
//! Anything else, including `gist.github.com`, `github.io` pages, and HTML
//! preview pages without an explicit `.pdf` suffix, is rejected (the worker
//! falls through to its existing `unsupported` branch).

/// A GitHub URL that resolves to a downloadable PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubPdfUrl {
    /// The exact URL to GET to retrieve the PDF bytes. For `/blob/` inputs
    /// this is the rewritten `/raw/` form; for everything else it equals the
    /// input modulo `#fragment` trimming. We preserve `?query` because
    /// release-download links sometimes carry signed tokens.
    pub download_url: String,
    /// Suggested on-disk filename (basename of the URL path). Guaranteed to
    /// end in `.pdf` (case-insensitive) and contain no path separators or
    /// `..` traversal. Callers still apply their own filename sanitization.
    pub suggested_filename: String,
}

/// Try to interpret `url` as one of the supported GitHub PDF URL shapes.
/// Returns `None` for anything that isn't a clear, direct-download GitHub
/// PDF; the worker keeps its conservative posture of marking unknown
/// URLs `unsupported` rather than guessing.
pub fn detect(url: &str) -> Option<GithubPdfUrl> {
    let url = url.trim();

    // Drop any fragment, but keep the query (release links sometimes carry
    // tracking params and the asset endpoint accepts them harmlessly).
    let no_fragment = url.split_once('#').map(|(u, _)| u).unwrap_or(url);
    let (path_part, query_part) = match no_fragment.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (no_fragment, None),
    };

    let after_scheme = path_part.strip_prefix("https://")?;
    let (host, path) = after_scheme.split_once('/')?;

    // Reject `..` anywhere in the path (defense against any path-traversal
    // shenanigans that might leak via the suggested filename).
    if path.split('/').any(|seg| seg == ".." || seg.contains("..")) {
        return None;
    }

    // Final segment must end in `.pdf` (case-insensitive).
    let last_segment = path.rsplit('/').next()?;
    if !last_segment.to_ascii_lowercase().ends_with(".pdf") {
        return None;
    }
    if last_segment.len() == ".pdf".len() {
        // Bare ".pdf" with no stem; almost certainly not a real file.
        return None;
    }

    let download_path = match host {
        "raw.githubusercontent.com" => {
            // /{owner}/{repo}/{ref...}/{path}; minimum 4 non-empty segments.
            if path.split('/').filter(|s| !s.is_empty()).count() < 4 {
                return None;
            }
            path.to_string()
        }
        "github.com" => {
            let mut segs = path.split('/');
            let owner = segs.next()?;
            let repo = segs.next()?;
            let kind = segs.next()?;
            if owner.is_empty() || repo.is_empty() {
                return None;
            }
            match kind {
                "raw" => {
                    // Need at least {ref} and {filename} after /raw/.
                    let rest: Vec<&str> = segs.collect();
                    if rest.len() < 2 || rest.iter().any(|s| s.is_empty()) {
                        return None;
                    }
                    path.to_string()
                }
                "blob" => {
                    // /blob/ is an HTML viewer; rewrite to /raw/ which 302s
                    // to raw.githubusercontent.com for the actual bytes.
                    let rest: Vec<&str> = segs.collect();
                    if rest.len() < 2 || rest.iter().any(|s| s.is_empty()) {
                        return None;
                    }
                    format!("{}/{}/raw/{}", owner, repo, rest.join("/"))
                }
                "releases" => {
                    // Two shapes: /releases/download/{tag}/{file} and
                    // /releases/latest/download/{file}.
                    match segs.next()? {
                        "download" => {
                            let rest: Vec<&str> = segs.collect();
                            if rest.len() < 2 || rest.iter().any(|s| s.is_empty()) {
                                return None;
                            }
                            path.to_string()
                        }
                        "latest" => {
                            if segs.next()? != "download" {
                                return None;
                            }
                            let rest: Vec<&str> = segs.collect();
                            // GitHub's /releases/latest/download/ convenience
                            // redirect only accepts a bare filename.
                            if rest.len() != 1 || rest[0].is_empty() {
                                return None;
                            }
                            path.to_string()
                        }
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        _ => return None,
    };

    let mut download_url = format!("https://{}/{}", host, download_path);
    if let Some(q) = query_part {
        download_url.push('?');
        download_url.push_str(q);
    }

    Some(GithubPdfUrl {
        download_url,
        suggested_filename: last_segment.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_github_url_passes_through() {
        let got = detect("https://github.com/Edwinexd/minerva/raw/master/docs/spec.pdf")
            .expect("should match");
        assert_eq!(
            got.download_url,
            "https://github.com/Edwinexd/minerva/raw/master/docs/spec.pdf"
        );
        assert_eq!(got.suggested_filename, "spec.pdf");
    }

    #[test]
    fn raw_refs_heads_branch_passes_through() {
        // GitHub's "Copy permalink" UI emits this longer form; treat it the
        // same as the shorthand /raw/{branch}/...
        let got = detect("https://github.com/Edwinexd/minerva/raw/refs/heads/master/docs/spec.pdf")
            .expect("should match");
        assert_eq!(got.suggested_filename, "spec.pdf");
        assert!(got.download_url.contains("/raw/refs/heads/master/"));
    }

    #[test]
    fn blob_url_rewrites_to_raw() {
        let got = detect("https://github.com/Edwinexd/minerva/blob/master/docs/spec.pdf")
            .expect("should match");
        assert_eq!(
            got.download_url,
            "https://github.com/Edwinexd/minerva/raw/master/docs/spec.pdf"
        );
    }

    #[test]
    fn raw_githubusercontent_url_passes_through() {
        let got = detect("https://raw.githubusercontent.com/Edwinexd/minerva/master/docs/spec.pdf")
            .expect("should match");
        assert_eq!(
            got.download_url,
            "https://raw.githubusercontent.com/Edwinexd/minerva/master/docs/spec.pdf"
        );
        assert_eq!(got.suggested_filename, "spec.pdf");
    }

    #[test]
    fn releases_download_url_passes_through() {
        let got =
            detect("https://github.com/Edwinexd/minerva/releases/download/v1.2.0/handbook.pdf")
                .expect("should match");
        assert_eq!(
            got.download_url,
            "https://github.com/Edwinexd/minerva/releases/download/v1.2.0/handbook.pdf"
        );
        assert_eq!(got.suggested_filename, "handbook.pdf");
    }

    #[test]
    fn releases_latest_download_url_passes_through() {
        let got =
            detect("https://github.com/Edwinexd/minerva/releases/latest/download/handbook.pdf")
                .expect("should match");
        assert_eq!(
            got.download_url,
            "https://github.com/Edwinexd/minerva/releases/latest/download/handbook.pdf"
        );
    }

    #[test]
    fn query_string_is_preserved() {
        let got = detect("https://github.com/Edwinexd/minerva/raw/master/docs/spec.pdf?token=abc")
            .expect("should match");
        assert!(got.download_url.ends_with("/spec.pdf?token=abc"));
    }

    #[test]
    fn fragment_is_dropped() {
        let got = detect("https://github.com/Edwinexd/minerva/raw/master/docs/spec.pdf#page=3")
            .expect("should match");
        assert!(!got.download_url.contains('#'));
        assert!(got.download_url.ends_with("/spec.pdf"));
    }

    #[test]
    fn uppercase_pdf_extension_matches() {
        assert!(detect("https://github.com/Edwinexd/minerva/raw/master/docs/SPEC.PDF").is_some());
    }

    #[test]
    fn non_pdf_path_rejected() {
        assert!(detect("https://github.com/Edwinexd/minerva/raw/master/docs/spec.txt").is_none());
        assert!(detect("https://github.com/Edwinexd/minerva/blob/master/README.md").is_none());
    }

    #[test]
    fn non_github_host_rejected() {
        assert!(detect("https://example.com/foo.pdf").is_none());
        assert!(detect("https://gist.github.com/Edwinexd/abc/raw/foo.pdf").is_none());
        assert!(detect("https://edwinexd.github.io/site/foo.pdf").is_none());
    }

    #[test]
    fn http_scheme_rejected() {
        // We don't ingest over plaintext HTTP; GitHub redirects to HTTPS
        // anyway, but rejecting up-front avoids the wasted round-trip.
        assert!(detect("http://github.com/Edwinexd/minerva/raw/master/docs/spec.pdf").is_none());
    }

    #[test]
    fn unknown_github_path_kind_rejected() {
        // /tree/, /commit/, /pull/, /wiki/ etc. are HTML, not file content.
        assert!(detect("https://github.com/Edwinexd/minerva/tree/master/docs/spec.pdf").is_none());
        assert!(detect("https://github.com/Edwinexd/minerva/wiki/spec.pdf").is_none());
    }

    #[test]
    fn malformed_releases_paths_rejected() {
        // Missing /download/ after /releases/.
        assert!(
            detect("https://github.com/Edwinexd/minerva/releases/v1.2.0/handbook.pdf").is_none()
        );
        // /releases/latest/handbook.pdf without /download/.
        assert!(
            detect("https://github.com/Edwinexd/minerva/releases/latest/handbook.pdf").is_none()
        );
        // /releases/latest/download/ only accepts a bare filename.
        assert!(detect(
            "https://github.com/Edwinexd/minerva/releases/latest/download/sub/handbook.pdf",
        )
        .is_none());
    }

    #[test]
    fn path_traversal_rejected() {
        // `..` segment can't appear anywhere in the path.
        assert!(
            detect("https://github.com/Edwinexd/minerva/raw/master/../etc/passwd.pdf",).is_none()
        );
        // Filename starting with `..` (e.g. `..pdf`) is also rejected for
        // simplicity; real GitHub paths never need this.
        assert!(detect("https://github.com/Edwinexd/minerva/raw/master/docs/..pdf").is_none());
    }

    #[test]
    fn raw_path_must_have_ref_and_file() {
        // /raw/ alone or /raw/branch/ without a file segment is malformed.
        assert!(detect("https://github.com/Edwinexd/minerva/raw/").is_none());
        assert!(detect("https://github.com/Edwinexd/minerva/raw/master/").is_none());
    }

    #[test]
    fn bare_dot_pdf_rejected() {
        // A path ending in `.pdf` with no stem isn't a real file.
        assert!(detect("https://github.com/Edwinexd/minerva/raw/master/docs/.pdf").is_none());
    }
}
