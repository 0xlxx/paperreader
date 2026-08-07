//! Table of Contents extraction.
//!
//! Strategy (in order of preference), aligned with ISO 32000-1 and common
//! PDF-industry practice:
//!
//! 1. **Embedded outlines** (`/Outlines` in the document catalog, ISO 32000-1
//!    §12.3.3) — the exact tree PDF readers render in the sidebar. Parsed with
//!    pdf_oxide's spec-compliant `get_outline()`: PDF text-string decoding
//!    (UTF-16BE/LE BOM, PDFDocEncoding, PDF 2.0 UTF-8), `/Dest` arrays, GoTo
//!    actions, named destinations via `/Dests` dict *and* the `/Names` name
//!    tree, plus page-reference → index resolution with a single tree walk.
//!    Fast and zero false positives.
//!
//! 2. **Printed TOC pages** — layout-aware heuristic. Detects the actual TOC
//!    page(s) in the front matter, then parses entries using text-line
//!    geometry: right-aligned page-number runs, indentation, and font size.
//!    Page numbers are validated/recovered by locating each title in the body
//!    text (via the on-disk index cache when available), which also corrects
//!    the front-matter offset between printed page numbers and physical page
//!    indexes (ISO 32000-1 §12.4.2 Page Labels are often absent).
//!
//! 3. **Heading scan** — last resort for documents without a printed TOC:
//!    scan (sampled) pages for strong chapter/section heading patterns.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use serde::Serialize;
use pdf_oxide::PdfDocument;
use pdf_oxide::outline::Destination;

/// A detected TOC entry.
#[derive(Debug, Clone, Serialize)]
pub struct TocEntry {
    /// Physical 1-indexed page number (usable with --extract-page/--extract-range).
    pub page: usize,
    pub title: String,
    /// Nesting level: 0 = part/unit, 1 = chapter, 2 = section, 3 = subsection.
    pub level: usize,
}

/// Where the TOC entries came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TocSource {
    /// PDF embedded outlines (/Outlines).
    Outlines,
    /// Printed TOC page(s) in the front matter.
    PrintedToc,
    /// Heuristic heading scan fallback.
    HeadingScan,
}

impl std::fmt::Display for TocSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TocSource::Outlines => write!(f, "embedded PDF outlines"),
            TocSource::PrintedToc => write!(f, "printed TOC pages"),
            TocSource::HeadingScan => write!(f, "heading scan"),
        }
    }
}

/// Result of a TOC extraction run.
#[derive(Debug, Clone, Serialize)]
pub struct TocReport {
    pub entries: Vec<TocEntry>,
    pub total_pages: usize,
    pub source: TocSource,
}

/// Internal entry during heuristic parsing: holds both the *printed* page
/// number from the TOC and the resolved *physical* page.
#[derive(Debug, Clone)]
struct RawEntry {
    title: String,
    /// Printed page number as it appears in the TOC (may be offset from physical).
    printed_page: Option<usize>,
    /// Resolved physical page (1-indexed), set by title search / heading scan.
    page: Option<usize>,
    level: usize,
    /// True when the title carries an explicit numbering prefix (第X章, 1.1, …).
    has_numbering: bool,
    /// Left edge of the title run (layout) or indentation proxy (plain text).
    x: f32,
    font_size: f32,
}

/// A layout line extracted from a page, with geometry.
#[derive(Debug, Clone)]
struct RawLine {
    text: String,
    x: f32,
    y: f32,
    width: f32,
    font_size: f32,
    words: Vec<RawWord>,
}

#[derive(Debug, Clone)]
struct RawWord {
    text: String,
    x: f32,
    width: f32,
}

/// Extract the document TOC.
///
/// Opens the PDF exactly once (page-count + outline + heuristic all share the
/// same `PdfDocument`). `force_heuristic` skips the embedded-outline path.
pub fn detect_toc(path: &Path, force_heuristic: bool) -> TocReport {
    let doc = match PdfDocument::open(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  Warning: failed to open PDF: {}", e);
            return TocReport { entries: Vec::new(), total_pages: 0, source: TocSource::HeadingScan };
        }
    };
    let total_pages = doc.page_count().unwrap_or(0);
    if total_pages == 0 {
        eprintln!("  Warning: PDF reports 0 pages (possibly corrupt); no TOC extracted.");
        return TocReport { entries: Vec::new(), total_pages: 0, source: TocSource::HeadingScan };
    }

    if !force_heuristic {
        if let Some(entries) = outlines_from_doc(&doc, total_pages) {
            return TocReport { entries, total_pages, source: TocSource::Outlines };
        }
    }

    let (entries, source) = heuristic_toc(&doc, path, total_pages);
    TocReport { entries, total_pages, source }
}

// =========================================================================
// 1. Embedded outlines (ISO 32000-1 §12.3.3)
// =========================================================================

/// Flatten pdf_oxide's hierarchical outline into 1-indexed `TocEntry`s.
///
/// Returns `None` when the outline is absent, trivial, or mostly unresolvable —
/// a production-artifact outline (e.g. a handful of cover markers) is worse
/// than the printed-TOC heuristic, so we fall through in that case.
fn outlines_from_doc(doc: &PdfDocument, total_pages: usize) -> Option<Vec<TocEntry>> {
    let items = doc.get_outline().ok()??;
    let mut entries = Vec::new();
    flatten_outline(&items, 0, &mut entries, total_pages);
    if entries.len() < 4 {
        return None;
    }
    let resolved = entries.iter().filter(|e| e.page > 0).count();
    if resolved * 2 < entries.len() {
        return None; // more than half the bookmarks point nowhere
    }
    let unique = entries.iter().map(|e| normalize_title(&e.title)).collect::<HashSet<_>>();
    if unique.len() < 2 {
        return None; // artifact bookmarks (repeated cover markers)
    }
    Some(entries)
}

fn flatten_outline(
    items: &[pdf_oxide::outline::OutlineItem],
    level: usize,
    out: &mut Vec<TocEntry>,
    total_pages: usize,
) {
    for item in items {
        if !item.title.trim().is_empty() {
            let page = match &item.dest {
                Some(Destination::PageIndex(i)) if *i < total_pages => Some(*i + 1),
                _ => None, // out-of-range or unresolved (Named) destinations
            };
            if let Some(page) = page {
                out.push(TocEntry {
                    page,
                    title: item.title.trim().to_string(),
                    level: level.min(3),
                });
            }
        }
        flatten_outline(&item.children, level + 1, out, total_pages);
    }
}

// =========================================================================
// 2. Heuristic: printed TOC pages
// =========================================================================

/// Try the printed-TOC heuristics; fall back to a heading scan.
fn heuristic_toc(doc: &PdfDocument, path: &Path, total_pages: usize) -> (Vec<TocEntry>, TocSource) {
    let cache = load_page_cache(path, total_pages);

    // Fast path: plain-text scan. Page text (from the on-disk index cache, or a
    // single cheap extraction per page) is enough to find and parse a
    // well-formed printed TOC: dot leaders, right-aligned page numbers and
    // wrapped-title continuations all survive in plain text. Avoids the
    // expensive layout extraction (`extract_text_lines`, which computes word
    // geometry) for the ~20 front-matter pages — this is the difference between
    // ~0s and ~3s on a typical textbook.
    let plain = plain_scan(doc, path, total_pages, cache.as_ref());
    if plain_is_acceptable(&plain) {
        let mut entries = plain.entries;
        let offset = resolve_pages(&mut entries, doc, path, &plain.pages, total_pages, cache.as_ref());
        let finalized = finalize(entries, offset);
        if finalized.len() >= 3 {
            eprintln!(
                "  TOC: detected printed TOC pages (plain text){} — {} entries",
                if cache.is_some() { " from index cache" } else { "" },
                finalized.len()
            );
            return (finalized, TocSource::PrintedToc);
        }
    }

    // Robust path: layout-aware scan. Text-line geometry (right-aligned page
    // runs, indentation, font size) stays correct where plain text misleads:
    // multi-column TOC grids (地理), broken ToUnicode CMaps that interleave page
    // numbers into titles (数学一年级), or mojibake page numbers (数学七年级).
    // Only pages near a plain-text 目录/Contents marker are layout-extracted,
    // which bounds the cost to a handful of pages instead of the whole front
    // matter.
    let toc = find_toc_pages(doc, total_pages, Some(&plain.markers));
    if !toc.pages.is_empty() {
        let mut entries = parse_toc_pages(&toc, total_pages);
        if entries.len() >= 3 {
            let offset = resolve_pages(&mut entries, doc, path, &toc.pages, total_pages, cache.as_ref());
            let finalized = finalize(entries, offset);
            if finalized.len() >= 3 {
                eprintln!(
                    "  TOC: detected printed TOC on page(s) {}{} — {} entries",
                    toc.pages.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(","),
                    if cache.is_some() { " (index cache)" } else { "" },
                    finalized.len()
                );
                return (finalized, TocSource::PrintedToc);
            }
        }
    }

    eprintln!("  TOC: no printed TOC found; falling back to heading scan");
    let entries = heading_scan(doc, path, total_pages, cache.as_ref());
    (finalize_entries(entries), TocSource::HeadingScan)
}

