//! Moodle course backup (.mbz) importer.
//!
//! Extracts a gzipped-tar `.mbz` archive and yields one [`MbzItem`] per piece
//! of course material the teacher would see on the course page. The visibility
//! rules match the `local_minerva` Moodle plugin so admins with access to a
//! backup file can ingest the same content they would get by sync.
//!
//! Rules enforced here:
//! * Activities: skipped unless `module.xml` has `<visible>1</visible>` **and**
//!   `<availability>` is absent or the literal Moodle null marker `$@NULL@$`.
//! * Sections: skipped unless `section.xml` has `<visible>1</visible>`.
//! * Book chapters: skipped when `<hidden>` is set.
//! * Files: only `mod_*` components with filearea `content` belonging to a
//!   visible activity's context id are included.
//!
//! The returned [`MbzImport`] owns the extraction directory via a held
//! [`TempDir`]; drop it after you have copied each item's bytes out.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use serde::Deserialize;
use tempfile::TempDir;
use thiserror::Error;

/// Moodle's sentinel for a null `<availability>` element.
const NULL_AVAILABILITY: &str = "$@NULL@$";

#[derive(Debug, Error)]
pub enum MbzError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("archive extraction failed: {0}")]
    Extract(String),
    #[error("not a Moodle backup: missing moodle_backup.xml")]
    NotABackup,
    #[error("xml parse error in {path}: {source}")]
    Xml {
        path: String,
        #[source]
        source: quick_xml::DeError,
    },
}

/// Where the bytes for an importable item live.
pub enum ItemBody {
    /// Literal bytes built in-memory (wrapped HTML, URL text).
    Inline(Vec<u8>),
    /// Path inside the extraction directory.
    File(PathBuf),
}

/// A single piece of course material to be ingested.
pub struct MbzItem {
    pub filename: String,
    pub mime: String,
    pub body: ItemBody,
    /// Short human-readable label for logs / UI (not persisted).
    pub display: String,
}

/// Result of importing a `.mbz`. The held [`TempDir`] is cleaned up on drop.
pub struct MbzImport {
    pub items: Vec<MbzItem>,
    pub skipped_hidden: usize,
    _tmp: TempDir,
}

/// Extract and walk a `.mbz` archive.
pub fn import_mbz(archive: &Path) -> Result<MbzImport, MbzError> {
    import_mbz_at(archive, now_unix())
}

