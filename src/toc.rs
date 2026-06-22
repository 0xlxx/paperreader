//! Table of Contents extraction via two-phase heuristics.
//!
//! Phase 1: Dot-leader detection on early pages (where the printed TOC lives).
//! Phase 2: Heading-pattern scan across ALL pages for chapter/section headings.
//!
//! Only strong, specific patterns are matched — no broad "title case" heuristic,
//! to avoid body-text false positives.

use std::path::Path;
use serde::Serialize;

/// A detected TOC entry
#[derive(Debug, Clone, Serialize)]
pub struct TocEntry {
    pub page: usize,
    pub title: String,
    /// Nesting level: 0 = part/title, 1 = chapter, 2 = section, 3 = subsection
    pub level: usize,
}

/// Detect TOC from a PDF or EPUB file.
///
/// Two-phase: first scan early pages for dot-leader TOC entries (with target page
/// numbers), then scan every page for chapter/section heading patterns.
pub fn detect_toc(path: &Path, is_epub: bool, total_pages: usize) -> Vec<TocEntry> {
    // Phase 1: Dot-leader TOC from early pages
    let mut entries: Vec<TocEntry> = Vec::new();
    let scan_pages = total_pages.min(15);

    for page_num in 1..=scan_pages {
        let page_entries = extract_dot_leaders(&extract_text(path, is_epub, page_num), total_pages);
        entries.extend(page_entries);
        // Early exit once we have enough entries
        if entries.len() >= 10 {
            break;
        }
    }

    let has_toc = entries.len() >= 3;

    // Phase 2: Heading scan. Always do a sampling pass across the document.
    // When we have a TOC, also scan near chapter boundaries for section headings.
    let step = (total_pages / 25).max(5);
    let mut pages_to_scan: Vec<usize> = (1..=total_pages).step_by(step).collect();

    if has_toc {
        // Also scan ±3 pages around each chapter boundary
        for e in &entries {
            let p = e.page as i32;
            for offset in -3i32..=3i32 {
                let page = (p + offset).max(1).min(total_pages as i32) as usize;
                pages_to_scan.push(page);
            }
        }
        pages_to_scan.sort();
        pages_to_scan.dedup();
    }

    let heading_entries = extract_headings_for_pages(path, is_epub, &pages_to_scan);

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
fn extract_headings_for_pages(path: &Path, is_epub: bool, pages: &[usize]) -> Vec<TocEntry> {
    let mut entries = Vec::new();
    for &page_num in pages {
        let text = extract_text(path, is_epub, page_num);
        entries.extend(extract_headings(&text, page_num));
    }
    entries
}

/// Extract text from a page, preferring the index cache if available (fast disk read).
/// Falls back to pdf_oxide/epub extraction when no cache exists.
fn extract_text(path: &Path, is_epub: bool, page_num: usize) -> String {
    // Try index cache first — near-instant vs pdf_oxide extraction
    if let Some(cached) = read_from_index_cache(path, page_num) {
        return cached;
    }
    // Fallback: direct extraction
    if is_epub {
        crate::extract_epub_chapter(path, page_num).unwrap_or_default()
    } else {
        crate::pdf::extract_page(path, page_num).unwrap_or_default()
    }
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
    // Title must start with uppercase and have at least one more uppercase word
    let first_char = title.chars().next().unwrap_or(' ');
    if !first_char.is_ascii_uppercase() {
        return None;
    }
    // Reject code-like lines and body text fragments
    if title.contains("/*") || title.contains("*/") || title.contains('{') || title.contains('}')
        || title.contains("the ") && word_count <= 3 {
        return None; // "1 the mesh itself" — body text
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