/// Quality gate for the fast plain-text path.
///
/// Plain text is trusted only when it yields a solid number of entries with
/// readable text AND the page numbers have not been interleaved into the titles
/// — the signature of a broken ToUnicode CMap (e.g. "396~10的认识和加减法5",
/// "认识钟表847" in 数学一年级上册). Such garbage is rejected so the layout
/// path can recover the real structure.
fn plain_is_acceptable(plain: &PlainScan) -> bool {
    if plain.entries.len() < 4 {
        return false;
    }
    let readable = plain.entries.iter().filter(|e| title_has_text(&e.title)).count();
    if readable * 2 < plain.entries.len() {
        return false;
    }
    // A title that still contains digits after its numbering prefix is
    // stripped is a garble signal (broken ToUnicode CMap interleaves page
    // numbers into titles, e.g. "396~10的认识和加减法5" in 数学一年级上册).
    // Legitimate numeric titles such as "1 负数" / "2 百分数（二）" strip
    // cleanly to "负数" / "百分数（二）", so ANY remaining digit means the
    // plain text is unreliable — fall through to the layout path, which can
    // recover the true structure from word geometry.
    let garbled = plain
        .entries
        .iter()
        .filter(|e| strip_numbering_prefix(&e.title).chars().any(|c| c.is_ascii_digit()))
        .count();
    garbled == 0
}

/// Load every page's extracted text from the on-disk index cache, once.
fn load_page_cache(path: &Path, total_pages: usize) -> Option<Vec<Option<String>>> {
    let meta = crate::index::read_valid_meta(path)?;
    let hash_dir = crate::index::get_index_dir(path);
    let mut texts = vec![None; total_pages];
    let n = total_pages.min(meta.pages);
    for page in 1..=n {
        let file = hash_dir.join(format!("page_{:04}.txt", page));
        texts[page - 1] = match std::fs::read_to_string(&file) {
            Ok(s) if !s.trim().is_empty() => Some(s),
            _ => None,
        };
    }
    Some(texts)
}

