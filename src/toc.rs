//! Table of Contents extraction.
//!
//! Strategy (in order of preference):
//! 1. PDF embedded outlines (/Outlines in document catalog) — what PDF readers show
//! 2. Heuristic: dot-leader TOC on early pages + heading scan

use std::path::Path;
use std::collections::HashMap;
use serde::Serialize;
use pdf_oxide::{PdfDocument, object::{ObjectRef, Object}};

/// A detected TOC entry
#[derive(Debug, Clone, Serialize)]
pub struct TocEntry {
    pub page: usize,
    pub title: String,
    /// Nesting level: 0 = part/title, 1 = chapter, 2 = section, 3 = subsection
    pub level: usize,
}

/// Extract the PDF's embedded outline tree (what PDF readers display as the TOC sidebar).
/// Returns None if the PDF has no outlines or extraction fails.
fn extract_pdf_outlines(path: &Path) -> Option<Vec<TocEntry>> {
    let doc = PdfDocument::open(path).ok()?;
    let catalog = doc.catalog().ok()?;
    let catalog_dict = catalog.as_dict()?;

    // Try common key names for the outline tree
    let outlines_obj = catalog_dict.get("Outlines")
        .or_else(|| catalog_dict.get("Outlines "))
        .or_else(|| {
            // Some PDFs store it indirectly — scan all keys
            catalog_dict.iter().find(|(k, _)| {
                k.eq_ignore_ascii_case("Outlines") || k.contains("utline")
            }).map(|(_, v)| v)
        })?;

    let outlines_ref = outlines_obj.as_reference()?;
    let outlines_root = doc.load_object(outlines_ref).ok()?;
    let outlines_dict = outlines_root.as_dict()?;

    // Get the first top-level outline item
    let first_ref = match outlines_dict.get("First") {
        Some(f) => f.as_reference()?,
        None => {            // Some PDFs use /Kids array for outlines
            match outlines_dict.get("Kids").and_then(|k| k.as_array()) {
                Some(kids) => {                    let mut entries = Vec::new();
                    for kid in kids {
                        if let Some(kid_ref) = kid.as_reference() {
                            walk_outline_items(&doc, kid_ref, 0, &mut entries);
                        }
                    }                    return Some(entries);
                }
                None => { eprintln!("  TOC: no /First and no /Kids"); return None; }
            }
        }
    };
    let mut entries = Vec::new();
    walk_outline_items(&doc, first_ref, 0, &mut entries);    Some(entries)
}