/// Like [`import_mbz`] but takes an explicit "now" unix timestamp, which the
/// availability evaluator uses to decide whether a date-gated activity is
/// currently unlocked. Exposed for tests.
pub fn import_mbz_at(archive: &Path, now_unix_ts: i64) -> Result<MbzImport, MbzError> {
    let tmp = tempfile::Builder::new().prefix("minerva-mbz-").tempdir()?;
    extract_tgz(archive, tmp.path())?;

    if !tmp.path().join("moodle_backup.xml").is_file() {
        return Err(MbzError::NotABackup);
    }

    let walk = build_items(tmp.path(), now_unix_ts)?;

    Ok(MbzImport {
        items: walk.items,
        skipped_hidden: walk.skipped_hidden,
        _tmp: tmp,
    })
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn extract_tgz(archive: &Path, dest: &Path) -> Result<(), MbzError> {
    let f = fs::File::open(archive)?;
    let gz = GzDecoder::new(f);
    let mut ar = tar::Archive::new(gz);
    // tar::Archive::unpack refuses absolute paths and `..` traversal by default.
    ar.unpack(dest)
        .map_err(|e| MbzError::Extract(e.to_string()))?;
    Ok(())
}

struct Walk {
    items: Vec<MbzItem>,
    skipped_hidden: usize,
}

fn build_items(root: &Path, now_ts: i64) -> Result<Walk, MbzError> {
    let mut items: Vec<MbzItem> = Vec::new();
    let mut skipped_hidden: usize = 0;
    // contextid -> (kept | dropped-as-hidden). Used when filtering files.xml.
    let mut visible_contexts: std::collections::HashSet<i64> = std::collections::HashSet::new();

    // Activities.
    let activities_dir = root.join("activities");
    if activities_dir.is_dir() {
        for entry in fs::read_dir(&activities_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dirname = entry.file_name().to_string_lossy().into_owned();

            let module_xml_path = path.join("module.xml");
            let module: ModuleXml = match load_xml(&module_xml_path)? {
                Some(m) => m,
                None => continue,
            };

            if !module.is_visible(now_ts) {
                skipped_hidden += 1;
                continue;
            }

            let modulename = module.modulename.as_str();
            // activity file (one of url.xml, page.xml, etc.) carries the
            // context id we need to match with files.xml.
            let activity_xml_path = path.join(format!("{}.xml", modulename));
            if let Some(ctxid) = load_activity_contextid(&activity_xml_path)? {
                visible_contexts.insert(ctxid);
            }

            match modulename {
                "url" => add_url_item(&activity_xml_path, &dirname, &mut items)?,
                "page" => add_page_item(&activity_xml_path, &dirname, &mut items)?,
                "book" => add_book_items(&activity_xml_path, &dirname, &mut items)?,
                "label" => add_label_item(&activity_xml_path, &dirname, &mut items)?,
                "resource" => add_resource_intro(&activity_xml_path, &dirname, &mut items)?,
                // Quiz, assign, forum, etc.: user-generated or interactive,
                // not course material.
                _ => {}
            }
        }
    }

    // Section summaries.
    let sections_dir = root.join("sections");
    if sections_dir.is_dir() {
        for entry in fs::read_dir(&sections_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let section_xml_path = path.join("section.xml");
            let section: SectionXml = match load_xml(&section_xml_path)? {
                Some(s) => s,
                None => continue,
            };
            if section.visible != 1 {
                skipped_hidden += 1;
                continue;
            }
            if let Some(html) = section_summary_html(&section) {
                let label = section_label(&section);
                let display = format!("{} (section summary)", label);
                let id = section.id.unwrap_or_default();
                let item = build_html_item(
                    "section",
                    id,
                    &display,
                    &html,
                    section.summaryformat.unwrap_or(1),
                );
                items.push(item);
            }
        }
    }

    // Attached files from visible modules.
    let files_xml_path = root.join("files.xml");
    if files_xml_path.is_file() {
        let parsed: FilesXml = match load_xml(&files_xml_path)? {
            Some(f) => f,
            None => FilesXml { file: Vec::new() },
        };
        for f in parsed.file {
            if !f.component.starts_with("mod_") {
                continue;
            }
            if f.filearea != "content" {
                continue;
            }
            if f.filename.is_empty() || f.filename == "." {
                continue;
            }
            if f.filesize == 0 {
                continue;
            }
            if f.contenthash.len() < 3 {
                continue;
            }
            if !visible_contexts.contains(&f.contextid) {
                continue;
            }
            let src = root
                .join("files")
                .join(&f.contenthash[..2])
                .join(&f.contenthash);
            if !src.is_file() {
                continue;
            }
            let mime = if f.mimetype.trim().is_empty() {
                "application/octet-stream".to_string()
            } else {
                f.mimetype
            };
            items.push(MbzItem {
                filename: f.filename.clone(),
                display: f.filename,
                mime,
                body: ItemBody::File(src),
            });
        }
    }

    Ok(Walk {
        items,
        skipped_hidden,
    })
}

fn load_xml<T>(path: &Path) -> Result<Option<T>, MbzError>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    // Strip UTF-8 BOM if present (Moodle writes clean UTF-8 but defensive).
    let slice: &[u8] = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        &bytes
    };
    let parsed: T = quick_xml::de::from_reader(slice).map_err(|e| MbzError::Xml {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(Some(parsed))
}

fn load_activity_contextid(path: &Path) -> Result<Option<i64>, MbzError> {
    #[derive(Deserialize)]
    struct ActivityShell {
        #[serde(rename = "@contextid")]
        contextid: Option<i64>,
    }
    Ok(load_xml::<ActivityShell>(path)?.and_then(|a| a.contextid))
}

fn add_url_item(
    activity_xml_path: &Path,
    dirname: &str,
    items: &mut Vec<MbzItem>,
) -> Result<(), MbzError> {
    let parsed: UrlActivity = match load_xml(activity_xml_path)? {
        Some(p) => p,
        None => return Ok(()),
    };
    let external = parsed.url.externalurl.trim();
    if external.is_empty() {
        return Ok(());
    }
    let name = parsed
        .url
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("URL");
    let filename = format!("{}.url", safe_slug(name));
    let _ = dirname;

    items.push(MbzItem {
        filename,
        mime: "text/x-url".to_string(),
        body: ItemBody::Inline(external.as_bytes().to_vec()),
        display: format!("{} (URL)", name),
    });
    Ok(())
}

fn add_page_item(
    activity_xml_path: &Path,
    dirname: &str,
    items: &mut Vec<MbzItem>,
) -> Result<(), MbzError> {
    let parsed: PageActivity = match load_xml(activity_xml_path)? {
        Some(p) => p,
        None => return Ok(()),
    };
    let p = parsed.page;
    let id = p.id.unwrap_or_else(|| suffix_id(dirname).unwrap_or(0));
    let name = p.name.as_deref().map(str::trim).unwrap_or("Page");
    let content = p.content.unwrap_or_default();
    let format = p.contentformat.unwrap_or(1);
    if is_empty_html(&content) {
        return Ok(());
    }
    items.push(build_html_item("page", id, name, &content, format));
    Ok(())
}

fn add_book_items(
    activity_xml_path: &Path,
    dirname: &str,
    items: &mut Vec<MbzItem>,
) -> Result<(), MbzError> {
    let parsed: BookActivity = match load_xml(activity_xml_path)? {
        Some(p) => p,
        None => return Ok(()),
    };
    let _ = dirname;
    let book = parsed.book;
    let bookname = book.name.as_deref().map(str::trim).unwrap_or("Book");
    let chapters = book.chapters.map(|c| c.chapter).unwrap_or_default();
    for chapter in chapters {
        if chapter.hidden.unwrap_or(0) != 0 {
            continue;
        }
        let title = chapter.title.as_deref().map(str::trim).unwrap_or("Chapter");
        let content = chapter.content.unwrap_or_default();
        if is_empty_html(&content) {
            continue;
        }
        let format = chapter.contentformat.unwrap_or(1);
        let id = chapter.id.unwrap_or(0);
        let display = format!("{} / {}", bookname, title);
        items.push(build_html_item(
            "book_chapter",
            id,
            &display,
            &content,
            format,
        ));
    }
    Ok(())
}

fn add_label_item(
    activity_xml_path: &Path,
    dirname: &str,
    items: &mut Vec<MbzItem>,
) -> Result<(), MbzError> {
    let parsed: LabelActivity = match load_xml(activity_xml_path)? {
        Some(p) => p,
        None => return Ok(()),
    };
    let label = parsed.label;
    let id = label.id.unwrap_or_else(|| suffix_id(dirname).unwrap_or(0));
    let name = label.name.as_deref().map(str::trim).unwrap_or("Label");
    let intro = label.intro.unwrap_or_default();
    if is_empty_html(&intro) {
        return Ok(());
    }
    let format = label.introformat.unwrap_or(1);
    items.push(build_html_item("label", id, name, &intro, format));
    Ok(())
}

fn add_resource_intro(
    activity_xml_path: &Path,
    dirname: &str,
    items: &mut Vec<MbzItem>,
) -> Result<(), MbzError> {
    let parsed: ResourceActivity = match load_xml(activity_xml_path)? {
        Some(p) => p,
        None => return Ok(()),
    };
    let r = parsed.resource;
    let id = r.id.unwrap_or_else(|| suffix_id(dirname).unwrap_or(0));
    let name = r.name.as_deref().map(str::trim).unwrap_or("Resource");
    let intro = r.intro.unwrap_or_default();
    if is_empty_html(&intro) {
        return Ok(());
    }
    let format = r.introformat.unwrap_or(1);
    let display = format!("{} (description)", name);
    items.push(build_html_item(
        "resource_intro",
        id,
        &display,
        &intro,
        format,
    ));
    Ok(())
}

/// Mirror of the plugin's `build_html_item` wrapper: a self-contained HTML
/// document with the title as an `<h1>` above the content.
fn build_html_item(
    type_key: &str,
    _instance_id: i64,
    title: &str,
    content: &str,
    format: i32,
) -> MbzItem {
    let html = normalise_format(content, format);
    let safe_title = html_escape(title);
    let document = format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title></head><body>\n<h1>{title}</h1>\n{body}\n</body></html>\n",
        title = safe_title,
        body = html
    );
    let filename = format!("{}.html", safe_slug(&format!("{}-{}", type_key, title)));
    MbzItem {
        filename,
        mime: "text/html".to_string(),
        body: ItemBody::Inline(document.into_bytes()),
        display: title.to_string(),
    }
}