/// Page text from cache, else a one-shot extraction on the shared document.
fn page_text(
    cache: Option<&Vec<Option<String>>>,
    doc: &PdfDocument,
    _path: &Path,
    page: usize,
) -> Option<String> {
    if let Some(c) = cache {
        return c.get(page - 1).cloned().flatten();
    }
    let t = crate::pdf::safe_extract_text(doc, page - 1);
    if t.trim().is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Page text with all whitespace removed — used for fuzzy title matching that
/// tolerates line-wrapped headings ("…统一多民族\n封建国家…" == "…统一多民族封建国家…").
/// Remove all whitespace — used for fuzzy title matching that tolerates
/// line-wrapped headings ("…统一多民族\n封建国家…" == "…统一多民族封建国家…").
fn compact_text(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    for ch in t.chars() {
        if !ch.is_whitespace() {
            s.push(ch);
        }
    }
    s
}

// -------------------------------------------------------------------------
// 2a. Plain-text scan (fast path)
// -------------------------------------------------------------------------

struct PlainScan {
    entries: Vec<RawEntry>,
    pages: Vec<usize>,
    /// Pages carrying the 目录/Contents marker (even when `entries` was
    /// rejected) — used to seed the layout scan so it only extracts geometry
    /// for pages near the marker instead of the whole front matter.
    markers: Vec<usize>,
}

/// Scan the first pages of plain text for a printed TOC.
///
/// Two passes: first score each page (a "TOC page" has the 目录/Contents marker
/// or at least two page-numbered entries), then collect entries from exactly
/// those pages. This keeps body pages — which can contain stray trailing
/// numbers or heading lines — out of the parsed TOC and out of the "skip"
/// range used by page resolution.
fn plain_scan(
    doc: &PdfDocument,
    path: &Path,
    total_pages: usize,
    cache: Option<&Vec<Option<String>>>,
) -> PlainScan {
    let max_scan = total_pages.min(20);
    let mut scored: Vec<(usize, PlainPage)> = Vec::new();
    // For unindexed documents each page costs a full pdf_oxide extraction
    // (~100ms), so once a marker page with a solid number of entries is found
    // (i.e. a real printed TOC), only the next two pages can be TOC
    // continuation — stop there instead of extracting the whole front matter.
    // Indexed scans read the on-disk cache and stay full-scan (cheap + exact).
    let mut stop_after: Option<usize> = None;
    for page in 1..=max_scan {
        if let Some(limit) = stop_after {
            if page > limit {
                break;
            }
        }
        let Some(text) = page_text(cache, doc, path, page) else { continue };
        let parsed = plain_parse_page(&text);
        if cache.is_none() && stop_after.is_none() {
            let numbered = parsed.entries.iter().filter(|e| e.printed_page.is_some()).count();
            if parsed.marker && numbered >= 3 {
                stop_after = Some((page + 2).min(max_scan));
            }
        }
        scored.push((page, parsed));
    }
    let marker_pages: Vec<usize> = scored
        .iter()
        .filter(|(_, p)| p.marker)
        .map(|(page, _)| *page)
        .collect();
    let toc_pages: Vec<usize> = scored
        .iter()
        .filter(|(page, p)| {
            let numbered = p.entries.iter().filter(|e| e.printed_page.is_some()).count();
            // A TOC page must be anchored to the 目录/Contents marker: the marker
            // page itself, or a continuation page within ±2 (multi-page TOCs).
            // Bare `numbered >= 4` is NOT enough — math-textbook body pages are
            // full of lines ending in digits and would flood the TOC with
            // body sentences (数学六年级下册 page 8+).
            numbered >= 3
                && (p.marker || marker_pages.iter().any(|&m| m.abs_diff(*page) <= 2))
        })
        .map(|(page, _)| *page)
        .collect();
    if toc_pages.is_empty() {
        return PlainScan { entries: Vec::new(), pages: Vec::new(), markers: marker_pages };
    }

    let mut entries: Vec<RawEntry> = Vec::new();
    for (page, parsed) in &scored {
        if !toc_pages.contains(page) {
            continue;
        }
        for e in parsed.entries.iter() {
            if entry_is_junk(&e.title) {
                continue;
            }
            if e.printed_page.map_or(true, |p| p >= 1 && p <= total_pages.saturating_add(20)) {
                entries.push(e.clone());
            }
        }
    }

    // A body page in the front matter can rarely produce 1-2 stray entries;
    // require either the explicit marker or a solid number of entries.
    let marker_seen = toc_pages.iter().any(|&p| {
        scored.iter().find(|(page, _)| *page == p).map(|(_, s)| s.marker).unwrap_or(false)
    });
    if entries.len() >= 4 && (marker_seen || entries.len() >= 6) {
        PlainScan { entries, pages: toc_pages, markers: marker_pages }
    } else {
        PlainScan { entries: Vec::new(), pages: Vec::new(), markers: marker_pages }
    }
}

struct PlainPage {
    marker: bool,
    entries: Vec<RawEntry>,
}

/// Parse one page of plain text into TOC entries (dot leaders, trailing page
/// numbers, wrapped-title continuation merging).
///
/// Continuations are detected structurally rather than by indentation alone:
/// many publishers wrap long CJK titles flush-left, so any page-less line that
/// follows a numbered entry still awaiting its page number is treated as a
/// continuation.
fn plain_parse_page(text: &str) -> PlainPage {
    let mut marker = false;
    let mut entries: Vec<RawEntry> = Vec::new();
    let mut last_has_numbering = false;

    for line in text.lines() {
        let raw = line.trim_end();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_toc_marker(trimmed) {
            marker = true;
            continue;
        }
        let indented = raw.starts_with(char::is_whitespace) && raw.len() > raw.trim_start().len();

        if let Some((title, printed)) = plain_line_page(raw) {
            let title = clean_title(&normalize_title_spacing(&title));
            if title.chars().count() < 2 || title.chars().count() > 60 {
                continue;
            }
            let level = level_from_title(&title);
            if let Some(level) = level {
                entries.push(RawEntry {
                    title,
                    printed_page: Some(printed),
                    page: None,
                    level,
                    has_numbering: true,
                    x: if indented { 30.0 } else { 0.0 },
                    font_size: 0.0,
                });
                last_has_numbering = true;
            } else if !entries.is_empty()
                && entries.last().unwrap().printed_page.is_none()
                && entries.last().unwrap().page.is_none()
                && last_has_numbering
            {
                // Wrapped continuation of the previous entry, carrying its page.
                let last = entries.last_mut().unwrap();
                append_title(&mut last.title, &title);
                last.printed_page = Some(printed);
            } else {
                let level = level_for_unnumbered(&title, 0.0, 0.0);
                entries.push(RawEntry {
                    title,
                    printed_page: Some(printed),
                    page: None,
                    level,
                    has_numbering: false,
                    x: if indented { 30.0 } else { 0.0 },
                    font_size: 0.0,
                });
                last_has_numbering = false;
            }
        } else if !entries.is_empty()
            && entries.last().unwrap().printed_page.is_none()
            && entries.last().unwrap().page.is_none()
            && last_has_numbering
        {
            // Continuation without a page number (e.g. a wrapped unit title).
            let last = entries.last_mut().unwrap();
            append_title(&mut last.title, trimmed);
        } else if !indented {
            // A heading-like line without a page number (e.g. a unit title whose
            // page number is unreadable, or 附录 whose page follows on the next
            // line): keep it; resolution happens later.
            if let Some((title, level)) = try_chapter_heading(trimmed) {
                entries.push(RawEntry {
                    title,
                    printed_page: None,
                    page: None,
                    level,
                    has_numbering: true,
                    x: 0.0,
                    font_size: 0.0,
                });
                last_has_numbering = true;
            } else if let Some(level) = level_from_keywords(trimmed) {
                entries.push(RawEntry {
                    title: trimmed.to_string(),
                    printed_page: None,
                    page: None,
                    level,
                    has_numbering: true,
                    x: 0.0,
                    font_size: 0.0,
                });
                last_has_numbering = true;
            }
        } else if let Some(multi) = split_multi_entries(raw) {
            // One physical line carries several "title page" pairs separated by
            // dot leaders — extract_text joins same-baseline words from grid /
            // two-column TOCs (e.g. 地理七年级上册: "….第一节 地球和地球仪2
            // ….第二节 地球的运动11 …").
            for (mtitle, mpage) in multi {
                let title = clean_title(&normalize_title_spacing(&mtitle));
                if title.chars().count() < 2 || title.chars().count() > 60 {
                    continue;
                }
                let numbered = level_from_title(&title).is_some();
                let level = numbered
                    .then(|| level_from_title(&title).unwrap())
                    .unwrap_or_else(|| level_for_unnumbered(&title, 0.0, 0.0));
                entries.push(RawEntry {
                    title,
                    printed_page: Some(mpage),
                    page: None,
                    level,
                    has_numbering: numbered,
                    x: if indented { 30.0 } else { 0.0 },
                    font_size: 0.0,
                });
                last_has_numbering = true;
            }
        }
    }

    PlainPage { marker, entries }
}

/// Split a line into several (title, page) pairs when it contains multiple
/// dot-leader-separated entries on one baseline.
///
/// Grid-style TOCs place several "title + page number" groups on a line and
/// separate them with dot leaders; pdf_oxide's plain `extract_text` joins the
/// words into a single line. Each group is `title` immediately followed by its
/// page digits, and groups are delimited by runs of 3+ dots.
fn split_multi_entries(line: &str) -> Option<Vec<(String, usize)>> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut dot_run = 0usize;
    let mut saw_boundary = false;
    for c in line.chars() {
        if c == '.' || c == '·' || c == '…' {
            dot_run += 1;
            continue;
        }
        if dot_run >= 3 {
            chunks.push(std::mem::take(&mut current));
            saw_boundary = true;
        }
        dot_run = 0;
        current.push(c);
    }
    if dot_run >= 3 {
        chunks.push(std::mem::take(&mut current));
        saw_boundary = true;
    } else if !current.trim().is_empty() {
        chunks.push(current);
    }
    // Only dot-leader-separated multi-entry lines qualify; a single group with
    // trailing digits is a normal entry handled by plain_line_page.
    if !saw_boundary {
        return None;
    }

    let mut out = Vec::new();
    for chunk in chunks {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        // Trailing digits are the printed page; the rest is the title.
        let mut split = chunk.len();
        for (idx, c) in chunk.char_indices().rev() {
            if c.is_ascii_digit() || matches!(c, '０'..='９') {
                split = idx;
            } else {
                break;
            }
        }
        if split == chunk.len() {
            continue; // no page digits → not an entry group
        }
        let (title, digits) = chunk.split_at(split);
        let title = title.trim().trim_end_matches(['.', '·', '…']).trim();
        if title.chars().count() < 2 || !title_has_text(title) {
            continue;
        }
        let Some(page) = parse_page_text(digits) else { continue };
        if page == 0 {
            continue;
        }
        out.push((title.to_string(), page));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// True when `s` contains a run of 3+ dot-leader characters ('.', '·', '…')
/// after its leading dots — i.e. the line joins several "title page" groups.
fn has_internal_dot_leader(s: &str) -> bool {
    let mut seen_non_dot = false;
    let mut run = 0usize;
    for c in s.chars() {
        if c == '.' || c == '·' || c == '…' {
            run += 1;
        } else {
            if seen_non_dot && run >= 3 {
                return true;
            }
            seen_non_dot = true;
            run = 0;
        }
    }
    seen_non_dot && run >= 3
}

/// Parse one plain-text line into (title, printed page). Handles dot leaders
/// ("Introduction...........45"), spaced numbers ("Section Title  42"),
/// numbers attached directly to the title ("第1课 标题2"), and page numbers
/// extracted *before* the title ("28第6课 标题.....").
fn plain_line_page(line: &str) -> Option<(String, usize)> {
    let t = line.trim_end();
    if let Some((title, page)) = split_leading_page(t) {
        return Some((title, page));
    }
    if let Some((title, page)) = split_dot_leader(t) {
        return Some((title, page));
    }
    // Last whitespace-separated token is a number. Refuse when the "title"
    // itself still contains dot leaders — that means the line holds several
    // "title page" groups (grid TOCs), and split_multi_entries handles it.
    if let Some(idx) = t.rfind(char::is_whitespace) {
        let last = t[idx..].trim();
        if is_numeric_token(last) {
            let title = t[..idx].trim();
            if !title.is_empty() && !has_internal_dot_leader(title) {
                if let Some(page) = parse_page_text(last) {
                    return Some((title.to_string(), page));
                }
            }
        }
    }
    // Number attached directly to the end ("…早期国家2").
    let mut digits_start = None;
    for (idx, c) in t.char_indices().rev() {
        if c.is_ascii_digit() || matches!(c, '０'..='９') {
            continue;
        }
        digits_start = Some(idx + c.len_utf8());
        break;
    }
    let Some(ds) = digits_start else { return None };
    let (title, digits) = t.split_at(ds);
    if digits.is_empty() || title.trim().is_empty() {
        return None;
    }
    if let Some(page) = parse_page_text(digits) {
        let title = title.trim_end().trim();
        // A "title" that still contains dot leaders joins several "title page"
        // groups (grid TOCs) — leave it for split_multi_entries.
        if title.chars().count() >= 2 && !has_internal_dot_leader(title) {
            return Some((title.to_string(), page));
        }
    }
    None
}

/// "75第6课 标题....." — the page number was extracted before the title.
fn split_leading_page(line: &str) -> Option<(String, usize)> {
    let t = line.trim();
    let digit_len = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_len == 0 || digit_len > 4 {
        return None;
    }
    let rest = t[digit_len..].trim_start();
    let lower = rest.to_lowercase();
    let heading_start = rest.starts_with('第')
        || lower.starts_with("chapter ")
        || lower.starts_with("appendix ")
        || lower.starts_with("part ");
    if !heading_start {
        return None;
    }
    let (title, _) = split_on_dot_leader(rest)?;
    let page = parse_page_text(&t[..digit_len])?;
    if page > 0 {
        Some((title, page))
    } else {
        None
    }
}

/// Split "text .... rest" at the dot-leader run → (title, rest after dots).
fn split_on_dot_leader(line: &str) -> Option<(String, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut run_start = None;
    let mut run_len = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if c == '.' || c == '·' || c == '…' {
            if run_start.is_none() {
                run_start = Some(i);
            }
            run_len += 1;
        } else if run_len >= 3 {
            break;
        } else {
            run_start = None;
            run_len = 0;
        }
    }
    let start = run_start?;
    if run_len < 3 {
        return None;
    }
    let title: String = chars[..start].iter().collect();
    let title = title
        .trim()
        .trim_end_matches(['.', '·', '…', ':'])
        .trim()
        .to_string();
    let after: String = chars[start + run_len..].iter().collect();
    Some((title, after))
}

/// Dot-leader split with a trailing page number: "text .... 42" → ("text", 42).
fn split_dot_leader(line: &str) -> Option<(String, usize)> {
    let (title, after) = split_on_dot_leader(line)?;
    let page_str = after.split_whitespace().next().unwrap_or("");
    let page = parse_page_text(page_str)?;
    if !title.is_empty() && page > 0 {
        Some((title, page))
    } else {
        None
    }
}

// -------------------------------------------------------------------------
// 2b. Layout-aware scan (robust path for CJK / garbled encodings)
// -------------------------------------------------------------------------

/// A detected printed-TOC page block, including the layout lines extracted
/// from each page (so parsing does not re-extract geometry).
struct TocPages {
    pages: Vec<usize>,
    lines: HashMap<usize, Vec<RawLine>>,
}

/// Score the first pages and return those that look like a printed TOC.
///
/// A page qualifies on its own when it carries the 目录/Contents marker with a
/// few entries, or has many gap-separated right-aligned page runs. Pages next
/// to a marker page with at least a few runs are added too — multi-page TOCs
/// (especially two-column ones) often have fewer runs on continuation pages.
fn find_toc_pages(doc: &PdfDocument, total_pages: usize, marker_hint: Option<&[usize]>) -> TocPages {
    let max_scan = total_pages.min(20);
    // Only pages near a plain-text 目录/Contents marker can be printed-TOC
    // pages; extracting layout geometry for the whole front matter is the main
    // cost of the heuristic path (~150ms/page). Without a marker hint (marker
    // glyphs themselves garbled) fall back to scanning all front-matter pages.
    let candidates: Vec<usize> = match marker_hint {
        Some(markers) if !markers.is_empty() => (1..=max_scan)
            .filter(|&page| markers.iter().any(|&m| m.abs_diff(page) <= 2))
            .collect(),
        _ => (1..=max_scan).collect(),
    };
    let mut info: Vec<(usize, bool, bool, usize)> = Vec::new(); // (page, marker, is_toc, runs)
    let mut lines: HashMap<usize, Vec<RawLine>> = HashMap::new();
    for page in candidates {
        let page_lines = raw_lines_from_page(doc, page);
        if page_lines.is_empty() {
            continue;
        }
        let page_width = page_lines.iter().map(|l| l.x + l.width).fold(0.0f32, f32::max);
        let (marker, is_toc) = toc_page_score(&page_lines, page_width);
        let runs = page_lines.iter().filter_map(|l| split_page_run(l, page_width)).count();
        lines.insert(page, page_lines);
        info.push((page, marker, is_toc, runs));
    }
    let marker_pages: Vec<usize> = info.iter().filter(|(_, m, _, _)| *m).map(|(p, _, _, _)| *p).collect();
    if marker_pages.is_empty() {
        return TocPages { pages: Vec::new(), lines };
    }
    // The marker page seeds the block; only pages near it (or the marker page
    // itself) qualify. This keeps body pages with right-aligned formula numbers
    // or running headers out of the TOC.
    let mut pages: Vec<usize> = info
        .iter()
        .filter(|(page, _, _, runs)| {
            *runs >= 3 && marker_pages.iter().any(|&m| m.abs_diff(*page) <= 2)
        })
        .map(|(p, _, _, _)| *p)
        .collect();
    pages.sort_unstable();
    pages.dedup();
    TocPages { pages, lines }
}

/// TOC-page classifier: a page is a printed-TOC page when it carries the
/// 目录/Contents marker with a few entries, or when it has many
/// gap-separated right-aligned page runs (the hallmark of a TOC column).
/// Body pages with a running header/footer contribute at most one such run.
fn toc_page_score(lines: &[RawLine], page_width: f32) -> (bool, bool) {
    let mut has_marker = false;
    let mut right_runs = 0usize;
    let mut entry_like = 0usize;
    for l in lines {
        let t = l.text.trim();
        if t.is_empty() {
            continue;
        }
        if is_toc_marker(t) {
            has_marker = true;
            continue;
        }
        if let Some((title, _page_text, is_right)) = split_page_run(l, page_width) {
            if (2..=80).contains(&title.chars().count()) {
                entry_like += 1;
                if is_right {
                    right_runs += 1;
                }
            }
        }
    }
    let is_toc = (has_marker && entry_like >= 3) || right_runs >= 6;
    (has_marker, is_toc)
}

/// Convert pdf_oxide TextLines to RawLines, merging same-baseline fragments
/// (extractors often split a title and its right-aligned page number).
fn raw_lines_from_page(doc: &PdfDocument, page_num: usize) -> Vec<RawLine> {
    let lines = match doc.extract_text_lines(page_num - 1) {
        Ok(l) => l,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        let mut fs = 0.0f32;
        let mut n = 0usize;
        let mut words = Vec::with_capacity(l.words.len());
        for w in &l.words {
            let text = w.text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            fs += w.avg_font_size;
            n += 1;
            words.push(RawWord {
                text,
                x: w.bbox.x,
                width: w.bbox.width,
            });
        }
        if words.is_empty() {
            continue;
        }
        let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        out.push(RawLine {
            text,
            x: l.bbox.x,
            y: l.bbox.y,
            width: l.bbox.width,
            font_size: fs / n as f32,
            words,
        });
    }
    merge_same_baseline(out)
}

fn merge_same_baseline(mut lines: Vec<RawLine>) -> Vec<RawLine> {
    // pdf_oxide reports line y in top-origin coordinates (larger y = higher on
    // the page), so sorting descending yields top-to-bottom reading order —
    // essential for continuation merging ("的建立与巩固" must follow its
    // "第一单元 …" parent line).
    lines.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut out: Vec<RawLine> = Vec::new();
    for l in lines {
        if let Some(last) = out.last_mut() {
            if (l.y - last.y).abs() <= 3.0 {
                // Fragments on one baseline (title + right-aligned page run) may
                // arrive with slightly different y values; re-sort by x so the
                // page run is always the final word.
                last.words.extend(l.words);
                last.words.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
                last.x = last.words.first().map(|w| w.x).unwrap_or(0.0);
                let right = last.words.iter().map(|w| w.x + w.width).fold(0.0f32, f32::max);
                last.width = right - last.x;
                last.text = last.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                last.font_size = last.font_size.max(l.font_size);
                continue;
            }
        }
        out.push(l);
    }
    out
}

/// Split a layout line into (title, page-run text, is_right_aligned).
///
/// A right-aligned page run is a word separated from the title by a large
/// typographic gap (a right-justified column), not merely the last word of a
/// justified body line.
fn split_page_run(line: &RawLine, page_width: f32) -> Option<(String, String, bool)> {
    let words = &line.words;
    if words.len() < 2 {
        return None;
    }
    let last = words.last()?;
    let prev = &words[words.len() - 2];
    let last_text = last.text.trim();
    if last_text.is_empty() {
        return None;
    }
    let numeric = is_numeric_token(last_text);
    let gap = last.x - (prev.x + prev.width);
    let right_aligned = gap > 25.0 && last.x > page_width * 0.5;
    if !numeric && !right_aligned {
        return None;
    }
    let title = words[..words.len() - 1]
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some((title, last_text.to_string(), right_aligned))
}

/// Parse the detected TOC pages into raw entries.
fn parse_toc_pages(toc: &TocPages, total_pages: usize) -> Vec<RawEntry> {
    let mut entries: Vec<RawEntry> = Vec::new();
    for &page in &toc.pages {
        let Some(lines) = toc.lines.get(&page) else { continue };
        if lines.is_empty() {
            continue;
        }
        let page_width = lines.iter().map(|l| l.x + l.width).fold(0.0f32, f32::max);
        let med = median_font(&entries);
        for l in lines {
            let t = l.text.trim();
            if t.is_empty() || is_toc_marker(t) {
                continue;
            }
            let (title_raw, page_text, _is_right) = match split_page_run(l, page_width) {
                Some(x) => x,
                None => {
                    // No page run on this line.
                    let cleaned = clean_title(&normalize_title_spacing(t));
                    if cleaned.chars().count() < 2 {
                        continue;
                    }
                    if let Some(level) = level_from_title(&cleaned) {
                        // Numbered heading whose page number sits on the next
                        // (indented, wrapped) line — e.g. long CJK unit titles.
                        entries.push(RawEntry {
                            title: cleaned,
                            printed_page: None,
                            page: None,
                            level,
                            has_numbering: true,
                            x: l.x,
                            font_size: l.font_size,
                        });
                    } else if let Some(last) = entries.last_mut() {
                        if last.page.is_none()
                            && last.printed_page.is_none()
                            && last.has_numbering
                            && l.x > last.x + 10.0
                        {
                            append_title(&mut last.title, &cleaned);
                        }
                    }
                    continue;
                }
            };
            let title = clean_title(&normalize_title_spacing(&title_raw));
            if title.chars().count() < 2 {
                continue;
            }
            let printed = parse_page_text(&page_text);
            if let Some(level) = level_from_title(&title) {
                entries.push(RawEntry {
                    title,
                    printed_page: printed,
                    page: None,
                    level,
                    has_numbering: true,
                    x: l.x,
                    font_size: l.font_size,
                });
            } else if let Some(last) = entries.last_mut() {
                // Unnumbered line with a page run: usually the wrapped
                // continuation of the previous numbered entry.
                if last.page.is_none()
                    && last.printed_page.is_none()
                    && last.has_numbering
                    && l.x > last.x + 10.0
                {
                    append_title(&mut last.title, &title);
                    last.printed_page = printed.or(last.printed_page);
                } else {
                    let level = level_for_unnumbered(&title, l.font_size, med);
                    entries.push(RawEntry {
                        title,
                        printed_page: printed,
                        page: None,
                        level,
                        has_numbering: false,
                        x: l.x,
                        font_size: l.font_size,
                    });
                }
            } else {
                let level = level_for_unnumbered(&title, l.font_size, med);
                entries.push(RawEntry {
                    title,
                    printed_page: printed,
                    page: None,
                    level,
                    has_numbering: false,
                    x: l.x,
                    font_size: l.font_size,
                });
            }
        }
    }
    // Sanity: drop junk and entries pointing outside the document.
    entries.retain(|e| !entry_is_junk(&e.title));
    entries.retain(|e| e.printed_page.map_or(true, |p| p <= total_pages.saturating_add(20)));
    entries
}

fn median_font(entries: &[RawEntry]) -> f32 {
    let mut fs: Vec<f32> = entries.iter().map(|e| e.font_size).filter(|f| *f > 0.0).collect();
    if fs.is_empty() {
        return 0.0;
    }
    fs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    fs[fs.len() / 2]
}

// -------------------------------------------------------------------------
// 2c. Page resolution
// -------------------------------------------------------------------------

/// Shared page-text accessor for page resolution: prefers the on-disk index
/// cache; otherwise extracts each page at most once and keeps it in memory.
struct PageTexts<'a> {
    doc: &'a PdfDocument,
    cache: Option<&'a Vec<Option<String>>>,
    local: HashMap<usize, String>,
}

