//! Reading SU's rulebook pages for `crate::rulebook`: the entries of a
//! section index, a page's text and the files it links, and which of those
//! to mirror. Pure functions over the HTML, so the layout assumptions are
//! pinned by tests instead of discovered on the live site.
//!
//! The site is Sitevision. Everything authored sits in `div.pagecontent`;
//! the title (`h1`), breadcrumbs, menus and table of contents are outside
//! it, and "Senast uppdaterad" is in `.su-page-info`. Older versions of a
//! document sit in a `div.sv-collapsible-content` headed "Äldre ...".

use std::collections::HashSet;

use chrono::NaiveDate;
use reqwest::Url;
use scraper::{CaseSensitivity, ElementRef, Html, Node, Selector};

/// Where the rulebook lives on the site. Page links outside it are not
/// rulebook documents.
const RULEBOOK_PATH: &str = "/vart-su/styrning/regelboken/";

/// Category of an index link that comes before any heading.
const NO_CATEGORY: &str = "Övrigt";

/// Share of a text's word 4-grams that must recur elsewhere for the text to
/// count as already carried there. Measured on the education section in
/// November 2026: every page that links its decided PDF had 73-97% of its
/// text in that PDF, and every other file had at most 8% of its text on
/// the page, so the threshold has room on both sides.
pub const COVERED: f64 = 0.7;

/// One entry of a section index.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub url: String,
    pub title: String,
    /// The index heading it is listed under.
    pub category: String,
    /// A file linked straight from the index rather than a page.
    pub is_file: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileLink {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub title: Option<String>,
    /// The page content with Markdown headings and list markers.
    pub body: String,
    pub updated_on: Option<NaiveDate>,
    /// PDFs the page links outside its older versions, in page order.
    pub files: Vec<FileLink>,
}

/// The entries a section index lists, in order, each under the `h2` it
/// follows. `None` when the page has no content area, i.e. the layout
/// changed and nothing should be delisted on its account.
pub fn parse_index(html: &str, index_url: &Url) -> Option<Vec<Entry>> {
    let doc = Html::parse_document(html);
    let content = content_root(&doc)?;
    let mut walk = Walk::new(index_url);
    walk.children(content);
    let mut seen = HashSet::new();
    Some(
        walk.links
            .into_iter()
            .filter_map(|link| {
                let url = Url::parse(&link.url).ok()?;
                let is_file = is_pdf_download(&url, index_url);
                if !is_file && !is_rulebook_page(&url, index_url) {
                    return None;
                }
                if !seen.insert(link.url.clone()) {
                    return None;
                }
                let title = link_title(&link.text, &url);
                Some(Entry {
                    url: link.url,
                    title,
                    category: link.heading.unwrap_or_else(|| NO_CATEGORY.to_string()),
                    is_file,
                })
            })
            .collect(),
    )
}

/// A rulebook page's text and files. `None` when the page has no content
/// area.
pub fn parse_page(html: &str, page_url: &Url) -> Option<Page> {
    let doc = Html::parse_document(html);
    let content = content_root(&doc)?;
    let mut walk = Walk::new(page_url);
    walk.children(content);
    let body = tidy(&walk.out);

    let mut seen = HashSet::new();
    let files = walk
        .links
        .into_iter()
        .filter_map(|link| {
            let url = Url::parse(&link.url).ok()?;
            if !is_pdf_download(&url, page_url) || !seen.insert(link.url.clone()) {
                return None;
            }
            let title = link_title(&link.text, &url);
            Some(FileLink {
                url: link.url,
                title,
            })
        })
        .collect();

    let h1 = Selector::parse("h1").expect("static selector");
    let title = doc
        .select(&h1)
        .next()
        .map(|h| collapse(&inline_text(h)))
        .filter(|t| !t.is_empty());

    Some(Page {
        title,
        body,
        updated_on: updated_on(&doc),
        files,
    })
}

/// The text a page is mirrored as. The header names the source so a
/// retrieved excerpt can be traced back to it.
pub fn render_document(title: &str, category: &str, url: &str, body: &str) -> String {
    format!(
        "# {title}\n\nStockholms universitets regelbok (Regelboken), {category}.\n\
         Källa: {url}\n\n{body}\n"
    )
}