/// Moodle's `FORMAT_*` codes: 0=MOODLE (legacy wiki), 1=HTML, 2=PLAIN, 4=MARKDOWN.
/// We only support the common cases; anything else is passed through unchanged.
fn normalise_format(content: &str, format: i32) -> String {
    match format {
        // PLAIN: escape then wrap newlines.
        2 => {
            let escaped = html_escape(content);
            escaped.replace('\n', "<br />\n")
        }
        // Everything else (MOODLE, HTML, MARKDOWN) is already close enough
        // to HTML for chunking/extraction. The worker's HTML extractor runs
        // through `scraper` which strips tags safely.
        _ => content.to_string(),
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn is_empty_html(s: &str) -> bool {
    let mut cleaned = String::with_capacity(s.len());
    let mut inside_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            c if !inside_tag => cleaned.push(c),
            _ => {}
        }
    }
    cleaned.trim().is_empty()
}

fn safe_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_us = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    let slug = if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    };
    let mut s = slug;
    if s.chars().count() > 120 {
        s = s.chars().take(120).collect();
    }
    s
}

fn suffix_id(dirname: &str) -> Option<i64> {
    // `<modulename>_<id>` and `section_<id>` both end in the numeric id.
    let tail: String = dirname
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if tail.is_empty() {
        None
    } else {
        tail.parse().ok()
    }
}

