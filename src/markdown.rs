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

/// One attachment available for inline substitution.
pub struct InlineAttachment {
    pub uid: String,
    pub filename: String,
    pub is_image: bool,
}

/// One note-to-note link, resolved from a `{{memo:UUID}}` token, ready for inline
/// substitution. `viewable` is false when the target exists but the current viewer
/// may not see it (the title is then withheld and the link renders as a locked chip).
pub struct InlineMemoLink {
    pub uuid: String,
    /// The target note's current short uid, used to build its `/m/{uid}` permalink.
    pub uid: String,
    pub title: String,
    pub viewable: bool,
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// True for the base62 uid characters produced by `auth::new_uid`.
fn is_uid_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// True for the characters of a UUIDv4 string produced by `auth::new_uuid`.
fn is_uuid_char(b: u8) -> bool {
    b.is_ascii_hexdigit() || b == b'-'
}

/// Collect the note uuids referenced by `{{memo:UUID}}` tokens, in order,
/// de-duplicated. Empty `{{memo}}` placeholders carry no uuid and are ignored.
pub fn extract_memo_refs(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let needle = b"{{memo:";
    let mut i = 0;
    while let Some(pos) = find(&bytes[i..], needle) {
        let start = i + pos + needle.len();
        let mut j = start;
        while j < bytes.len() && is_uuid_char(bytes[j]) {
            j += 1;
        }
        if j > start && bytes.get(j) == Some(&b'}') && bytes.get(j + 1) == Some(&b'}') {
            let uuid = content[start..j].to_string();
            if !out.contains(&uuid) {
                out.push(uuid);
            }
        }
        i = j.max(start);
    }
    out
}

/// Collect the attachment uids referenced by `{{attach:UID}}` tokens, in order,
/// de-duplicated. Empty `{{attach}}` placeholders carry no uid and are ignored.
pub fn extract_attachment_refs(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let needle = b"{{attach:";
    let mut i = 0;
    while let Some(pos) = find(&bytes[i..], needle) {
        let start = i + pos + needle.len();
        let mut j = start;
        while j < bytes.len() && is_uid_char(bytes[j]) {
            j += 1;
        }
        if j > start && bytes.get(j) == Some(&b'}') && bytes.get(j + 1) == Some(&b'}') {
            let uid = content[start..j].to_string();
            if !out.contains(&uid) {
                out.push(uid);
            }
        }
        i = j.max(start);
    }
    out
}

/// Render memo markdown, then substitute both attachment (`{{attach:UID}}`) and
/// note-link (`{{memo:UUID}}`) tokens with their inline HTML. Only tokens present
/// in `atts` / `links` are substituted; any leftover tokens are stripped so stray
/// or in-flight placeholders never show as raw text.
pub fn render_with_inlines(content: &str, atts: &[InlineAttachment], links: &[InlineMemoLink]) -> String {
    let html = substitute_attachments(render(content), atts);
    substitute_memo_links(html, links)
}

/// Render with only attachment tokens substituted (no note links).
pub fn render_with_attachments(content: &str, atts: &[InlineAttachment]) -> String {
    render_with_inlines(content, atts, &[])
}

/// Replace `{{attach:UID}}` tokens (images inline, other files as download chips),
/// then strip any leftover attachment tokens.
fn substitute_attachments(mut html: String, atts: &[InlineAttachment]) -> String {
    for a in atts {
        let token = format!("{{{{attach:{}}}}}", a.uid);
        let replacement = if a.is_image {
            format!(
                "<a href=\"/r/{uid}\" target=\"_blank\"><img class=\"inline-attachment\" src=\"/r/{uid}\" alt=\"{alt}\" loading=\"lazy\"></a>",
                uid = a.uid,
                alt = escape_html(&a.filename),
            )
        } else {
            format!(
                "<a class=\"chip\" href=\"/r/{uid}\" target=\"_blank\">\u{1F4CE} {name}</a>",
                uid = a.uid,
                name = escape_html(&a.filename),
            )
        };
        html = html.replace(&token, &replacement);
    }
    strip_attachment_tokens(&html)
}

/// Replace `{{memo:UUID}}` tokens with note-link chips, then turn any leftover
/// (unresolvable — deleted, or another user's note) memo tokens into a broken-link
/// placeholder so they never show as raw text.
fn substitute_memo_links(mut html: String, links: &[InlineMemoLink]) -> String {
    for l in links {
        let token = format!("{{{{memo:{}}}}}", l.uuid);
        let replacement = if l.viewable {
            format!(
                "<a class=\"memo-link\" href=\"/m/{uid}\">{title}</a>",
                uid = l.uid,
                title = escape_html(&l.title),
            )
        } else {
            // Target exists but isn't visible to this viewer: no title, no href.
            "<span class=\"memo-link memo-link-locked\">\u{1F512} note</span>".to_string()
        };
        html = html.replace(&token, &replacement);
    }
    strip_memo_tokens(&html)
}

/// Remove any remaining `{{attach:...}}` or `{{attach}}` tokens from rendered HTML.
fn strip_attachment_tokens(html: &str) -> String {
    let bytes = html.as_bytes();
    let needle = b"{{attach";
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if html[i..].as_bytes().starts_with(needle) {
            // Drop through the closing `}}` if present, else emit literally.
            if let Some(rel) = find(&bytes[i..], b"}}") {
                i += rel + 2;
                continue;
            }
        }
        let ch = html[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Replace any remaining `{{memo:...}}` or `{{memo}}` tokens in rendered HTML with
/// a broken-link placeholder (the target was deleted or isn't the author's note).
fn strip_memo_tokens(html: &str) -> String {
    let bytes = html.as_bytes();
    let needle = b"{{memo";
    let broken = "<span class=\"memo-link memo-link-broken\">\u{26A0} missing note</span>";
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if html[i..].as_bytes().starts_with(needle) {
            if let Some(rel) = find(&bytes[i..], b"}}") {
                out.push_str(broken);
                i += rel + 2;
                continue;
            }
        }
        let ch = html[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// First index of `needle` within `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&k| &haystack[k..k + needle.len()] == needle)
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
    fn line_start_tag_is_a_tag_not_a_heading() {
        // The composer seeds notes with "\n\n#username"; without a space after
        // '#' it's a tag, not an ATX heading. Both must hold for that to work.
        assert_eq!(extract_tags("a thought\n\n#jbudding"), vec!["jbudding"]);
        assert!(!render("a thought\n\n#jbudding").contains("<h1"));
    }

    #[test]
    fn renders_escaped_html() {
        let html = render("<script>alert(1)</script>");
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn extracts_attachment_refs_in_order() {
        let refs = extract_attachment_refs("a {{attach:AbC123}} b {{attach:zzz999}} {{attach:AbC123}}");
        assert_eq!(refs, vec!["AbC123", "zzz999"]);
        // Empty placeholder carries no uid.
        assert!(extract_attachment_refs("text {{attach}} more").is_empty());
    }

    #[test]
    fn extracts_memo_refs_in_order() {
        let a = "11111111-1111-4111-8111-111111111111";
        let b = "22222222-2222-4222-8222-222222222222";
        let refs = extract_memo_refs(&format!("see {{{{memo:{a}}}}} and {{{{memo:{b}}}}} again {{{{memo:{a}}}}}"));
        assert_eq!(refs, vec![a, b]);
        // Empty placeholder carries no uuid.
        assert!(extract_memo_refs("text {{memo}} more").is_empty());
    }

    #[test]
    fn substitutes_memo_links_locked_and_broken() {
        let known = "11111111-1111-4111-8111-111111111111";
        let hidden = "22222222-2222-4222-8222-222222222222";
        let links = vec![
            InlineMemoLink { uuid: known.into(), uid: "abc123".into(), title: "My note".into(), viewable: true },
            InlineMemoLink { uuid: hidden.into(), uid: "def456".into(), title: String::new(), viewable: false },
        ];
        let html = render_with_inlines(
            &format!("a {{{{memo:{known}}}}}\n\nb {{{{memo:{hidden}}}}}\n\nc {{{{memo:33333333-3333-4333-8333-333333333333}}}}"),
            &[],
            &links,
        );
        assert!(html.contains("<a class=\"memo-link\" href=\"/m/abc123\">My note</a>"));
        assert!(html.contains("memo-link-locked"));
        // Unknown target becomes a broken placeholder, never raw token text.
        assert!(html.contains("memo-link-broken"));
        assert!(!html.contains("{{memo"));
    }

    #[test]
    fn substitutes_known_tokens_and_strips_others() {
        let atts = vec![
            InlineAttachment { uid: "img0000000001".into(), filename: "p.png".into(), is_image: true },
            InlineAttachment { uid: "file000000001".into(), filename: "d.pdf".into(), is_image: false },
        ];
        let html = render_with_attachments(
            "top\n\n{{attach:img0000000001}}\n\nmid {{attach:file000000001}}\n\n{{attach:unknown00000}}",
            &atts,
        );
        assert!(html.contains("<img class=\"inline-attachment\" src=\"/r/img0000000001\""));
        assert!(html.contains("href=\"/r/file000000001\""));
        // Unknown / leftover tokens are removed, not shown raw.
        assert!(!html.contains("{{attach"));
    }
}