/// What to mirror of one page.
#[derive(Debug, PartialEq)]
pub struct Selection {
    pub mirror_text: bool,
    /// URLs of the files to mirror, in the order given.
    pub files: Vec<String>,
}

/// Decide what to mirror of a page, given its text and its downloaded
/// files' texts. Pages mostly restate the PDF they link, and the PDF is
/// the decided document, footnotes included; mirroring both would fill a
/// chat's excerpts with the same passage twice. So:
///
/// - When the files together carry the page's text, mirror the files only.
/// - Otherwise mirror the page, plus every file it does not already carry.
///
/// A file that failed to download is absent from `files`, so its page is
/// then mirrored instead of losing the text.
pub fn select(page_text: Option<&str>, files: &[(&str, &str)]) -> Selection {
    let file_texts: Vec<&str> = files.iter().map(|(_, text)| *text).collect();
    let page_text = page_text.filter(|t| !t.trim().is_empty());
    let Some(page_text) = page_text else {
        return Selection {
            mirror_text: false,
            files: files.iter().map(|(url, _)| url.to_string()).collect(),
        };
    };
    if !files.is_empty() && coverage(page_text, &file_texts) >= COVERED {
        return Selection {
            mirror_text: false,
            files: files.iter().map(|(url, _)| url.to_string()).collect(),
        };
    }
    Selection {
        mirror_text: true,
        files: files
            .iter()
            .filter(|(_, text)| coverage(text, &[page_text]) < COVERED)
            .map(|(url, _)| url.to_string())
            .collect(),
    }
}

/// Share of `inner`'s word 4-grams that occur in any of `outers`. Words
/// are compared lower-cased, so line breaks, page headers and markup
/// differences between a web page and its PDF only cost the few 4-grams
/// they cut through. 0 when `inner` has fewer than four words.
pub fn coverage(inner: &str, outers: &[&str]) -> f64 {
    let inner_words = words(inner);
    let inner_grams: HashSet<&[String]> = inner_words.windows(4).collect();
    if inner_grams.is_empty() {
        return 0.0;
    }
    let outer_words: Vec<Vec<String>> = outers.iter().map(|t| words(t)).collect();
    let outer_grams: HashSet<&[String]> = outer_words.iter().flat_map(|w| w.windows(4)).collect();
    let shared = inner_grams
        .iter()
        .filter(|g| outer_grams.contains(*g))
        .count();
    shared as f64 / inner_grams.len() as f64
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn content_root(doc: &Html) -> Option<ElementRef<'_>> {
    let sel = Selector::parse("div.pagecontent").expect("static selector");
    doc.select(&sel).next()
}

fn updated_on(doc: &Html) -> Option<NaiveDate> {
    let sel = Selector::parse(".su-page-info p").expect("static selector");
    let line = doc
        .select(&sel)
        .map(|p| p.text().collect::<String>())
        .find(|t| t.contains("Senast uppdaterad"))?;
    let date = regex::Regex::new(r"\d{4}-\d{2}-\d{2}")
        .expect("static regex")
        .find(&line)?;
    NaiveDate::parse_from_str(date.as_str(), "%Y-%m-%d").ok()
}

fn is_pdf_download(url: &Url, base: &Url) -> bool {
    url.host_str() == base.host_str()
        && url.path().starts_with("/download/")
        && url.path().to_ascii_lowercase().ends_with(".pdf")
}

/// A page inside the rulebook, below a section (`/regelboken/{section}`
/// itself is an index, not a document).
fn is_rulebook_page(url: &Url, base: &Url) -> bool {
    url.host_str() == base.host_str()
        && url.path().strip_prefix(RULEBOOK_PATH).is_some_and(|rest| {
            rest.trim_end_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .count()
                >= 2
        })
}