fn section_summary_html(section: &SectionXml) -> Option<String> {
    let s = section.summary.as_deref()?;
    if is_empty_html(s) {
        return None;
    }
    Some(s.to_string())
}

fn section_label(section: &SectionXml) -> String {
    if let Some(name) = section.name.as_deref() {
        let t = name.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    format!("Section {}", section.number.unwrap_or(0))
}

// ---------------------------------------------------------------------------
// XML schema structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ModuleXml {
    modulename: String,
    visible: Option<u8>,
    availability: Option<String>,
}

impl ModuleXml {
    fn is_visible(&self, now_ts: i64) -> bool {
        if self.visible.unwrap_or(0) != 1 {
            return false;
        }
        match self.availability.as_deref().map(str::trim) {
            None | Some("") | Some(NULL_AVAILABILITY) => true,
            Some(json) => availability_currently_permissive(json, now_ts),
        }
    }
}

/// Evaluate a Moodle `<availability>` JSON tree and decide whether the
/// restriction is satisfied "right now".
///
/// We only understand date conditions: everything else (group, grouping,
/// grade, completion, profile, role) is per-student, and we have no user
/// context at ingest time. If the tree contains any non-date leaf, or an
/// operator we do not recognise, we fall back to "not permissive" so that
/// potentially-hidden material stays out of RAG.
fn availability_currently_permissive(json: &str, now_ts: i64) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    evaluate_availability(&value, now_ts).unwrap_or(false)
}