/// Recursively walk the PDF outline tree (linked list via /Next and /First).
fn walk_outline_items(doc: &PdfDocument, item_ref: ObjectRef, level: usize, entries: &mut Vec<TocEntry>) {
    let mut current_ref = item_ref;
    let mut visited = std::collections::HashSet::new();

    loop {
        if !visited.insert((current_ref.id, current_ref.r#gen)) {
            break; // prevent infinite loops from malformed PDFs
        }

        let item = match doc.load_object(current_ref) {
            Ok(o) => o,
            Err(e) => { eprintln!("  TOC: failed to load item {:?}: {}", current_ref, e); break; }
        };
        let dict = match item.as_dict() {
            Some(d) => d,
            None => { eprintln!("  TOC: item is not a dict"); break; }
        };
        // Extract title
        let title = dict.get("Title")
            .and_then(|t| {                t.as_string()
            })
            .map(|bytes| {                pdf_string_to_text(bytes)
            })
            .unwrap_or_default();

        if !title.is_empty() {
            // Extract page number from /Dest or /A (GoTo action)
            let page = dict.get("Dest")
                .and_then(|d| parse_dest_page(doc, d))
                .or_else(|| dict.get("A").and_then(|a| {                    // If /A is a reference, load it first
                    let action_obj = if let Some(ref r) = a.as_reference() {
                        doc.load_object(*r).ok()?
                    } else {
                        a.clone()
                    };
                    parse_action_dest(doc, &action_obj)
                }))
                .unwrap_or(0);
            if page > 0 {
                entries.push(TocEntry { page, title, level });
            }
        }

        // Process children first (they come before next sibling in display)
        if let Some(child_ref) = dict.get("First").and_then(|c| c.as_reference()) {
            walk_outline_items(doc, child_ref, level + 1, entries);
        }

        // Move to next sibling
        match dict.get("Next").and_then(|n| n.as_reference()) {
            Some(next) => current_ref = next,
            None => break,
        }
    }
}

/// Parse a PDF GoTo action (/A dict) to get the destination page number.
fn parse_action_dest(doc: &PdfDocument, action: &Object) -> Option<usize> {
    let action_dict = action.as_dict()?;    let s = action_dict.get("S")?.as_name()?;    if s != "GoTo" { return None; }
    let dest = action_dict.get("D")?;    parse_dest_page(doc, dest)
}

/// Parse a PDF destination (/Dest) to get the page number (1-indexed).
/// Dest can be: [page_ref /XYZ ...] or [page_ref /Fit] or just a page number.
fn parse_dest_page(doc: &PdfDocument, dest: &Object) -> Option<usize> {
    match dest {
        // Array format: [page_ref /Fit] or [page_idx /XYZ ...]
        Object::Array(arr) => {
            let first = arr.first()?;            if let Some(page_idx) = first.as_integer() {
                return Some((page_idx + 1) as usize);
            }
            if let Some(page_ref) = first.as_reference() {
                return page_ref_to_index(doc, page_ref);
            }
            None
        }
        // Named destination (string) — resolve from /Dests name tree
        Object::String(name_bytes) => {
            let name = String::from_utf8_lossy(name_bytes);            resolve_named_dest(doc, name_bytes)
        }
        // Integer page number directly
        Object::Integer(idx) => Some((*idx + 1) as usize),
        _ => { eprintln!("  TOC: unexpected dest type: {}", dest.type_name()); None }
    }
}

/// Resolve a named destination from the document's /Dests name tree.
fn resolve_named_dest(doc: &PdfDocument, name: &[u8]) -> Option<usize> {
    let catalog = doc.catalog().ok()?;
    let catalog_dict = catalog.as_dict()?;
    let dests = catalog_dict.get("Dests")?;    // /Dests can be a dict or a reference to a dict
    let dests_dict = if let Some(d) = dests.as_dict() {
        d.clone()
    } else if let Some(r) = dests.as_reference() {
        doc.load_object(r).ok()?.as_dict()?.clone()
    } else {
        return None;
    };
    let name_str = String::from_utf8_lossy(name);
    let dest_obj = dests_dict.get(name_str.as_ref())?;
    parse_dest_page(doc, dest_obj)
}

/// Map a page object reference to its 1-indexed page number by scanning the page tree.
fn page_ref_to_index(doc: &PdfDocument, target_ref: ObjectRef) -> Option<usize> {
    let catalog = doc.catalog().ok()?;
    let catalog_dict = catalog.as_dict()?;
    let pages_root = catalog_dict.get("Pages")?.as_reference()?;
    let pages_obj = doc.load_object(pages_root).ok()?;
    let pages_dict = pages_obj.as_dict()?;

    let mut page_refs = Vec::new();
    collect_page_refs(doc, pages_dict, &mut page_refs);
    page_refs.iter().position(|&r| r == target_ref).map(|i| i + 1)
}

/// Recursively collect page references from the page tree (/Kids arrays).
fn collect_page_refs(doc: &PdfDocument, node: &HashMap<String, Object>, refs: &mut Vec<ObjectRef>) {
    if let Some(type_name) = node.get("Type").and_then(|t| t.as_name()) {
        if type_name == "Page" {
            return; // leaf pages are collected by their parent
        }
    }

    if let Some(kids) = node.get("Kids").and_then(|k| k.as_array()) {
        for kid in kids {
            if let Some(kid_ref) = kid.as_reference() {
                if let Ok(kid_obj) = doc.load_object(kid_ref) {
                    if let Some(kid_dict) = kid_obj.as_dict() {
                        if let Some(type_name) = kid_dict.get("Type").and_then(|t| t.as_name()) {
                            if type_name == "Page" {
                                refs.push(kid_ref);
                            } else if type_name == "Pages" {
                                collect_page_refs(doc, kid_dict, refs);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Convert PDF-encoded string bytes to UTF-8 text.
fn pdf_string_to_text(bytes: &[u8]) -> String {
    // Try UTF-16BE (PDF standard encoding for text strings)
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let utf16: Vec<u16> = bytes[2..].chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&utf16).unwrap_or_default();
    }
    // Try plain UTF-8 / ASCII
    String::from_utf8(bytes.to_vec()).unwrap_or_default()
}

/// Detect TOC from a PDF or EPUB file.
///
/// Two-phase: first scan early pages for dot-leader TOC entries (with target page
/// numbers), then scan every page for chapter/section heading patterns.
pub fn detect_toc(path: &Path, total_pages: usize) -> Vec<TocEntry> {
    // Try PDF embedded outlines first — what PDF readers use for the TOC sidebar
    if let Some(outlines) = extract_pdf_outlines(path) {
        if !outlines.is_empty() {
            return outlines;
        }
    }

    // Fallback: heuristic detection
    // Phase 1: Dot-leader TOC from early pages
    let mut entries: Vec<TocEntry> = Vec::new();
    let scan_pages = total_pages.min(15);

    for page_num in 1..=scan_pages {
        let page_entries = extract_dot_leaders(&extract_text(path, page_num), total_pages);
        entries.extend(page_entries);
        // Early exit once we have enough entries
        if entries.len() >= 10 {
            break;
        }
    }

    let has_toc = entries.len() >= 3;

    // Phase 2: Heading scan. If the index cache covers this document, scan
    // ALL pages (near-instant from disk cache). Otherwise sample ~25 pages.
    let index_available = crate::index::read_valid_meta(path).is_some();

    let heading_entries = if index_available {
        // Full scan: read every page from index cache (~100ms for 800+ pages)
        extract_headings_for_pages(path, &(1..=total_pages).collect::<Vec<_>>())
    } else if has_toc {
        // Sample + boundary scan around known chapter pages
        let mut pages_to_scan: Vec<usize> = (1..=total_pages).step_by((total_pages / 25).max(5)).collect();
        for e in &entries {
            let p = e.page as i32;
            for offset in -3i32..=3i32 {
                let page = (p + offset).max(1).min(total_pages as i32) as usize;
                pages_to_scan.push(page);
            }
        }
        pages_to_scan.sort();
        pages_to_scan.dedup();
        extract_headings_for_pages(path, &pages_to_scan)
    } else {
        // No index, no TOC: sample ~25 pages
        extract_headings_for_pages(path, &(1..=total_pages).step_by((total_pages / 25).max(5)).collect::<Vec<_>>())
    };

    // Merge: prefer dot-leader entries (correct target page from TOC), supplement
    // with heading entries whose titles aren't already covered by dot-leader results.
    if has_toc {
        let covered_titles: std::collections::HashSet<String> = entries.iter()
            .map(|e| normalize_title(&e.title))
            .collect();
        for h in heading_entries {
            if !covered_titles.contains(&normalize_title(&h.title)) {
                entries.push(h);
            }
        }
    } else {
        entries = heading_entries;
    }

    // Deduplicate by normalized title (keep first occurrence)
    let mut seen = std::collections::HashSet::new();
    entries.retain(|e| seen.insert(normalize_title(&e.title)));

    // Sort by page
    entries.sort_by_key(|e| e.page);

    entries
}

/// Extract headings from a specific set of pages
fn extract_headings_for_pages(path: &Path, pages: &[usize]) -> Vec<TocEntry> {
    let mut entries = Vec::new();
    for &page_num in pages {
        let text = extract_text(path, page_num);
        entries.extend(extract_headings(&text, page_num));
    }
    entries
}

/// Extract text from a page, preferring the index cache if available (fast disk read).
/// Falls back to pdf_oxide/epub extraction when no cache exists.
fn extract_text(path: &Path, page_num: usize) -> String {
    // Try index cache first — near-instant vs pdf_oxide extraction
    if let Some(cached) = read_from_index_cache(path, page_num) {
        return cached;
    }
    // Fallback: direct extraction
    crate::pdf::extract_page(path, page_num).unwrap_or_default()
}

/// Read a page's text from the index cache if the document is indexed and fresh
fn read_from_index_cache(path: &Path, page_num: usize) -> Option<String> {
    let meta = crate::index::read_valid_meta(path)?;
    if page_num > meta.pages {
        return None;
    }
    let hash_dir = crate::index::get_index_dir(path);
    let page_file = hash_dir.join(format!("page_{:04}.txt", page_num));
    std::fs::read_to_string(&page_file).ok()
}

/// Phase 1: Extract TOC entries from a page.
/// Handles two common TOC formats:
///   - Dot-leader: "Section Title...........42"
///   - Right-aligned: "1.1 Section Title           42"
/// Indentation level maps to hierarchy depth.
fn extract_dot_leaders(text: &str, total_pages: usize) -> Vec<TocEntry> {
    // Collect candidates first, then apply page-level filtering
    let mut dot_leaders = Vec::new();
    let mut right_aligned = Vec::new();

    for raw_line in text.lines() {
        if raw_line.trim().is_empty() {
            continue;
        }

        if let Some((title, target_page)) = try_dot_leader(raw_line) {
            if target_page > 0 && target_page <= total_pages.saturating_add(100) {
                dot_leaders.push(TocEntry {
                    page: target_page,
                    title,
                    level: indent_level(raw_line),
                });
                continue;
            }
        }

        if let Some((title, target_page)) = try_right_aligned(raw_line) {
            if target_page > 0 && target_page <= total_pages.saturating_add(100) {
                right_aligned.push(TocEntry {
                    page: target_page,
                    title,
                    level: indent_level(raw_line),
                });
            }
        }
    }

    // Dot-leaders are always high-confidence. Right-aligned only if ≥2 on the page
    // (isolated matches are likely false positives from body text).
    let mut entries = dot_leaders;
    if right_aligned.len() >= 2 {
        entries.extend(right_aligned);
    }

    entries
}

/// Hybrid hierarchy level: tries indentation first, falls back to numbering depth
fn indent_level(line: &str) -> usize {
    let spaces = line.chars().take_while(|&c| c == ' ').count();
    if spaces >= 6 { return 3; }
    if spaces >= 3 { return 2; }

    // Fallback: numbering depth (1→0, 1.1→1, 1.1.1→2)
    numbering_level(line.trim())
}

/// Level from section numbering pattern ("1.1 Title") — dots count = depth
fn numbering_level(text: &str) -> usize {
    if let Some(first_word) = text.split_whitespace().next() {
        let digits_and_dots: String = first_word.chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if digits_and_dots.contains('.') {
            let dots = digits_and_dots.matches('.').count();
            return dots.min(2); // 1.1→1, 1.1.1→2
        }
        // Plain number like "1" or "2" → chapter
        if digits_and_dots.chars().all(|c| c.is_ascii_digit()) && !digits_and_dots.is_empty() {
            return 0;
        }
    }
    0
}

/// Phase 2: Extract chapter/section headings from a page.
/// Only matches strong, specific patterns — avoids body-text false positives.
fn extract_headings(text: &str, page_num: usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.len() > 120 {
            continue;
        }

        if let Some((title, level)) = try_chapter_heading(line) {
            entries.push(TocEntry { page: page_num, title, level });
        }
    }

    entries
}

/// Dot-leader pattern: "text ............ page_number"
fn try_dot_leader(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();

    let mut dot_start = None;
    let mut dot_end = None;
    let mut consecutive = 0usize;

    for (i, &c) in chars.iter().enumerate() {
        if c == '.' || c == '·' || c == '…' {
            if consecutive == 0 {
                dot_start = Some(i);
            }
            consecutive += 1;
            dot_end = Some(i);
        } else if consecutive > 0 {
            if consecutive >= 3 {
                break;
            }
            consecutive = 0;
            dot_start = None;
        }
    }

    if consecutive < 3 || dot_start.is_none() {
        return None;
    }

    let start = dot_start.unwrap();
    let end = dot_end.unwrap();

    let title: String = chars[..start].iter().collect();
    let title = title.trim().trim_end_matches('.').trim().to_string();

    let after: String = chars[end + 1..].iter().collect();
    let page_str = after.split_whitespace().next().unwrap_or("");

    if let Ok(page_num) = page_str.parse::<usize>() {
        if !title.is_empty() && title.len() > 1 && page_num > 0 && page_num < 10000 {
            return Some((title, page_num));
        }
    }

    None
}

/// Right-aligned TOC format: "Section Title           42"
/// PDF text extraction collapses whitespace — the page number just needs to
/// be the last word and the title not look like a sentence.
fn try_right_aligned(line: &str) -> Option<(String, usize)> {
    let trimmed = line.trim();
    let last_word = trimmed.split_whitespace().last()?;
    let page_num = last_word.parse::<usize>().ok()?;
    if page_num == 0 || page_num > 10000 {
        return None;
    }

    let before_num = &trimmed[..trimmed.len() - last_word.len()];
    let title = before_num.trim().to_string();
    if title.len() < 2 || title.len() > 100 {
        return None;
    }

    // Reject if it looks like a sentence
    let word_count = title.split_whitespace().count();
    if word_count > 8 || title.ends_with('.') || title.ends_with(',') || title.ends_with(';') {
        return None;
    }
    // Title must look like a heading: numbered prefix, or short enough to be a title
    let first_char = title.chars().next().unwrap_or(' ');
    let has_numbered_prefix = first_char.is_ascii_digit() || first_char.is_alphabetic();
    if !has_numbered_prefix || word_count < 2 {
        // Short single-word or punctuation-starting lines are unlikely to be TOC entries
        if word_count < 2 && title.len() < 3 {
            return None;
        }
    }

    Some((title, page_num))
}

/// Strong heading patterns only. No broad heuristics.
fn try_chapter_heading(line: &str) -> Option<(String, usize)> {
    // Pattern 1: "Chapter N" or "Chapter N: Title" or "CHAPTER N"
    if let Some(result) = match_chapter(line) {
        return Some(result);
    }

    // Pattern 2: "第X章" (Chinese chapter headings)
    if let Some(result) = match_cjk_chapter(line) {
        return Some(result);
    }

    // Pattern 3: Numbered chapter: "N. Title" (single number + period, chapter level)
    if let Some(result) = match_numbered_chapter(line) {
        return Some(result);
    }

    // Pattern 4: Numbered section: "N.N Title" or "N.N.N Title"
    if let Some(result) = match_numbered_section(line) {
        return Some(result);
    }

    // Pattern 5: "Part N" or "Part N: Title"
    if let Some(result) = match_part(line) {
        return Some(result);
    }

    // Pattern 6: ALL-CAPS short line (likely a heading like "INTRODUCTION" or "REFERENCES")
    if let Some(result) = match_allcaps_heading(line) {
        return Some(result);
    }

    None
}

fn match_chapter(line: &str) -> Option<(String, usize)> {
    let lower = line.to_lowercase();
    if !lower.starts_with("chapter ") {
        return None;
    }

    let rest = line[8..].trim(); // skip "Chapter "
    if rest.is_empty() {
        return None;
    }

    // "Chapter N: Title" or "Chapter N - Title" or "Chapter N. Title"
    for sep in [':', '-', '.'] {
        if let Some((num_str, title_suffix)) = rest.split_once(sep) {
            if let Ok(num) = num_str.trim().parse::<usize>() {
                let title = format!("Chapter {}: {}", num, title_suffix.trim());
                return Some((title, 1));
            }
        }
    }

    // Just "Chapter N"
    if let Ok(num) = rest.parse::<usize>() {
        return Some((format!("Chapter {}", num), 1));
    }

    None
}

fn match_cjk_chapter(line: &str) -> Option<(String, usize)> {
    if !line.starts_with('第') {
        return None;
    }
    let zhang_pos = line.find('章')?;
    let rest = line[zhang_pos + 3..].trim(); // skip "章" (3 bytes in UTF-8)

    let title = if rest.is_empty() {
        line.to_string()
    } else {
        format!("{} {}", &line[..zhang_pos + 3], rest)
    };
    Some((title, 1))
}

/// Numbered chapter: "4. Parallelism and Concurrent Programming"
fn match_numbered_chapter(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    // Read digits
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    // Must be followed by a period, then whitespace, then a title
    if i == 0 || i >= chars.len() || chars[i] != '.' {
        return None;
    }
    if i + 1 >= chars.len() || !chars[i + 1].is_whitespace() {
        return None;
    }
    let num: usize = chars[..i].iter().collect::<String>().parse().ok()?;
    // Chapter numbers are sane (≤ 50 for any book)
    if num > 50 {
        return None;
    }
    let title: String = chars[i + 1..].iter().collect();
    let title = title.trim().to_string();
    let word_count = title.split_whitespace().count();
    if title.len() < 3 || title.len() > 80 || word_count > 8 || word_count < 2 {
        return None;
    }
    // Title must start with uppercase
    let first_char = title.chars().next().unwrap_or(' ');
    if !first_char.is_ascii_uppercase() {
        return None;
    }
    // Reject code-like lines
    if title.contains("/*") || title.contains("*/") || title.contains('{') || title.contains('}') {
        return None;
    }
    // Title case check: count lowercase-starting words. Chapter headings have ≤2
    // (prepositions like "of", "and"). Body text has ≥3 lowercase starters.
    let lowercase_words: usize = title.split_whitespace()
        .filter(|w| w.starts_with(|c: char| c.is_ascii_lowercase()))
        .count();
    if lowercase_words > 2 {
        return None;
    }
    // Reject common body-text markers
    let lower = title.to_lowercase();
    for body_word in &["might", "should", "will", "can ", "may ", "does ", "has ", "the target"] {
        if lower.contains(body_word) && word_count > 4 { return None; }
    }
    Some((format!("{} {}", num, title), 0))
}

fn match_numbered_section(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    // Read digits and dots: "1.1" or "2.3.1" or "A.1"
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
        i += 1;
    }

    if i < 3 || !chars[..i].iter().collect::<String>().contains('.') {
        return None;
    }

    // Must be followed by whitespace
    if i >= chars.len() || !chars[i].is_whitespace() {
        return None;
    }

    let num: String = chars[..i].iter().collect();
    let title: String = chars[i..].iter().collect();
    let title = title.trim().to_string();

    // Reject if title looks like a sentence or code
    let word_count = title.split_whitespace().count();
    if word_count > 12 || title.ends_with('.') || title.len() < 2 {
        return None;
    }
    if title.contains("/*") || title.contains("*/") || title.contains('{') || title.contains('}') {
        return None;
    }

    let level = (num.matches('.').count() + 1).min(3); // "1"→1, "1.1"→2, "1.1.1"→3
    Some((format!("{} {}", num, title), level))
}

fn match_part(line: &str) -> Option<(String, usize)> {
    let lower = line.to_lowercase();
    if !lower.starts_with("part ") {
        return None;
    }
    let rest = line[5..].trim();
    // Must start with a number or roman numeral
    let first_char = rest.chars().next()?;
    if !first_char.is_ascii_digit() && !"IVXLCDMivxlcdm".contains(first_char) {
        return None;
    }
    // "Part I" or "Part 1: Title"
    for sep in [':', '-', '.'] {
        if let Some((num_str, title_suffix)) = rest.split_once(sep) {
            let title = format!("Part {}: {}", num_str.trim(), title_suffix.trim());
            return Some((title, 0));
        }
    }
    Some((format!("Part {}", rest), 0))
}

fn match_allcaps_heading(line: &str) -> Option<(String, usize)> {
    // Only match if the line is ALL uppercase ASCII letters/spaces and short
    if line.len() < 4 || line.len() > 60 {
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

    // Common heading keywords
    let upper = line.to_uppercase();
    let heading_words = ["INTRODUCTION", "CONCLUSION", "REFERENCES", "BIBLIOGRAPHY",
        "APPENDIX", "PREFACE", "ACKNOWLEDGMENTS", "INDEX", "GLOSSARY",
        "ABSTRACT", "SUMMARY", "FOREWORD", "CONTENTS"];
    if heading_words.iter().any(|&w| upper.contains(w)) {
        return Some((line.to_string(), 1));
    }

    None
}

/// Normalize a title for deduplication: lowercase, collapse whitespace, strip trailing number
fn normalize_title(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut words: Vec<&str> = lower.split_whitespace().collect();
    // Strip trailing page number if present (Phase 2 picks these up on TOC pages)
    if words.len() > 1 {
        if let Some(last) = words.last() {
            if last.parse::<usize>().is_ok() {
                words.pop();
            }
        }
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot_leader_simple() {
        let result = try_dot_leader("Introduction...........45");
        assert!(result.is_some());
        let (title, page) = result.unwrap();
        assert_eq!(title, "Introduction");
        assert_eq!(page, 45);
    }

    #[test]
    fn test_dot_leader_numbered() {
        let result = try_dot_leader("1.1 Section Title............123");
        assert!(result.is_some());
        let (title, page) = result.unwrap();
        assert_eq!(title, "1.1 Section Title");
        assert_eq!(page, 123);
    }

    #[test]
    fn test_chapter_with_title() {
        let result = match_chapter("Chapter 4: System Design and Execution Models");
        assert!(result.is_some());
        let (title, level) = result.unwrap();
        assert_eq!(title, "Chapter 4: System Design and Execution Models");
        assert_eq!(level, 1);
    }

    #[test]
    fn test_chapter_plain_number() {
        let result = match_chapter("Chapter 1");
        assert!(result.is_some());
        let (title, level) = result.unwrap();
        assert_eq!(title, "Chapter 1");
        assert_eq!(level, 1);
    }

    #[test]
    fn test_cjk_chapter() {
        let result = match_cjk_chapter("第一章 理论基础");
        assert!(result.is_some());
        let (title, level) = result.unwrap();
        assert_eq!(title, "第一章 理论基础");
        assert_eq!(level, 1);
    }

    #[test]
    fn test_numbered_section_level() {
        let result = match_numbered_section("1.1 Origins of ECS Architecture");
        assert!(result.is_some());
        let (title, level) = result.unwrap();
        assert_eq!(title, "1.1 Origins of ECS Architecture");
        assert_eq!(level, 2);
    }

    #[test]
    fn test_numbered_subsection_level() {
        let result = match_numbered_section("2.3.1 Detailed Analysis of Patterns");
        assert!(result.is_some());
        let (title, level) = result.unwrap();
        assert_eq!(title, "2.3.1 Detailed Analysis of Patterns");
        assert_eq!(level, 3);
    }

    #[test]
    fn test_body_text_rejected() {
        // Sentences from body text should NOT match
        assert!(try_chapter_heading("If the Velocity component is removed, the entity moves to archetype C").is_none());
        assert!(try_chapter_heading("Additionally, we explore strategies for partitioning entities").is_none());
        assert!(try_chapter_heading("The principal taxonomy of ECS patterns").is_none());
    }

    #[test]
    fn test_allcaps_heading() {
        let result = match_allcaps_heading("INTRODUCTION");
        assert!(result.is_some());
    }

    #[test]
    fn test_allcaps_non_heading_rejected() {
        assert!(match_allcaps_heading("RANDOM TEXT THAT IS ALL CAPS").is_none());
    }

    #[test]
    fn test_right_aligned_simple() {
        // Realistic extracted text — PDF doesn't preserve visual spacing
        let result = try_right_aligned("1 Introduction 1");
        assert!(result.is_some());
        let (title, page) = result.unwrap();
        assert_eq!(title, "1 Introduction");
        assert_eq!(page, 1);
    }

    #[test]
    fn test_right_aligned_section() {
        let result = try_right_aligned("1.1 What Is a Game? 3");
        assert!(result.is_some());
        let (title, page) = result.unwrap();
        assert_eq!(title, "1.1 What Is a Game?");
        assert_eq!(page, 3);
    }

    #[test]
    fn test_right_aligned_rejects_sentence() {
        // A sentence ending with a number — too many words, should be rejected
        assert!(try_right_aligned("This is a long sentence that ends with the number 42").is_none());
    }

    #[test]
    fn test_indent_levels() {
        assert_eq!(indent_level("Preface xi"), 0);
        assert_eq!(indent_level("1 Introduction 1"), 0);
        assert_eq!(indent_level("1.1 Section Title  3"), 1);
        assert_eq!(indent_level("1.1.1 Subsection  5"), 2);
        assert_eq!(indent_level("    indented section  3"), 2);
    }
}