impl<'a> PageTexts<'a> {
    fn new(doc: &'a PdfDocument, cache: Option<&'a Vec<Option<String>>>) -> Self {
        PageTexts { doc, cache, local: HashMap::new() }
    }

    /// Whitespace-free text for a page, extracting once if not cached.
    fn compact(&mut self, page: usize) -> Option<String> {
        if let Some(c) = self.cache {
            return c.get(page - 1).cloned().flatten().map(|t| compact_text(&t));
        }
        if let Some(t) = self.local.get(&page) {
            return Some(t.clone());
        }
        let t = crate::pdf::safe_extract_text(self.doc, page - 1);
        if t.trim().is_empty() {
            return None;
        }
        let c = compact_text(&t);
        self.local.insert(page, c.clone());
        Some(c)
    }

    /// Pages extracted so far (unindexed case), in ascending order.
    fn extracted_pages(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.local.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

/// Resolve physical pages for TOC entries.
///
/// Title search (whitespace-insensitive, against the index cache when present)
/// locates each title in the body, which also yields the median
/// (physical − printed) offset across the document. Printed page numbers are
/// then re-based with that offset — they are authoritative modulo the
/// front-matter offset — while entries without a readable printed page keep
/// their search result. Returns the offset for `finalize` to apply to any
/// entries that were not searchable.
fn resolve_pages(
    entries: &mut [RawEntry],
    doc: &PdfDocument,
    path: &Path,
    toc_pages: &[usize],
    total_pages: usize,
    cache: Option<&Vec<Option<String>>>,
) -> Option<usize> {
    let last_toc = toc_pages.iter().copied().max().unwrap_or(0);

    // Resolve every searchable entry: printed pages are offset from physical
    // pages by the front matter, and a single document scan (fast via the index
    // cache) both finds true heading pages and derives that offset.
    let mut keys: Vec<Option<String>> = entries
        .iter()
        .map(|e| search_key(&e.title))
        .collect();

    if keys.iter().any(|k| k.is_some()) {
        let scan_pages = body_pages_to_scan(cache, total_pages, last_toc);

        // Page text is expensive without an index cache, so extract each page
        // at most once across all passes. Also, once a few (physical, printed)
        // pairs have been found the median front-matter offset is derivable,
        // and printed + offset resolves every entry that carries a printed
        // page — so the full-body scan can stop early.
        let mut texts = PageTexts::new(doc, cache);

        // Pass 1: exact (whitespace-insensitive) title match.
        let need_offset_pairs = if cache.is_some() { usize::MAX } else { 4 };
        let mut offset_pairs = 0usize;
        for page in &scan_pages {
            let Some(text) = texts.compact(*page) else { continue };
            for (i, k) in keys.iter_mut().enumerate() {
                if let Some(key) = k {
                    if text.contains(key.as_str()) {
                        entries[i].page = Some(*page);
                        *k = None;
                        if entries[i].printed_page.is_some() {
                            offset_pairs += 1;
                        }
                    }
                }
            }
            if keys.iter().all(|k| k.is_none()) || offset_pairs >= need_offset_pairs {
                break;
            }
        }
        // Pass 2: shorter distinctive prefix (tolerates punctuation drift),
        // over pages already extracted (no new extraction when unindexed).
        let mut still: Vec<(usize, String)> = keys
            .iter()
            .enumerate()
            .filter_map(|(i, k)| k.clone().map(|k| (i, prefix_key(&k))))
            .collect();
        if !still.is_empty() {
            let pages: Vec<usize> = if cache.is_some() {
                body_pages_to_scan(cache, total_pages, last_toc)
            } else {
                texts.extracted_pages()
            };
            for page in pages {
                let Some(text) = texts.compact(page) else { continue };
                for (i, pk) in still.iter_mut() {
                    if !pk.is_empty() && text.contains(pk.as_str()) {
                        entries[*i].page = Some(page);
                        pk.clear();
                    }
                }
                if still.iter().all(|(_, k)| k.is_empty()) {
                    break;
                }
            }
            for (i, pk) in still {
                if pk.is_empty() {
                    keys[i] = None;
                }
            }
        }
        // Pass 3: heading-scan map for whatever remains — including entries
        // whose search key was too short/generic to search reliably.
        if entries.iter().any(|e| e.page.is_none()) {
            let map = heading_pages_after(doc, path, total_pages, last_toc, cache);
            for e in entries.iter_mut() {
                if e.page.is_none() {
                    if let Some(p) = map.get(&normalize_title(&e.title)) {
                        e.page = Some(*p);
                    }
                }
            }
        }
    }

    let offset = median_offset(entries);
    // Printed page numbers in the TOC are authoritative modulo a constant
    // front-matter offset (ISO 32000-1 §12.4.2 Page Labels are frequently
    // absent). Title search can land on an incidental mention (e.g. a lesson
    // name quoted in body text), so once we know the offset we prefer
    // printed + offset for every entry that has a printed page, and keep the
    // search result only for entries without one (e.g. garbled ToUnicode maps).
    if let Some(o) = offset {
        for e in entries.iter_mut() {
            if e.printed_page.is_some() {
                e.page = Some(e.printed_page.unwrap() + o);
            }
        }
    }
    offset
}

/// Pages to scan when resolving titles/headings.
///
/// With an index cache every page is a cheap disk read, so scan everything.
/// Without one, each page costs a full pdf_oxide extraction, so sample: the 40
/// pages right after the TOC densely (textbook entries cluster there, keeping
/// the derived front-matter offset exact), then a stride over the remainder.
fn body_pages_to_scan(
    cache: Option<&Vec<Option<String>>>,
    total_pages: usize,
    after: usize,
) -> Vec<usize> {
    if cache.is_some() {
        (after + 1..=total_pages).collect()
    } else {
        let dense_end = total_pages.min(after + 40);
        let mut v: Vec<usize> = (after + 1..=dense_end).collect();
        let stride = (total_pages / 40).max(5);
        v.extend((dense_end + 1..=total_pages).step_by(stride));
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// Median (physical - printed) offset across entries that have both.
fn median_offset(entries: &[RawEntry]) -> Option<usize> {
    let mut offsets: Vec<i64> = entries
        .iter()
        .filter_map(|e| match (e.page, e.printed_page) {
            (Some(p), Some(pr)) => Some(p as i64 - pr as i64),
            _ => None,
        })
        .collect();
    if offsets.len() < 2 {
        return None;
    }
    offsets.sort_unstable();
    let mid = offsets[offsets.len() / 2];
    Some(mid.max(0) as usize)
}

/// Build a search key from a title: strip numbering/garbage prefixes.
fn search_key(title: &str) -> Option<String> {
    let t = strip_numbering_prefix(title);
    let t = strip_garbage_prefix(&t);
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return None;
    }
    if is_generic_label(&t) {
        return None;
    }
    // Short keys (e.g. "有理数", "附录") match incidental mentions across the
    // body; let the heading map or the printed+offset path handle them instead.
    if t.chars().count() < 4 {
        return None;
    }
    if !t.chars().any(|c| c.is_alphabetic() || is_cjk_char(c)) {
        return None;
    }
    Some(t)
}

fn prefix_key(key: &str) -> String {
    let cjk = key.chars().filter(|&c| is_cjk_char(c)).count();
    if cjk * 2 >= key.chars().count() {
        key.chars().take(10).collect()
    } else {
        key.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
    }
}

/// True when a parsed title is extraction noise: no CJK and no ASCII letters
/// (pure digits / garbled glyphs from broken ToUnicode CMaps), or copyright /
/// colophon metadata lines.
fn entry_is_junk(title: &str) -> bool {
    if !title_has_text(title) {
        return true;
    }
    const COLOPHON: &[&str] = &[
        "ISBN", "书号", "印 张", "印张", "开 本", "开本", "定价", "责任编辑", "出版",
        "网 址", "版权所有", "毫米", "版次", "印次", "字数", "图 书在版编目", "绿色印刷",
    ];
    COLOPHON.iter().any(|k| title.contains(k))
}

/// Does the title carry readable text (CJK or ASCII letters)?
fn title_has_text(title: &str) -> bool {
    title.chars().any(|c| is_cjk_char(c) || c.is_ascii_alphabetic())
}

fn is_generic_label(t: &str) -> bool {
    const GENERIC: &[&str] = &[
        "小结", "复习题", "练习", "数学活动", "活动课", "阅读与思考", "观察与猜想", "实验与探究",
        "信息技术应用", "探究与发现", "思考", "目录", "contents", "index", "glossary",
        "acknowledgments", "acknowledgements",
    ];
    GENERIC.iter().any(|g| *g == t)
}

// -------------------------------------------------------------------------
// 3. Heading scan fallback
// -------------------------------------------------------------------------

/// Scan pages for strong heading patterns when there is no printed TOC.
fn heading_scan(
    doc: &PdfDocument,
    path: &Path,
    total_pages: usize,
    cache: Option<&Vec<Option<String>>>,
) -> Vec<TocEntry> {
    let pages: Vec<usize> = if cache.is_some() {
        (1..=total_pages).collect()
    } else {
        let mut v: Vec<usize> = (1..=total_pages).step_by((total_pages / 40).max(5)).collect();
        v.extend(1..=total_pages.min(20));
        v.sort_unstable();
        v.dedup();
        v
    };
    let mut entries = Vec::new();
    for &page in &pages {
        if let Some(text) = page_text(cache, doc, path, page) {
            entries.extend(extract_headings(&text, page));
        }
    }
    finalize_entries(entries)
}

/// Heading pages after a given page, keyed by normalized title → first page.
fn heading_pages_after(
    doc: &PdfDocument,
    path: &Path,
    total_pages: usize,
    after: usize,
    cache: Option<&Vec<Option<String>>>,
) -> HashMap<String, usize> {
    let pages = body_pages_to_scan(cache, total_pages, after);
    let mut map = HashMap::new();
    for &page in &pages {
        if let Some(text) = page_text(cache, doc, path, page) {
            for h in extract_headings(&text, page) {
                map.entry(normalize_title(&h.title)).or_insert(h.page);
            }
        }
    }
    map
}

/// Extract chapter/section headings from a page's plain text.
fn extract_headings(text: &str, page_num: usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.chars().count() > 120 {
            continue;
        }
        if let Some((title, level)) = try_chapter_heading(line) {
            entries.push(TocEntry {
                page: page_num,
                title,
                level,
            });
        }
    }
    entries
}

// -------------------------------------------------------------------------
// Final assembly
// -------------------------------------------------------------------------

fn finalize_entries(entries: Vec<TocEntry>) -> Vec<TocEntry> {
    let mut seen = HashSet::new();
    let mut out: Vec<TocEntry> = entries.into_iter().filter(|e| e.page >= 1).collect();
    out.retain(|e| seen.insert(normalize_title(&e.title)));
    out.sort_by_key(|e| (e.page, e.level));
    out
}

fn finalize(entries: Vec<RawEntry>, offset: Option<usize>) -> Vec<TocEntry> {
    let mut out = Vec::new();
    for e in entries {
        // Prefer the resolved physical page; otherwise fall back to the printed
        // page re-based by the median offset, or to the printed page itself
        // when no offset could be derived (e.g. unindexed documents).
        let page = e.page.or_else(|| e.printed_page.map(|p| offset.map_or(p, |o| p + o)));
        if let Some(p) = page {
            if p >= 1 {
                out.push(TocEntry {
                    page: p,
                    title: e.title,
                    level: e.level.min(3),
                });
            }
        }
    }
    let mut seen = HashSet::new();
    out.retain(|e| seen.insert(normalize_title(&e.title)));
    out.sort_by_key(|e| (e.page, e.level));
    out
}

/// Append a wrapped continuation to a title.
///
/// A space is inserted between Latin words, and after standalone labels such as
/// "第一单元"/"附录" whose continuation is a full title on the next line. No
/// space is inserted mid-phrase ("…封建国家的建立与巩固"), which is how CJK
/// wrapped headings read in print.
fn append_title(dst: &mut String, src: &str) {
    let need_space = dst
        .chars()
        .last()
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
        && src
            .chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false);
    let ends_with_label = dst.ends_with(|c: char| c.is_whitespace())
        || ["单元", "章", "课", "节", "讲", "篇", "部分", "专题", "模块", "附录", "索引", "后记"]
            .iter()
            .any(|u| dst.ends_with(u));
    if need_space || ends_with_label {
        dst.push(' ');
    }
    dst.push_str(src);
}

// -------------------------------------------------------------------------
// Title/level helpers (shared by plain, layout and heading-scan paths)
// -------------------------------------------------------------------------

/// Level from an explicit numbering/keyword prefix, if any.
fn level_from_title(title: &str) -> Option<usize> {
    if let Some((_, lvl)) = match_cjk_heading(title) {
        return Some(lvl);
    }
    if let Some((_, lvl)) = match_chapter(title) {
        return Some(lvl);
    }
    if let Some((_, lvl)) = match_part(title) {
        return Some(lvl);
    }
    if let Some((_, lvl)) = match_numbered_chapter(title) {
        return Some(lvl);
    }
    if let Some((_, lvl)) = match_numbered_section(title) {
        return Some(lvl);
    }
    if let Some((_, lvl)) = match_allcaps_heading(title) {
        return Some(lvl);
    }
    None
}

/// Level for an unnumbered entry: keyword mapping first, then font-size
/// relative to the page's median (larger → higher level).
fn level_for_unnumbered(title: &str, font_size: f32, median: f32) -> usize {
    if let Some(kw) = level_from_keywords(title) {
        return kw;
    }
    if median > 0.0 && font_size >= median * 1.15 {
        1
    } else {
        2
    }
}

fn level_from_keywords(title: &str) -> Option<usize> {
    let upper = title.to_uppercase();
    const CHAPTER_ISH: &[&str] = &[
        "INTRODUCTION", "CONCLUSION", "REFERENCES", "BIBLIOGRAPHY", "APPENDIX", "PREFACE",
        "ACKNOWLEDGMENTS", "ACKNOWLEDGEMENTS", "INDEX", "GLOSSARY", "ABSTRACT", "SUMMARY",
        "FOREWORD", "目录", "附录", "参考文献", "后记",
    ];
    if CHAPTER_ISH.iter().any(|w| upper.contains(w)) {
        return Some(1);
    }
    const SECTION_ISH: &[&str] = &["小结", "复习题", "练习", "数学活动", "活动课"];
    if SECTION_ISH.iter().any(|w| title.contains(w)) {
        return Some(2);
    }
    None
}

/// Strong heading patterns only. No broad heuristics.
fn try_chapter_heading(line: &str) -> Option<(String, usize)> {
    if let Some(r) = match_cjk_heading(line) {
        return Some(r);
    }
    if let Some(r) = match_chapter(line) {
        return Some(r);
    }
    if let Some(r) = match_numbered_chapter(line) {
        return Some(r);
    }
    if let Some(r) = match_numbered_section(line) {
        return Some(r);
    }
    if let Some(r) = match_part(line) {
        return Some(r);
    }
    if let Some(r) = match_allcaps_heading(line) {
        return Some(r);
    }
    None
}

fn match_chapter(line: &str) -> Option<(String, usize)> {
    let lower = line.to_lowercase();
    if !lower.starts_with("chapter ") {
        return None;
    }
    let rest = line[8..].trim();
    if rest.is_empty() {
        return None;
    }
    for sep in [':', '-', '.'] {
        if let Some((num_str, title_suffix)) = rest.split_once(sep) {
            if num_str.trim().parse::<usize>().is_ok() {
                let title = format!("Chapter {}: {}", num_str.trim(), title_suffix.trim());
                return Some((title, 1));
            }
        }
    }
    if rest.parse::<usize>().is_ok() {
        return Some((format!("Chapter {}", rest), 1));
    }
    None
}

/// Chinese numbered headings: 第X单元/部分/篇 → 0, 第X章/专题/讲/模块 → 1,
/// 第X课/节 → 2. Handles Arabic and Chinese numerals.
fn match_cjk_heading(line: &str) -> Option<(String, usize)> {
    let s = line.strip_prefix('第')?;
    let num_chars: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '０'..='９') || "一二三四五六七八九十百零〇两".contains(*c))
        .collect();
    if num_chars.is_empty() {
        return None;
    }
    let rest = &s[num_chars.len()..];
    for (unit, level) in [
        ("单元", 0usize),
        ("部分", 0),
        ("篇", 0),
        ("章", 1),
        ("专题", 1),
        ("讲", 1),
        ("模块", 1),
        ("课", 2),
        ("节", 2),
    ] {
        if let Some(after) = rest.strip_prefix(unit) {
            let after = after.trim();
            let title = if after.is_empty() {
                line.trim().to_string()
            } else {
                format!("第{}{} {}", num_chars, unit, after)
            };
            return Some((title, level));
        }
    }
    None
}