/// A link's text, or the file name in its URL when the text says nothing
/// (an icon, "här").
fn link_title(text: &str, url: &Url) -> String {
    let text = collapse(text);
    if text.chars().filter(|c| c.is_alphanumeric()).count() >= 3 && text != "här" {
        return text;
    }
    let name = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or_default();
    let name = urlencoding::decode(name)
        .map(|n| n.into_owned())
        .unwrap_or_else(|_| name.to_string());
    let stem = name
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or(name);
    collapse(&stem.replace('_', " "))
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Visible text of an element: screen-reader-only text (the "pdf, 98.5
/// kB." after a file link) left out.
fn inline_text(el: ElementRef<'_>) -> String {
    let mut out = String::new();
    for node in el.descendants() {
        if let Node::Text(t) = node.value() {
            let hidden = node.ancestors().any(|a| match a.value() {
                Node::Element(e) => is_assistive(e),
                _ => false,
            });
            if !hidden {
                out.push_str(t);
            }
        }
    }
    out
}

fn is_assistive(e: &scraper::node::Element) -> bool {
    e.has_class("env-assistive-text", CaseSensitivity::CaseSensitive)
}

/// A link found on the walk, with the `h2` it follows.
struct Link {
    url: String,
    text: String,
    heading: Option<String>,
}

/// Depth-first walk of the content area that renders Markdown-ish text and
/// collects links. Older versions are skipped whole, text and links alike.
struct Walk<'u> {
    base: &'u Url,
    out: String,
    links: Vec<Link>,
    heading: Option<String>,
}

impl<'u> Walk<'u> {
    fn new(base: &'u Url) -> Self {
        Self {
            base,
            out: String::new(),
            links: Vec::new(),
            heading: None,
        }
    }

    fn children(&mut self, el: ElementRef<'_>) {
        for child in el.children() {
            match child.value() {
                Node::Text(t) => push_inline(&mut self.out, t),
                Node::Element(e) => {
                    if let Some(el) = ElementRef::wrap(child) {
                        self.element(el, e);
                    }
                }
                _ => {}
            }
        }
    }

    fn element(&mut self, el: ElementRef<'_>, e: &scraper::node::Element) {
        let name = e.name();
        if matches!(
            name,
            "script" | "style" | "noscript" | "svg" | "img" | "button" | "form" | "nav"
        ) || is_assistive(e)
            || e.has_class("sv-custom-module", CaseSensitivity::CaseSensitive)
        {
            return;
        }
        if e.has_class("sv-collapsible-content", CaseSensitivity::CaseSensitive)
            && is_older_versions(el)
        {
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = usize::from(name.as_bytes()[1] - b'0');
                let text = collapse(&inline_text(el));
                if text.is_empty() {
                    return;
                }
                self.collect_links(el);
                block(&mut self.out);
                self.out.push_str(&"#".repeat(level));
                self.out.push(' ');
                self.out.push_str(&text);
                block(&mut self.out);
                if level == 2 {
                    self.heading = Some(text);
                }
            }
            "li" => {
                line(&mut self.out);
                self.out.push_str("- ");
                self.children(el);
                line(&mut self.out);
            }
            "br" => line(&mut self.out),
            "tr" => {
                line(&mut self.out);
                let mut first = true;
                for cell in el.children().filter_map(ElementRef::wrap) {
                    if !matches!(cell.value().name(), "td" | "th") {
                        continue;
                    }
                    // A cell's paragraphs stay on the row's line.
                    let outer = std::mem::take(&mut self.out);
                    self.children(cell);
                    let text = collapse(&std::mem::replace(&mut self.out, outer));
                    if !first {
                        self.out.push_str(" | ");
                    }
                    self.out.push_str(&text);
                    first = false;
                }
                line(&mut self.out);
            }
            "a" => {
                if let Some(href) = e.attr("href") {
                    self.push_link(href, inline_text(el));
                }
                self.children(el);
            }
            "p" | "div" | "section" | "article" | "ul" | "ol" | "table" | "thead" | "tbody"
            | "tfoot" | "blockquote" | "figure" | "dl" | "dt" | "dd" | "header" | "footer" => {
                block(&mut self.out);
                self.children(el);
                block(&mut self.out);
            }
            _ => self.children(el),
        }
    }

    /// Headings are rendered from their text alone; a link inside one is
    /// still a link.
    fn collect_links(&mut self, el: ElementRef<'_>) {
        let a = Selector::parse("a[href]").expect("static selector");
        for link in el.select(&a) {
            if let Some(href) = link.value().attr("href") {
                self.push_link(href, inline_text(link));
            }
        }
    }

    fn push_link(&mut self, href: &str, text: String) {
        let Ok(mut url) = self.base.join(href.trim()) else {
            return;
        };
        if !matches!(url.scheme(), "http" | "https") {
            return;
        }
        url.set_fragment(None);
        self.links.push(Link {
            url: url.to_string(),
            text,
            heading: self.heading.clone(),
        });
    }
}