fn evaluate_availability(node: &serde_json::Value, now_ts: i64) -> Option<bool> {
    let op = node.get("op")?.as_str()?;
    let conditions = node.get("c")?.as_array()?;

    let mut results: Vec<bool> = Vec::with_capacity(conditions.len());
    for c in conditions {
        let passed = if c.get("op").is_some() {
            // Nested availability tree.
            evaluate_availability(c, now_ts)?
        } else {
            let kind = c.get("type")?.as_str()?;
            if kind != "date" {
                // Non-date leaf: cannot evaluate universally.
                return None;
            }
            let direction = c.get("d")?.as_str()?;
            let ts = c.get("t")?.as_i64()?;
            match direction {
                ">=" => now_ts >= ts,
                "<" => now_ts < ts,
                _ => return None,
            }
        };
        results.push(passed);
    }

    Some(match op {
        "&" => results.iter().all(|&r| r),
        "|" => results.iter().any(|&r| r),
        "!&" => !results.iter().all(|&r| r),
        "!|" => !results.iter().any(|&r| r),
        _ => return None,
    })
}

#[derive(Debug, Deserialize)]
struct UrlActivity {
    url: UrlInner,
}

#[derive(Debug, Deserialize)]
struct UrlInner {
    name: Option<String>,
    externalurl: String,
}

#[derive(Debug, Deserialize)]
struct PageActivity {
    page: PageInner,
}

#[derive(Debug, Deserialize)]
struct PageInner {
    #[serde(rename = "@id")]
    id: Option<i64>,
    name: Option<String>,
    content: Option<String>,
    contentformat: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct BookActivity {
    book: BookInner,
}

#[derive(Debug, Deserialize)]
struct BookInner {
    name: Option<String>,
    chapters: Option<BookChapters>,
}

#[derive(Debug, Deserialize)]
struct BookChapters {
    #[serde(default)]
    chapter: Vec<BookChapter>,
}

#[derive(Debug, Deserialize)]
struct BookChapter {
    #[serde(rename = "@id")]
    id: Option<i64>,
    title: Option<String>,
    content: Option<String>,
    contentformat: Option<i32>,
    hidden: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct LabelActivity {
    label: LabelInner,
}

#[derive(Debug, Deserialize)]
struct LabelInner {
    #[serde(rename = "@id")]
    id: Option<i64>,
    name: Option<String>,
    intro: Option<String>,
    introformat: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ResourceActivity {
    resource: ResourceInner,
}

#[derive(Debug, Deserialize)]
struct ResourceInner {
    #[serde(rename = "@id")]
    id: Option<i64>,
    name: Option<String>,
    intro: Option<String>,
    introformat: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct SectionXml {
    #[serde(rename = "@id")]
    id: Option<i64>,
    number: Option<i32>,
    name: Option<String>,
    summary: Option<String>,
    summaryformat: Option<i32>,
    visible: u8,
}

#[derive(Debug, Deserialize)]
struct FilesXml {
    #[serde(default)]
    file: Vec<FileEntry>,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    contenthash: String,
    contextid: i64,
    component: String,
    filearea: String,
    filename: String,
    filesize: i64,
    #[serde(default)]
    mimetype: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_keeps_alphanumeric_and_utf8() {
        assert_eq!(safe_slug("Föreläsning 1!"), "Föreläsning_1");
        assert_eq!(safe_slug(""), "untitled");
        assert_eq!(safe_slug("..."), "untitled");
    }