/// "N. Title" (single number + period) → chapter (level 1).
fn match_numbered_chapter(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i >= chars.len() || chars[i] != '.' {
        return None;
    }
    if i + 1 >= chars.len() || !chars[i + 1].is_whitespace() {
        return None;
    }
    let num: usize = chars[..i].iter().collect::<String>().parse().ok()?;
    if num > 50 {
        return None;
    }
    let title: String = chars[i + 1..].iter().collect();
    let title = title.trim().to_string();
    let word_count = title.split_whitespace().count();
    if title.len() < 3 || title.len() > 80 || word_count > 8 || word_count < 2 {
        return None;
    }
    let first_char = title.chars().next().unwrap_or(' ');
    if !first_char.is_ascii_uppercase() {
        return None;
    }
    if title.contains("/*") || title.contains("*/") || title.contains('{') || title.contains('}') {
        return None;
    }
    if !title.chars().any(|c| c.is_alphabetic() || is_cjk_char(c)) {
        return None;
    }
    let lowercase_words: usize = title
        .split_whitespace()
        .filter(|w| w.starts_with(|c: char| c.is_ascii_lowercase()))
        .count();
    if lowercase_words > 2 {
        return None;
    }
    let lower = title.to_lowercase();
    for body_word in &["might", "should", "will", "can ", "may ", "does ", "has ", "the target"] {
        if lower.contains(body_word) && word_count > 4 {
            return None;
        }
    }
    Some((format!("{} {}", num, title), 1))
}

