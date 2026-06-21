use std::collections::BTreeSet;

use comrak::nodes::NodeValue;
use comrak::{Arena, Options, markdown_to_html, parse_document};

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    options.extension.strikethrough = true;
    // render.unsafe_ stays false: raw HTML in memo content is escaped, not executed.
    options
}

/// Render memo markdown to sanitized HTML.
pub fn render(content: &str) -> String {
    markdown_to_html(content, &options())
}

/// Collect `#tags` from the markdown's text nodes — code spans, code blocks,
/// and URLs never produce tags because they aren't Text nodes.
pub fn extract_tags(content: &str) -> Vec<String> {
    let arena = Arena::new();
    let root = parse_document(&arena, content, &options());
    let mut tags = BTreeSet::new();
    for node in root.descendants() {
        if let NodeValue::Text(text) = &node.data.borrow().value {
            collect_tags(text, &mut tags);
        }
    }
    tags.into_iter().collect()
}

fn is_tag_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'/'
}

fn collect_tags(text: &str, tags: &mut BTreeSet<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let starts_token = i == 0 || (!is_tag_char(bytes[i - 1]) && bytes[i - 1] != b'#');
            let mut j = i + 1;
            while j < bytes.len() && is_tag_char(bytes[j]) {
                j += 1;
            }
            // Require at least one letter so "#123" (issue refs etc.) isn't a tag.
            let candidate = &text[i + 1..j];
            if starts_token && !candidate.is_empty() && candidate.bytes().any(|b| b.is_ascii_alphabetic()) {
                tags.insert(candidate.to_ascii_lowercase());
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
}

/// First line of the memo, markdown markers stripped, truncated — for titles.
pub fn excerpt(content: &str, max: usize) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let cleaned: String = line
        .chars()
        .filter(|c| !matches!(c, '#' | '*' | '`' | '>' | '_' | '~'))
        .collect();
    let cleaned = cleaned.trim();
    let mut out: String = cleaned.chars().take(max).collect();
    if cleaned.chars().count() > max {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_tags() {
        assert_eq!(extract_tags("hello #work and #life/journal"), vec!["life/journal", "work"]);
    }

    #[test]
    fn skips_code_and_headings() {
        // `#nope` is inside a code span; "# Heading" is a heading marker, not a tag.
        assert_eq!(extract_tags("# Heading\n`#nope` but #yes"), vec!["yes"]);
    }

    #[test]
    fn requires_a_letter() {
        assert!(extract_tags("issue #123").is_empty());
    }

    #[test]
    fn renders_escaped_html() {
        let html = render("<script>alert(1)</script>");
        assert!(!html.contains("<script>"));
    }
}