/// A collapsible section holding superseded versions ("Äldre
/// antagningsordningar", "Tidigare versioner").
fn is_older_versions(section: ElementRef<'_>) -> bool {
    let headings = Selector::parse("h2, h3, h4, h5, h6").expect("static selector");
    section.select(&headings).next().is_some_and(|h| {
        let text = collapse(&inline_text(h)).to_lowercase();
        text.starts_with("äldre") || text.starts_with("tidigare")
    })
}

/// Append a text node: runs of whitespace (the HTML source's own line
/// breaks included) become one space.
fn push_inline(out: &mut String, text: &str) {
    if text.trim().is_empty() {
        if !text.is_empty() && !out.is_empty() && !out.ends_with([' ', '\n']) {
            out.push(' ');
        }
        return;
    }
    if text.starts_with(char::is_whitespace) && !out.is_empty() && !out.ends_with([' ', '\n']) {
        out.push(' ');
    }
    out.push_str(&collapse(text));
    if text.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

fn line(out: &mut String) {
    let trimmed = out.trim_end_matches(' ').len();
    out.truncate(trimmed);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn block(out: &mut String) {
    line(out);
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
}

/// Trim every line, drop list markers left without text, and keep at most
/// one blank line between blocks.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines().map(str::trim) {
        if line == "-" {
            continue;
        }
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    const SECTION: &str = "https://medarbetare.su.se/vart-su/styrning/regelboken/utbildning";

    /// Trimmed from the real index: the menu and breadcrumbs outside the
    /// content area link rulebook pages too, and must not count.
    const INDEX: &str = r##"<html><body>
        <nav><a href="/vart-su/styrning/regelboken/forskning/menu-page">Menu</a></nav>
        <article>
          <header><h1>Utbildning</h1>
            <a href="/vart-su/styrning/regelboken">Regelboken</a></header>
          <div class="pagecontent sv-layout">
            <div class="sv-text-portlet"><div class="sv-text-portlet-content">
              <p class="sv-font-ingress">Här finner du styrdokument som rör utbildning.</p>
            </div></div>
            <div class="sv-text-portlet"><div class="sv-text-portlet-content">
              <h2>Policy</h2>
              <ul><li><a href="/vart-su/styrning/regelboken/utbildning/kvalitetspolicy">Kvalitetspolicy</a></li></ul>
            </div></div>
            <div class="sv-text-portlet"><div class="sv-text-portlet-content">
              <h2>Planer</h2><p>För närvarande finns inga styrdokument inom det här området.</p>
            </div></div>
            <div class="sv-text-portlet"><div class="sv-text-portlet-content">
              <h2>Regler</h2>
              <ul>
                <li><a href="/vart-su/styrning/regelboken/utbildning/regler-och-handlaggningsordning-for-tillgodoraknande">Regler och handläggningsordning för tillgodoräknande</a></li>
                <li><a href="/vart-su/styrning/regelboken/utbildning/kvalitetspolicy">Kvalitetspolicy</a></li>
              </ul>
            </div></div>
            <div class="sv-text-portlet"><div class="sv-text-portlet-content">
              <h2>Stöddokument</h2>
              <p><a href="/download/18.1/1765540803173/Lista_%C3%B6ver_stodformer.pdf">Lista över stödformer<span class="env-assistive-text"> pdf, 246.7 kB.</span></a></p>
              <p><a href="https://su.figshare.com/articles/report/Plagiat/1">Plagiat - hur kan det undvikas?</a></p>
              <p><a href="/vart-su/styrning/regelboken/forskning">Forskning</a></p>
            </div></div>
          </div>
        </article></body></html>"##;

    #[test]
    fn index_lists_rulebook_pages_and_files_under_their_heading() {
        let entries = parse_index(INDEX, &url(SECTION)).unwrap();
        let got: Vec<(&str, &str, bool)> = entries
            .iter()
            .map(|e| (e.title.as_str(), e.category.as_str(), e.is_file))
            .collect();
        assert_eq!(
            got,
            [
                ("Kvalitetspolicy", "Policy", false),
                (
                    "Regler och handläggningsordning för tillgodoräknande",
                    "Regler",
                    false
                ),
                ("Lista över stödformer", "Stöddokument", true),
            ]
        );
        assert_eq!(
            entries[0].url,
            "https://medarbetare.su.se/vart-su/styrning/regelboken/utbildning/kvalitetspolicy"
        );
    }

    #[test]
    fn index_without_content_area_is_not_an_empty_listing() {
        assert!(parse_index("<html><body><h1>Fel</h1></body></html>", &url(SECTION)).is_none());
    }

    const PAGE: &str = r##"<html><body>
        <article>
          <header><h1 class="heading1">Lokal examensordning</h1></header>
          <div class="pagecontent sv-layout"><div id="Mittenspalt"><!-- Mittenspalt --></div>
            <div class="sv-text-portlet"><div class="sv-text-portlet-content">
              <p class="normal">Dnr SU FV-1935-26 2026-06-10.
                Ersätter dokument Lokal examensordning, beslut 2022-12-01.</p>
              <h2>Inledning</h2>
              <p>Vid Stockholms universitet utfärdas examina på
                 grundnivå, <strong>avancerad nivå</strong> och forskarnivå.</p>
              <ul><li>Högskoleexamen</li><li></li><li>Kandidatexamen</li></ul>
              <table><tr><th>Examen</th><th>Omfattning</th></tr>
                     <tr><td><p>Kandidat</p></td><td>180 hp</td></tr></table>
              <p>Se <a href="/download/18.9/111/Reell_kompetens_2013.pdf">Lokala riktlinjer<span class="env-assistive-text"> pdf, 98.5 kB.</span></a> i regelboken.</p>
            </div></div>
            <div class="sv-text-portlet"><div class="sv-text-portlet-content">
              <h2>Dokument i pdf-format</h2>
              <p><a href="/download/18.4/1788340437003/Lokal%20examensordning%20260610.pdf">Lokal examensordning 260610<span class="env-assistive-text"> pdf, 1.2 MB.</span></a></p>
              <p><a href="/download/18.5/1/Mall.docx">Mall<span class="env-assistive-text"> docx</span></a></p>
              <p><a href="/download/18.9/111/Reell_kompetens_2013.pdf">Samma fil igen</a></p>
            </div></div>
            <div class="sv-collapsible-content">
              <h3><a role="button" class="env-collapse-header" href="#svid12_1"><span>Äldre examensordningar</span></a></h3>
              <div class="sv-text-portlet env-collapse"><div class="sv-text-portlet-content">
                <p><a href="/download/18.6/1709818270464/Lokal_examensordning_2022.pdf">Lokal examensordning 221201</a></p>
              </div></div>
            </div>
            <div class="sv-collapsible-content">
              <h3><a class="env-collapse-header" href="#svid12_2"><span>Frågor och svar</span></a></h3>
              <div class="sv-text-portlet env-collapse"><div class="sv-text-portlet-content">
                <p>Svar på vanliga frågor.</p>
              </div></div>
            </div>
          </div>
          <div class="sv-custom-module"><nav>Innehåll på sidan</nav></div>
        </article>
        <footer><div class="su-page-info"><p>Senast uppdaterad<!-- -->: <!-- -->2026-09-08</p>
          <p>Sidansvarig: Studentavdelningen</p></div></footer>
        </body></html>"##;

    fn page_url() -> Url {
        url("https://medarbetare.su.se/vart-su/styrning/regelboken/utbildning/lokal-examensordning")
    }

    #[test]
    fn page_text_keeps_structure_and_drops_chrome() {
        let page = parse_page(PAGE, &page_url()).unwrap();
        assert_eq!(page.title.as_deref(), Some("Lokal examensordning"));
        assert_eq!(page.updated_on, NaiveDate::from_ymd_opt(2026, 9, 8));
        assert_eq!(
            page.body,
            "Dnr SU FV-1935-26 2026-06-10. Ersätter dokument Lokal examensordning, beslut 2022-12-01.\n\
             \n\
             ## Inledning\n\
             \n\
             Vid Stockholms universitet utfärdas examina på grundnivå, avancerad nivå och forskarnivå.\n\
             \n\
             - Högskoleexamen\n\
             - Kandidatexamen\n\
             \n\
             Examen | Omfattning\n\
             Kandidat | 180 hp\n\
             \n\
             Se Lokala riktlinjer i regelboken.\n\
             \n\
             ## Dokument i pdf-format\n\
             \n\
             Lokal examensordning 260610\n\
             \n\
             Mall\n\
             \n\
             Samma fil igen\n\
             \n\
             ### Frågor och svar\n\
             \n\
             Svar på vanliga frågor."
        );
    }

    #[test]
    fn page_files_are_current_pdfs_once_each() {
        let page = parse_page(PAGE, &page_url()).unwrap();
        let got: Vec<(&str, &str)> = page
            .files
            .iter()
            .map(|f| (f.url.as_str(), f.title.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "https://medarbetare.su.se/download/18.9/111/Reell_kompetens_2013.pdf",
                    "Lokala riktlinjer"
                ),
                (
                    "https://medarbetare.su.se/download/18.4/1788340437003/Lokal%20examensordning%20260610.pdf",
                    "Lokal examensordning 260610"
                ),
            ]
        );
    }

    #[test]
    fn a_link_without_words_is_named_after_its_file() {
        let u =
            url("https://medarbetare.su.se/download/18.1/2/Regler_f%C3%B6r_salstentamen_2026.pdf");
        assert_eq!(link_title(" här ", &u), "Regler för salstentamen 2026");
        assert_eq!(
            link_title("Regler för salstentamen", &u),
            "Regler för salstentamen"
        );
    }

    #[test]
    fn coverage_ignores_case_punctuation_and_line_breaks() {
        let page = "Om en student har gått igenom viss högskoleutbildning med godkänt resultat.";
        let pdf = "OM EN STUDENT har gått\nigenom viss högskole-\nutbildning med godkänt resultat";
        // The hyphenated line break cuts the 4-grams that span it.
        let c = coverage(page, &[pdf]);
        assert!(c > 0.4 && c < 1.0, "{c}");
        assert_eq!(coverage(page, &[page]), 1.0);
        assert_eq!(coverage("too short", &[page]), 0.0);
    }

    fn sentence(seed: &str, n: usize) -> String {
        (0..n).map(|i| format!("{seed}{i} ")).collect()
    }

    #[test]
    fn a_page_its_pdf_carries_is_mirrored_as_the_pdf() {
        let rules = sentence("regel", 200);
        let pdf = format!("{rules} fotnot med referenser till beslut");
        let s = select(Some(&rules), &[("a.pdf", &pdf)]);
        assert_eq!(
            s,
            Selection {
                mirror_text: false,
                files: vec!["a.pdf".into()]
            }
        );
    }

    #[test]
    fn a_page_with_its_own_text_keeps_it_and_drops_files_it_repeats() {
        let rules = sentence("regel", 200);
        let handbook = sentence("handbok", 500);
        let excerpt = sentence("regel", 50);
        let s = select(
            Some(&rules),
            &[("handbok.pdf", &handbook), ("utdrag.pdf", &excerpt)],
        );
        assert_eq!(
            s,
            Selection {
                mirror_text: true,
                files: vec!["handbok.pdf".into()]
            }
        );
    }

    #[test]
    fn a_page_without_files_or_text() {
        let rules = sentence("regel", 50);
        assert_eq!(
            select(Some(&rules), &[]),
            Selection {
                mirror_text: true,
                files: vec![]
            }
        );
        // A file entry of the index has no page text of its own.
        assert_eq!(
            select(None, &[("lista.pdf", "")]),
            Selection {
                mirror_text: false,
                files: vec!["lista.pdf".into()]
            }
        );
    }
}