/// "N.N Title" → level 2, "N.N.N Title" → level 3.
fn match_numbered_section(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
        i += 1;
    }
    if i < 3 || !chars[..i].iter().collect::<String>().contains('.') {
        return None;
    }
    if i >= chars.len() || !chars[i].is_whitespace() {
        return None;
    }
    let num: String = chars[..i].iter().collect();
    let title: String = chars[i..].iter().collect();
    let title = title.trim().to_string();
    let word_count = title.split_whitespace().count();
    if word_count > 12 || title.ends_with('.') || title.len() < 2 {
        return None;
    }
    if title.contains("/*") || title.contains("*/") || title.contains('{') || title.contains('}') {
        return None;
    }
    // Reject numeric data lines ("4.8 7.5") — a heading must contain real text.
    if !title.chars().any(|c| c.is_alphabetic() || is_cjk_char(c)) {
        return None;
    }
    let level = (num.matches('.').count() + 1).min(3);
    Some((format!("{} {}", num, title), level))
}

fn match_part(line: &str) -> Option<(String, usize)> {
    let lower = line.to_lowercase();
    if !lower.starts_with("part ") {
        return None;
    }
    let rest = line[5..].trim();
    let first_char = rest.chars().next()?;
    if !first_char.is_ascii_digit() && !"IVXLCDMivxlcdm".contains(first_char) {
        return None;
    }
    for sep in [':', '-', '.'] {
        if let Some((num_str, title_suffix)) = rest.split_once(sep) {
            let title = format!("Part {}: {}", num_str.trim(), title_suffix.trim());
            return Some((title, 0));
        }
    }
    Some((format!("Part {}", rest), 0))
}

