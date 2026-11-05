-- SU's rulebook (Regelboken) on the staff web, medarbetare.su.se: the
-- governing documents for education, mirrored into credit transfer
-- workspaces next to the DSV course plans (`minerva_app_core::rulebook`).
--
-- Global reference data, like the syllabus archive. A section's index page
-- lists one page per governing document; the page carries the document's
-- text and links its files (the decided PDF, appendices, documents it
-- refers to). Older versions, which a page keeps in an "Äldre ..." section,
-- are not tracked.

CREATE TABLE rulebook_sections (
    url TEXT PRIMARY KEY,
    last_checked_at TIMESTAMPTZ,
    next_check_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    fetch_error TEXT,
    fetch_failures INTEGER NOT NULL DEFAULT 0
);

INSERT INTO rulebook_sections (url)
VALUES ('https://medarbetare.su.se/vart-su/styrning/regelboken/utbildning');

-- One entry of a section index. Usually a page; an index can also link a
-- file directly (`is_file`), which is then its entry's only file.
CREATE TABLE rulebook_pages (
    url TEXT PRIMARY KEY,
    section_url TEXT NOT NULL REFERENCES rulebook_sections(url) ON DELETE CASCADE,
    -- The index heading it is listed under ('Regler', 'Stöddokument', ...)
    -- and its place in the index.
    category TEXT NOT NULL,
    position INTEGER NOT NULL,
    title TEXT NOT NULL,
    is_file BOOLEAN NOT NULL DEFAULT FALSE,
    -- The page's text with Markdown headings. NULL until first fetched, and
    -- always for a file entry.
    body TEXT,
    -- SHA-256 of the document the page is mirrored as: `body` under a
    -- header naming title, category and source
    -- (`rulebook_page::render_document`).
    content_hash TEXT,
    -- "Senast uppdaterad" on the page.
    updated_on DATE,
    content_changed_at TIMESTAMPTZ,
    last_checked_at TIMESTAMPTZ,
    -- Pages send no ETag or Last-Modified, so they are read again daily.
    next_check_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    fetch_error TEXT,
    fetch_failures INTEGER NOT NULL DEFAULT 0,
    -- Whether the page's own text is mirrored. Most pages repeat the PDF
    -- they link (minus its footnotes), and then only the PDF is.
    mirror_text BOOLEAN NOT NULL DEFAULT FALSE,
    -- Set when the text or the page's files change, so what to mirror is
    -- decided again once every listed file has been tried.
    needs_selection BOOLEAN NOT NULL DEFAULT TRUE,
    -- Set when the index no longer lists the page; cleared if it returns.
    delisted_at TIMESTAMPTZ,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_rulebook_pages_due
    ON rulebook_pages (next_check_at)
    WHERE delisted_at IS NULL AND NOT is_file;

-- A file a page links, outside its older versions. Keyed per page: two
-- pages can link the same file, which is then downloaded once.
CREATE TABLE rulebook_files (
    page_url TEXT NOT NULL REFERENCES rulebook_pages(url) ON DELETE CASCADE,
    url TEXT NOT NULL,
    position INTEGER NOT NULL,
    -- The link text, e.g. 'Lokal examensordning 260610'.
    title TEXT NOT NULL,
    -- SHA-256 of the PDF, stored at {docs_path}/rulebook/{content_hash}.pdf.
    content_hash TEXT,
    size_bytes BIGINT,
    -- Text with `[Page N]` markers, what the page's text is compared with.
    text TEXT,
    fetched_at TIMESTAMPTZ,
    -- Download URLs carry the upload time and are served as immutable, so
    -- a downloaded file is never fetched again; this only paces retries.
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    fetch_error TEXT,
    fetch_failures INTEGER NOT NULL DEFAULT 0,
    mirror BOOLEAN NOT NULL DEFAULT FALSE,
    -- Set when the page no longer links the file; cleared if it returns.
    delisted_at TIMESTAMPTZ,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (page_url, url)
);

CREATE INDEX idx_rulebook_files_url ON rulebook_files (url);