    #[test]
    fn empty_html_detects_tags_only() {
        assert!(is_empty_html(""));
        assert!(is_empty_html("<p></p>"));
        assert!(is_empty_html("<p>   </p>"));
        assert!(!is_empty_html("<p>hi</p>"));
    }

    #[test]
    fn module_visibility_requires_flag_and_null_availability() {
        const NOW: i64 = 1_700_000_000;

        let m = ModuleXml {
            modulename: "url".into(),
            visible: Some(1),
            availability: None,
        };
        assert!(m.is_visible(NOW));

        let m = ModuleXml {
            modulename: "url".into(),
            visible: Some(1),
            availability: Some(NULL_AVAILABILITY.into()),
        };
        assert!(m.is_visible(NOW));

        let m = ModuleXml {
            modulename: "url".into(),
            visible: Some(0),
            availability: None,
        };
        assert!(!m.is_visible(NOW));

        // Empty AND tree is "all conditions pass" vacuously, i.e. permissive.
        // Moodle rarely writes this, but being permissive matches the plugin's
        // behavior (`$cm->available` is true when no conditions fail).
        let m = ModuleXml {
            modulename: "url".into(),
            visible: Some(1),
            availability: Some(r#"{"op":"&","c":[]}"#.into()),
        };
        assert!(m.is_visible(NOW));
    }

    #[test]
    fn availability_allows_past_date_and_blocks_future_date() {
        const NOW: i64 = 1_700_000_000;
        // Unlock after a timestamp already passed: permitted.
        let past_unlock = format!(
            r#"{{"op":"&","c":[{{"type":"date","d":">=","t":{}}}]}}"#,
            NOW - 3600
        );
        assert!(availability_currently_permissive(&past_unlock, NOW));

        // Unlock after a future timestamp: blocked.
        let future_unlock = format!(
            r#"{{"op":"&","c":[{{"type":"date","d":">=","t":{}}}]}}"#,
            NOW + 3600
        );
        assert!(!availability_currently_permissive(&future_unlock, NOW));

        // Deadline in the future (< ts): permitted while we are still inside.
        let deadline = format!(
            r#"{{"op":"&","c":[{{"type":"date","d":"<","t":{}}}]}}"#,
            NOW + 3600
        );
        assert!(availability_currently_permissive(&deadline, NOW));

        // Deadline in the past: blocked.
        let expired = format!(
            r#"{{"op":"&","c":[{{"type":"date","d":"<","t":{}}}]}}"#,
            NOW - 3600
        );
        assert!(!availability_currently_permissive(&expired, NOW));

        // A window (AND of open + deadline) we are inside: permitted.
        let window = format!(
            r#"{{"op":"&","c":[
                {{"type":"date","d":">=","t":{}}},
                {{"type":"date","d":"<","t":{}}}
            ]}}"#,
            NOW - 3600,
            NOW + 3600
        );
        assert!(availability_currently_permissive(&window, NOW));
    }

    #[test]
    fn availability_blocks_non_date_conditions() {
        const NOW: i64 = 1_700_000_000;
        // Group restriction: we cannot evaluate universally, so skip.
        let group = r#"{"op":"&","c":[{"type":"group","id":7}]}"#;
        assert!(!availability_currently_permissive(group, NOW));

        // Mixed date + group: the group condition poisons the tree, skip.
        let mixed = format!(
            r#"{{"op":"&","c":[
                {{"type":"date","d":">=","t":{}}},
                {{"type":"group","id":7}}
            ]}}"#,
            NOW - 3600
        );
        assert!(!availability_currently_permissive(&mixed, NOW));

        // Completion, grade, profile: all skip.
        for t in ["completion", "grade", "profile", "role"] {
            let json = format!(r#"{{"op":"&","c":[{{"type":"{}"}}]}}"#, t);
            assert!(!availability_currently_permissive(&json, NOW));
        }
    }

    #[test]
    fn suffix_id_parses_trailing_digits() {
        assert_eq!(suffix_id("section_42"), Some(42));
        assert_eq!(suffix_id("url_1234"), Some(1234));
        assert_eq!(suffix_id("nope"), None);
    }

