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
        let text = extract_text(path, is_epub, page_num);
        let page_entries = extract_dot_leaders(&text, page_num);
        entries.extend(page_entries);
    }

    // If we found a real TOC (≥3 dot-leader entries), those are authoritative.
    // Otherwise, fall through to heading scan.
    let has_toc = entries.len() >= 3;

    // Phase 2: Heading scan across ALL pages
    let mut heading_entries: Vec<TocEntry> = Vec::new();
    for page_num in 1..=total_pages {
        let text = extract_text(path, is_epub, page_num);
        let page_headings = extract_headings(&text, page_num);
        heading_entries.extend(page_headings);
    }

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

fn extract_text(path: &Path, is_epub: bool, page_num: usize) -> String {
    if is_epub {
        crate::extract_epub_chapter(path, page_num).unwrap_or_default()
    } else {
        crate::pdf::extract_page(path, page_num).unwrap_or_default()
    }
}

/// Phase 1: Extract dot-leader TOC entries from a page.
/// Dot-leader lines have the format "Section Title...........42" where 42 is the target page.
fn extract_dot_leaders(text: &str, _scan_page: usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((title, target_page)) = try_dot_leader(line) {
            if target_page > 0 {
                entries.push(TocEntry {
                    page: target_page,
                    title,
                    level: estimate_level_from_toc(line),
                });
            }
        }
    }

    entries
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

    // Pattern 3: Numbered section: "N.N Title" or "N.N.N Title"
    if let Some(result) = match_numbered_section(line) {
        return Some(result);
    }

    // Pattern 4: "Part N" or "Part N: Title"
    if let Some(result) = match_part(line) {
        return Some(result);
    }

    // Pattern 5: ALL-CAPS short line (likely a heading like "INTRODUCTION" or "REFERENCES")
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
    if rest.is_empty() {
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

/// Normalize a title for deduplication: lowercase, collapse whitespace
fn normalize_title(title: &str) -> String {
    title.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Estimate level from indentation in a dot-leader TOC line
fn estimate_level_from_toc(line: &str) -> usize {
    // Count leading spaces as indentation proxy
    let leading_spaces = line.chars().take_while(|&c| c == ' ').count();
    if leading_spaces >= 8 {
        return 3;
    }
    if leading_spaces >= 4 {
        return 2;
    }

    // Check for numbered prefix depth
    let trimmed = line.trim();
    if let Some(first_word) = trimmed.split_whitespace().next() {
        let dots = first_word.chars().filter(|&c| c == '.').count();
        if dots > 0 && first_word.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return dots.min(3);
        }
    }

    1 // default chapter level
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
        // Random ALL CAPS that isn't a known heading word
        assert!(match_allcaps_heading("RANDOM TEXT THAT IS ALL CAPS").is_none());
    }
}
