//! Table of Contents extraction via heuristics.
//!
//! Detects TOC entries from early PDF/EPUB pages by looking for:
//! - Dot-leader patterns ("Introduction...........45")
//! - Chapter/section headings ("Chapter X", "第X章", numbered sections)

use std::path::Path;
use serde::Serialize;

/// A detected TOC entry
#[derive(Debug, Clone, Serialize)]
pub struct TocEntry {
    pub page: usize,
    pub title: String,
    /// Nesting level: 0 = top-level, 1 = sub-section, etc.
    pub level: usize,
}

/// Detect TOC from a PDF or EPUB file.
///
/// Extracts text from the first `max_pages` pages and applies heuristic
/// pattern matching to find table-of-contents entries.
pub fn detect_toc(path: &Path, is_epub: bool, max_pages: usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for page_num in 1..=max_pages {
        let text = if is_epub {
            crate::extract_epub_chapter(path, page_num).unwrap_or_default()
        } else {
            crate::pdf::extract_page(path, page_num).unwrap_or_default()
        };

        if text.trim().is_empty() {
            continue;
        }

        let page_entries = extract_toc_from_text(&text, page_num);
        entries.extend(page_entries);
    }

    // If we found enough dot-leader entries, those are reliable — filter out
    // weaker heuristic matches from the same pages
    let dot_leader_count = entries.iter().filter(|e| e.level < 100).count();
    if dot_leader_count >= 3 {
        entries.retain(|e| e.level < 100);
    }

    // Deduplicate by title
    let mut seen = std::collections::HashSet::new();
    entries.retain(|e| seen.insert(e.title.clone()));

    entries
}

/// Extract TOC-like entries from a single page's text.
/// Returns entries with their heuristic confidence encoded in level:
///   level < 100 = dot-leader TOC (high confidence)
///   level >= 100 = chapter heading heuristic (level - 100 = indent depth)
fn extract_toc_from_text(text: &str, page_num: usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Pattern 1: Dot-leader TOC ("Section Title...........42")
        if let Some((title, _target_page)) = try_dot_leader(line) {
            let level = estimate_level(&title);
            entries.push(TocEntry { page: page_num, title, level });
            continue;
        }

        // Pattern 2: Chapter/section headings
        if let Some((title, level)) = try_chapter_heading(line) {
            entries.push(TocEntry { page: page_num, title, level: level + 100 });
        }
    }

    entries
}

/// Try to match a dot-leader TOC line: "text .... page_number"
fn try_dot_leader(line: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();

    // Find a run of 3+ dot-like characters
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
    let after = after.trim();

    // Try to parse page number
    let page_str = after.split_whitespace().next().unwrap_or("");
    if let Ok(page_num) = page_str.parse::<usize>() {
        if !title.is_empty() && title.len() > 1 && page_num > 0 && page_num < 10000 {
            return Some((title, page_num));
        }
    } else if !title.is_empty() && title.len() > 2 {
        // Dots but no page number — still a TOC line
        return Some((title, 0));
    }

    None
}

/// Try to match a chapter/section heading line
fn try_chapter_heading(line: &str) -> Option<(String, usize)> {
    let line = line.trim();

    // Skip lines that are too long (not a heading)
    if line.len() > 120 {
        return None;
    }

    let lower = line.to_lowercase();

    // "Chapter X" or "Chapter X: Title" or "CHAPTER X"
    if lower.starts_with("chapter ") {
        let rest = &line[8..].trim();
        if let Some((num_str, title_suffix)) = rest.split_once([':', '-', '.']) {
            if let Ok(num) = num_str.trim().parse::<usize>() {
                let title = format!("Chapter {}: {}", num, title_suffix.trim());
                return Some((title, 0));
            }
        } else if let Ok(num) = rest.parse::<usize>() {
            return Some((format!("Chapter {}", num), 0));
        }
    }

    // "第X章" (Chinese chapter) — matches patterns like "第一章", "第1章", "第十二章"
    if line.starts_with('第') {
        if let Some(zhang_pos) = line.find('章') {
            let num_part = &line[3..zhang_pos]; // skip "第" (3 bytes in UTF-8)
            let rest = line[zhang_pos + 3..].trim(); // skip "章" (3 bytes)
            let num_str = if num_part.is_empty() { "" } else { num_part };
            let title = if rest.is_empty() {
                format!("第{}章", num_str)
            } else {
                format!("第{}章 {}", num_str, rest)
            };
            return Some((title, 0));
        }
    }

    // Numbered section: "1.1 Title" or "2.3.1 Title"
    if let Some((num, title)) = try_numbered_section(line) {
        let level = num.matches('.').count().saturating_sub(1);
        return Some((format!("{} {}", num, title), level.min(3)));
    }

    None
}

/// Try "N.N Title" or "N.N.N Title" pattern
fn try_numbered_section(line: &str) -> Option<(String, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    // Read digits and dots: "1.1" or "2.3.1"
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
        i += 1;
    }

    if i == 0 || !chars[..i].iter().collect::<String>().contains('.') {
        return None;
    }

    // Must be followed by whitespace
    if i >= chars.len() || !chars[i].is_whitespace() {
        return None;
    }

    let num: String = chars[..i].iter().collect();
    let title: String = chars[i..].iter().collect();
    let title = title.trim().to_string();

    if title.len() > 2 && title.len() < 100 {
        Some((num, title))
    } else {
        None
    }
}

/// Estimate nesting level from section numbering
fn estimate_level(title: &str) -> usize {
    if let Some(first_word) = title.split_whitespace().next() {
        let dots = first_word.chars().filter(|&c| c == '.').count();
        if dots > 0 && first_word.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return dots.saturating_sub(1);
        }
    }
    if title.starts_with("  ") {
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dot_leader() {
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
    fn test_chapter_heading() {
        let result = try_chapter_heading("Chapter 4: System Design and Execution Models");
        assert!(result.is_some());
        let (title, level) = result.unwrap();
        assert!(title.contains("Chapter 4"));
        assert_eq!(level, 0);
    }

    #[test]
    fn test_cjk_chapter() {
        let result = try_chapter_heading("第一章 理论基础");
        assert!(result.is_some());
        let (title, level) = result.unwrap();
        assert_eq!(title, "第一章 理论基础");
        assert_eq!(level, 0);
    }

    #[test]
    fn test_numbered_section() {
        let result = try_chapter_heading("1.1 Origins of ECS Architecture");
        assert!(result.is_some());
        let (title, _level) = result.unwrap();
        assert!(title.contains("1.1"));
    }
}