    /// Build a minimal .mbz covering every interesting visibility case and
    /// assert the parser keeps only what a student would currently see.
    #[test]
    fn import_mbz_filters_hidden_and_keeps_visible_material() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        const NOW: i64 = 1_700_000_000;
        let past = NOW - 3600;
        let future = NOW + 3600;

        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("backup.mbz");

        let pdf_bytes = b"%PDF-1.4 fake pdf contents";
        let pdf_hash = "abcdef1234567890abcdef1234567890abcdef12";

        let files: Vec<(String, Vec<u8>)> = vec![
            ("moodle_backup.xml".into(), b"<moodle_backup/>".to_vec()),
            // Visible URL, no restrictions.
            (
                "activities/url_1/module.xml".into(),
                br#"<module id="1"><modulename>url</modulename><visible>1</visible><availability>$@NULL@$</availability></module>"#.to_vec(),
            ),
            (
                "activities/url_1/url.xml".into(),
                br#"<activity id="1" contextid="100" modulename="url"><url id="1"><name>Course site</name><externalurl>https://example.org</externalurl></url></activity>"#.to_vec(),
            ),
            // Explicitly hidden URL.
            (
                "activities/url_2/module.xml".into(),
                br#"<module id="2"><modulename>url</modulename><visible>0</visible></module>"#.to_vec(),
            ),
            (
                "activities/url_2/url.xml".into(),
                br#"<activity id="2" contextid="101" modulename="url"><url id="2"><name>Secret</name><externalurl>https://secret.example.org</externalurl></url></activity>"#.to_vec(),
            ),
            // Visible page, no restrictions.
            (
                "activities/page_3/module.xml".into(),
                br#"<module id="3"><modulename>page</modulename><visible>1</visible><availability>$@NULL@$</availability></module>"#.to_vec(),
            ),
            (
                "activities/page_3/page.xml".into(),
                br#"<activity id="3" contextid="102" modulename="page"><page id="3"><name>Welcome</name><content>&lt;p&gt;Hi&lt;/p&gt;</content><contentformat>1</contentformat></page></activity>"#.to_vec(),
            ),
            // Visible resource with attached PDF, no restrictions.
            (
                "activities/resource_4/module.xml".into(),
                br#"<module id="4"><modulename>resource</modulename><visible>1</visible><availability>$@NULL@$</availability></module>"#.to_vec(),
            ),
            (
                "activities/resource_4/resource.xml".into(),
                br#"<activity id="4" contextid="103" modulename="resource"><resource id="4"><name>Slides</name><intro>deck</intro><introformat>1</introformat></resource></activity>"#.to_vec(),
            ),
            // Date-gated URL whose unlock time has already passed: KEEP.
            (
                "activities/url_5/module.xml".into(),
                format!(
                    r#"<module id="5"><modulename>url</modulename><visible>1</visible><availability>{{"op":"&amp;","c":[{{"type":"date","d":"&gt;=","t":{past}}}]}}</availability></module>"#,
                    past = past,
                )
                .into_bytes(),
            ),
            (
                "activities/url_5/url.xml".into(),
                br#"<activity id="5" contextid="104" modulename="url"><url id="5"><name>Lecture 1</name><externalurl>https://play.example/l1</externalurl></url></activity>"#.to_vec(),
            ),
            // Date-gated URL whose unlock time is still in the future: SKIP.
            (
                "activities/url_6/module.xml".into(),
                format!(
                    r#"<module id="6"><modulename>url</modulename><visible>1</visible><availability>{{"op":"&amp;","c":[{{"type":"date","d":"&gt;=","t":{future}}}]}}</availability></module>"#,
                    future = future,
                )
                .into_bytes(),
            ),
            (
                "activities/url_6/url.xml".into(),
                br#"<activity id="6" contextid="105" modulename="url"><url id="6"><name>Future lecture</name><externalurl>https://play.example/l2</externalurl></url></activity>"#.to_vec(),
            ),
            // Group-restricted URL: SKIP unconditionally.
            (
                "activities/url_7/module.xml".into(),
                br#"<module id="7"><modulename>url</modulename><visible>1</visible><availability>{"op":"&amp;","c":[{"type":"group","id":9}]}</availability></module>"#.to_vec(),
            ),
            (
                "activities/url_7/url.xml".into(),
                br#"<activity id="7" contextid="106" modulename="url"><url id="7"><name>Group only</name><externalurl>https://play.example/group</externalurl></url></activity>"#.to_vec(),
            ),
            // Sections.
            (
                "sections/section_1/section.xml".into(),
                br#"<section id="1"><number>1</number><name>Intro</name><summary>&lt;p&gt;Kick-off&lt;/p&gt;</summary><summaryformat>1</summaryformat><visible>1</visible></section>"#.to_vec(),
            ),
            (
                "sections/section_2/section.xml".into(),
                br#"<section id="2"><number>2</number><name>Hidden</name><summary>should not appear</summary><summaryformat>1</summaryformat><visible>0</visible></section>"#.to_vec(),
            ),
            // Files inventory + real bytes. The visible resource_4 and also
            // one file whose contextid does not belong to any kept activity
            // (so we verify we filter by visible contexts).
            (
                "files.xml".into(),
                format!(
                    r#"<files>
                        <file id="1">
                          <contenthash>{hash}</contenthash>
                          <contextid>103</contextid>
                          <component>mod_resource</component>
                          <filearea>content</filearea>
                          <filename>slides.pdf</filename>
                          <filesize>{size}</filesize>
                          <mimetype>application/pdf</mimetype>
                        </file>
                        <file id="2">
                          <contenthash>{hash}</contenthash>
                          <contextid>999</contextid>
                          <component>mod_resource</component>
                          <filearea>content</filearea>
                          <filename>hidden.pdf</filename>
                          <filesize>{size}</filesize>
                          <mimetype>application/pdf</mimetype>
                        </file>
                    </files>"#,
                    hash = pdf_hash,
                    size = pdf_bytes.len(),
                )
                .into_bytes(),
            ),
            (
                format!("files/{}/{}", &pdf_hash[..2], pdf_hash),
                pdf_bytes.to_vec(),
            ),
        ];