fn match_allcaps_heading(line: &str) -> Option<(String, usize)> {
    if line.chars().count() < 4 || line.chars().count() > 60 {
        return None;
    }
    let alpha_count = line.chars().filter(|c| c.is_ascii_uppercase()).count();
    let total = line.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 || (alpha_count as f64 / total as f64) < 0.8 {
        return None;
    }
    let word_count = line.split_whitespace().count();
    if word_count > 5 {
        return None;
    }
    let upper = line.to_uppercase();
    const HEADING_WORDS: &[&str] = &[
        "INTRODUCTION", "CONCLUSION", "REFERENCES", "BIBLIOGRAPHY", "APPENDIX", "PREFACE",
        "ACKNOWLEDGMENTS", "INDEX", "GLOSSARY", "ABSTRACT", "SUMMARY", "FOREWORD", "CONTENTS",
    ];
    if HEADING_WORDS.iter().any(|&w| upper.contains(w)) {
        return Some((line.to_string(), 1));
    }
    None
}

/// Strip a leading numbering prefix ("第3课", "第一章", "Chapter 4:", "1.2.3",
/// "Part II") from a title, leaving the searchable remainder.
fn strip_numbering_prefix(title: &str) -> String {
    let t = title.trim();
    if let Some(rest) = cjk_heading_rest(t) {
        let rest = rest.trim();
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    let lower = t.to_lowercase();
    for kw in ["chapter ", "part ", "appendix ", "unit ", "lesson ", "section ", "module "] {
        if let Some(rest) = lower.strip_prefix(kw) {
            let prefix_len = lower.len() - rest.len();
            let rest = &t[prefix_len..];
            let rest = rest
                .trim_start_matches(|c: char| c.is_ascii_digit() || "ivxlcdmIVXLCDM ".contains(c))
                .trim_start_matches([':', '-', '.', ' ', '\t']);
            let rest = rest.trim();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    let mut i = 0;
    for (idx, c) in t.char_indices() {
        if c.is_ascii_digit() || c == '.' {
            i = idx + c.len_utf8();
        } else {
            break;
        }
    }
    if i > 0 {
        let rest = t[i..].trim_start();
        if !rest.is_empty() && !rest.starts_with(|c: char| c.is_ascii_digit()) {
            return rest.to_string();
        }
    }
    t.to_string()
}

fn cjk_heading_rest(t: &str) -> Option<&str> {
    let s = t.strip_prefix('第')?;
    let num_len = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '０'..='９') || "一二三四五六七八九十百零〇两".contains(*c))
        .map(|c| c.len_utf8())
        .sum::<usize>();
    if num_len == 0 {
        return None;
    }
    let rest = &s[num_len..];
    for unit in ["单元", "部分", "篇", "章", "专题", "讲", "模块", "课", "节"] {
        if let Some(after) = rest.strip_prefix(unit) {
            let after = after.trim();
            if after.is_empty() {
                return None;
            }
            return Some(after);
        }
    }
    None
}

/// Strip leading tokens that carry no CJK/ASCII-alphanumeric content (garbled
/// glyphs from broken ToUnicode CMaps, bullet markers, …).
fn strip_garbage_prefix(t: &str) -> String {
    let mut words: Vec<&str> = t.split_whitespace().collect();
    while words.len() > 1 {
        let first = words[0];
        if !first.is_empty() && !first.chars().any(|c| c.is_ascii_alphanumeric() || is_cjk_char(c)) {
            words.remove(0);
        } else {
            break;
        }
    }
    words.join(" ")
}

/// Normalize spacing in extracted titles: "第 1 课 标题" → "第1课 标题".
fn normalize_title_spacing(title: &str) -> String {
    let tokens: Vec<&str> = title.split_whitespace().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if t == "第" && i + 1 < tokens.len() && is_num_token(tokens[i + 1]) {
            out.push_str(t);
            out.push_str(tokens[i + 1]);
            i += 2;
            if i < tokens.len() && is_unit_word(tokens[i]) {
                out.push_str(tokens[i]);
                i += 1;
            }
        } else {
            out.push_str(t);
            i += 1;
        }
        if i < tokens.len() {
            out.push(' ');
        }
    }
    out
}

fn is_num_token(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit() || matches!(c, '０'..='９') || "一二三四五六七八九十百零〇两".contains(c))
}

fn is_unit_word(s: &str) -> bool {
    matches!(
        s,
        "课" | "章" | "单元" | "节" | "讲" | "篇" | "部分" | "专题" | "模块" | "框"
    )
}

/// Clean a raw title: trim punctuation, drop leading dot-leader runs and
/// garbage tokens.
fn clean_title(t: &str) -> String {
    let t = t
        .trim()
        .trim_start_matches(['.', '·', '…'])
        .trim()
        .trim_end_matches(['.', '·', '…', ':', '，', ','])
        .trim();
    strip_garbage_prefix(t)
}

/// Lowercased, whitespace-collapsed title used for dedup.
fn normalize_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn is_cjk_char(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
        | '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{AC00}'..='\u{D7AF}'
        | '\u{3000}'..='\u{303F}' | '\u{FF00}'..='\u{FFEF}')
}

fn is_numeric_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '０'..='９'))
}

fn parse_page_text(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.is_empty() || !is_numeric_token(s) {
        return None;
    }
    let mut n = 0usize;
    for c in s.chars() {
        let d = if c.is_ascii_digit() {
            c as u32 - '0' as u32
        } else {
            c as u32 - '０' as u32
        };
        n = n.checked_mul(10)?.checked_add(d as usize)?;
    }
    Some(n)
}

fn is_toc_marker(t: &str) -> bool {
    let t = t.trim();
    let upper = t.to_uppercase();
    t.contains("目录")
        || t.contains("目 录")
        || t.contains("目次")
        || upper == "CONTENTS"
        || upper.contains("TABLE OF CONTENTS")
        || upper == "CONTENT"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cjk_heading_levels() {
        assert_eq!(match_cjk_heading("第一单元 从中华文明起源到秦汉").map(|(_, l)| l), Some(0));
        assert_eq!(match_cjk_heading("第一章 有理数").map(|(_, l)| l), Some(1));
        assert_eq!(match_cjk_heading("第1课 中华文明的起源与早期国家").map(|(_, l)| l), Some(2));
        assert_eq!(match_cjk_heading("第三节 一元一次方程").map(|(_, l)| l), Some(2));
        assert_eq!(match_cjk_heading("第十单元").map(|(_, l)| l), Some(0));
        assert!(match_cjk_heading("义务教育教科书").is_none());
    }

    #[test]
    fn test_cjk_chapter() {
        let result = match_cjk_heading("第一章 理论基础").unwrap();
        assert_eq!(result.0, "第一章 理论基础");
        assert_eq!(result.1, 1);
    }

    #[test]
    fn test_chapter_with_title() {
        let result = match_chapter("Chapter 4: System Design and Execution Models").unwrap();
        assert_eq!(result.0, "Chapter 4: System Design and Execution Models");
        assert_eq!(result.1, 1);
    }

    #[test]
    fn test_numbered_section_level() {
        assert_eq!(match_numbered_section("1.1 Origins of ECS Architecture").map(|(_, l)| l), Some(2));
        assert_eq!(match_numbered_section("2.3.1 Detailed Analysis of Patterns").map(|(_, l)| l), Some(3));
    }

    #[test]
    fn test_numbered_section_rejects_numeric_data() {
        assert!(match_numbered_section("4.8 7.5").is_none());
        assert!(match_numbered_section("91.3 88.7 88.8").is_none());
    }

    #[test]
    fn test_body_text_rejected() {
        assert!(try_chapter_heading("If the Velocity component is removed, the entity moves to archetype C").is_none());
        assert!(try_chapter_heading("Additionally, we explore strategies for partitioning entities").is_none());
        assert!(try_chapter_heading("The principal taxonomy of ECS patterns").is_none());
    }

    #[test]
    fn test_allcaps_heading() {
        assert_eq!(match_allcaps_heading("INTRODUCTION").map(|(_, l)| l), Some(1));
        assert!(match_allcaps_heading("RANDOM TEXT THAT IS ALL CAPS").is_none());
    }

    #[test]
    fn test_plain_line_page() {
        assert_eq!(plain_line_page("Introduction...........45"), Some(("Introduction".into(), 45)));
        assert_eq!(plain_line_page("1.1 Section Title............123"), Some(("1.1 Section Title".into(), 123)));
        assert_eq!(plain_line_page("  的建立与巩固 \t 1"), Some(("的建立与巩固".into(), 1)));
        assert_eq!(
            plain_line_page("第1课 中华文明的起源与早期国家2"),
            Some(("第1课 中华文明的起源与早期国家".into(), 2))
        );
        assert!(plain_line_page("第一单元 从中华文明起源到秦汉统一多民族封建国家").is_none());
    }

    #[test]
    fn test_plain_page_continuation_merge() {
        let text = "目 录\n第一单元 从中华文明起源到秦汉统一多民族封建国家\n  的建立与巩固 \t 1\n第1课 中华文明的起源与早期国家2\n第2课 诸侯纷争与变法运动9";
        let page = plain_parse_page(text);
        assert!(page.marker);
        assert_eq!(page.entries.len(), 3);
        assert_eq!(page.entries[0].title, "第一单元 从中华文明起源到秦汉统一多民族封建国家的建立与巩固");
        assert_eq!(page.entries[0].printed_page, Some(1));
        assert_eq!(page.entries[0].level, 0);
        assert_eq!(page.entries[1].title, "第1课 中华文明的起源与早期国家");
        assert_eq!(page.entries[1].printed_page, Some(2));
        assert_eq!(page.entries[1].level, 2);
        assert_eq!(page.entries[2].title, "第2课 诸侯纷争与变法运动");
        assert_eq!(page.entries[2].printed_page, Some(9));
    }

    #[test]
    fn test_normalize_title_spacing() {
        assert_eq!(normalize_title_spacing("第 1 课 中华文明的起源与早期国家"), "第1课 中华文明的起源与早期国家");
        assert_eq!(normalize_title_spacing("第 一 章 有理数"), "第一章 有理数");
        assert_eq!(normalize_title_spacing("第一章 有理数"), "第一章 有理数");
    }

    #[test]
    fn test_strip_numbering_prefix() {
        assert_eq!(strip_numbering_prefix("第1课 中华文明的起源与早期国家"), "中华文明的起源与早期国家");
        assert_eq!(strip_numbering_prefix("第一章 有理数"), "有理数");
        assert_eq!(strip_numbering_prefix("Chapter 4: System Design"), "System Design");
        assert_eq!(strip_numbering_prefix("1.1 What Is a Game?"), "What Is a Game?");
        assert_eq!(strip_numbering_prefix("Part II: The Deep History"), "The Deep History");
    }

    #[test]
    fn test_search_key() {
        assert_eq!(
            search_key("第1课 中华文明的起源与早期国家").as_deref(),
            Some("中华文明的起源与早期国家")
        );
        assert_eq!(search_key("ãƉã 正数和负数").as_deref(), Some("正数和负数"));
        assert!(search_key("小结").is_none());
        assert!(search_key("复习题").is_none());
    }

    #[test]
    fn test_clean_title() {
        assert_eq!(clean_title("Introduction............"), "Introduction");
        assert_eq!(clean_title("ãƉã 正数和负数"), "正数和负数");
    }

    #[test]
    fn test_append_title() {
        let mut t = "第22课 南京国民政府的统治和中国共产党开辟".to_string();
        append_title(&mut t, "革命新道路");
        assert_eq!(t, "第22课 南京国民政府的统治和中国共产党开辟革命新道路");
        let mut t2 = "Game Engine".to_string();
        append_title(&mut t2, "Architecture");
        assert_eq!(t2, "Game Engine Architecture");
    }

    #[test]
    fn test_parse_page_text() {
        assert_eq!(parse_page_text("42"), Some(42));
        assert_eq!(parse_page_text("０"), Some(0));
        assert_eq!(parse_page_text("ä"), None);
        assert_eq!(parse_page_text(""), None);
    }

    #[test]
    fn test_is_toc_marker() {
        assert!(is_toc_marker("目 录"));
        assert!(is_toc_marker("目录"));
        assert!(is_toc_marker("CONTENTS"));
        assert!(is_toc_marker("Table of Contents"));
        assert!(!is_toc_marker("前言"));
    }

    #[test]
    fn test_median_offset() {
        let mk = |page: usize, printed: usize| RawEntry {
            title: String::new(),
            printed_page: Some(printed),
            page: Some(page),
            level: 0,
            has_numbering: true,
            x: 0.0,
            font_size: 0.0,
        };
        let entries = vec![mk(8, 1), mk(9, 2), mk(34, 27), mk(16, 9)];
        assert_eq!(median_offset(&entries), Some(7));
        assert_eq!(median_offset(&[mk(8, 1)]), None);
    }

    #[test]
    fn test_merge_same_baseline_reading_order() {
        // pdf_oxide reports y in top-origin coordinates (larger y = higher on
        // the page); the merge must emit lines top-to-bottom so a wrapped
        // continuation ("的建立与巩固 1") follows its numbered parent
        // ("第一单元 …") instead of preceding it.
        let mk = |text: &str, y: f32, x: f32| RawLine {
            text: text.to_string(),
            x,
            y,
            width: 10.0,
            font_size: 12.0,
            words: vec![RawWord { text: text.to_string(), x, width: 10.0 }],
        };
        let merged = merge_same_baseline(vec![
            mk("第一单元 从中华文明起源到秦汉", 647.1, 183.8),
            mk("的建立与巩固", 622.1, 244.8),
            mk("目 录", 745.8, 270.0),
        ]);
        let texts: Vec<&str> = merged.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["目 录", "第一单元 从中华文明起源到秦汉", "的建立与巩固"]);
    }

    #[test]
    fn test_merge_same_baseline_fragments() {
        // Title and right-aligned page number on one baseline merge into a
        // single line with the page run last.
        let mk = |text: &str, x: f32| RawLine {
            text: text.to_string(),
            x,
            y: 300.0,
            width: 10.0,
            font_size: 12.0,
            words: vec![RawWord { text: text.to_string(), x, width: 10.0 }],
        };
        let merged = merge_same_baseline(vec![mk("第一章 有理数", 180.0), mk("8", 520.0)]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "第一章 有理数 8");
    }

    #[test]
    fn test_split_multi_entries_grid_toc() {
        // 地理七年级上册 style: several "title page" groups separated by dot
        // leaders on one extracted line.
        let line = "  .........................................第一节 地球和地球仪2 ..........................................第二节 地球的运动11 ..........................................第三节 地图的阅读16";
        let entries = split_multi_entries(line).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("第一节 地球和地球仪".to_string(), 2));
        assert_eq!(entries[1], ("第二节 地球的运动".to_string(), 11));
        assert_eq!(entries[2], ("第三节 地图的阅读".to_string(), 16));
        // A single entry without dot leaders is not a multi-entry line.
        assert!(split_multi_entries("第一章 有理数8").is_none());
        assert!(split_multi_entries("第一单元 从中华文明起源到秦汉").is_none());
    }

    #[test]
    fn test_has_internal_dot_leader() {
        assert!(has_internal_dot_leader("第一节 地球和地球仪2 .....第二节 地球的运动11"));
        assert!(!has_internal_dot_leader("............................................绪言 与同学们谈地理"));
        assert!(!has_internal_dot_leader("第1课 中华文明的起源与早期国家"));
    }

    #[test]
    fn test_plain_is_acceptable_gate() {
        let mk = |title: &str| RawEntry {
            title: title.to_string(),
            printed_page: Some(1),
            page: None,
            level: 2,
            has_numbering: true,
            x: 0.0,
            font_size: 0.0,
        };
        // Clean textbook TOC → accepted.
        let clean = PlainScan {
            entries: vec![
                mk("第1课 中华文明的起源与早期国家"),
                mk("第2课 诸侯纷争与变法运动"),
                mk("第3课 秦统一多民族封建国家的建立"),
                mk("第4课 西汉与东汉——统一多民族封建国家的巩固"),
            ],
            pages: vec![4],
            markers: vec![4],
        };
        assert!(plain_is_acceptable(&clean));
        // Broken ToUnicode glues page numbers into titles → rejected so the
        // layout path can recover the true structure.
        let garbled = PlainScan {
            entries: vec![
                mk("认识钟表847"),
                mk("以内数的认识和加减法143"),
                mk("准备课"),
                mk("数学乐园"),
            ],
            pages: vec![5],
            markers: vec![5],
        };
        assert!(!plain_is_acceptable(&garbled));
        // Legitimate numeric titles strip cleanly ("1 负数" → "负数").
        let numeric = PlainScan {
            entries: vec![
                mk("1 负数"),
                mk("2 百分数（二）"),
                mk("3 圆柱与圆锥"),
                mk("6 整理和复习"),
            ],
            pages: vec![5],
            markers: vec![5],
        };
        assert!(plain_is_acceptable(&numeric));
    }

    #[test]
    fn test_body_pages_to_scan_sampling() {
        // Unindexed: dense window after the TOC, then a stride.
        let pages = body_pages_to_scan(None, 150, 6);
        assert_eq!(pages[0], 7);
        assert!(pages.contains(&46)); // dense window covers 7..=46
        assert!(pages.windows(2).all(|w| w[0] < w[1]));
        // Indexed: every page.
        let all = body_pages_to_scan(Some(&vec![None; 150]), 150, 6);
        assert_eq!(all.len(), 144);
        assert_eq!(all[0], 7);
    }
}