        let archive_file = fs::File::create(&archive_path).unwrap();
        let gz = GzEncoder::new(archive_file, Compression::fast());
        let mut builder = tar::Builder::new(gz);
        for (name, bytes) in &files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, bytes.as_slice()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();

        let import = import_mbz_at(&archive_path, NOW).unwrap();
        let displays: Vec<&str> = import.items.iter().map(|i| i.display.as_str()).collect();

        assert!(displays.contains(&"Course site (URL)"), "{:?}", displays);
        assert!(displays.contains(&"Welcome"), "{:?}", displays);
        assert!(
            displays.iter().any(|d| d.contains("Slides")),
            "{:?}",
            displays
        );
        assert!(displays.contains(&"slides.pdf"), "{:?}", displays);
        assert!(
            displays.iter().any(|d| d.contains("Intro")),
            "{:?}",
            displays
        );
        // Date-gated with unlock in the past: should be in.
        assert!(
            displays.contains(&"Lecture 1 (URL)"),
            "date-permissive item missing: {:?}",
            displays
        );

        for d in &displays {
            assert!(!d.contains("Secret"));
            assert!(!d.contains("Hidden"));
            assert!(!d.contains("hidden.pdf"));
            assert!(!d.contains("Future lecture"));
            assert!(!d.contains("Group only"));
        }

        // Skipped count: hidden URL, future-date URL, group-only URL,
        // hidden section => 4.
        assert_eq!(import.skipped_hidden, 4);
    }
}
