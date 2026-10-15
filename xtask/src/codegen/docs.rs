//! Rustdoc text from reference descriptions.
//!
//! Descriptions are Markdown written for API readers. Rustdoc and the strict lints read more
//! into them than intended: identifiers outside code spans (`clippy::doc_markdown`), bare URLs
//! (`rustdoc::bare_urls`), `<placeholders>` (`rustdoc::invalid_html_tags`), `[brackets]`
//! (`rustdoc::broken_intra_doc_links`) and unbalanced backticks
//! (`rustdoc::unescaped_backticks`). This module rewrites prose so it renders as written.

/// Words the `doc_markdown` lint accepts although they look like identifiers.
const PROSE_WORDS: &[&str] = &[
    "GitHub",
    "JavaScript",
    "TypeScript",
    "WebSocket",
    "WebSockets",
    "OAuth",
    "NaN",
    "IPv4",
    "IPv6",
];

/// The doc comment lines (without `///`) for `text`.
pub fn lines(text: &str) -> Vec<String> {
    let text = text.trim();
    let balanced = text.matches('`').count().is_multiple_of(2);
    let mut out = String::with_capacity(text.len() + 16);
    let mut in_code = false;
    let mut prose = String::new();
    for c in text.chars() {
        if c == '`' && balanced {
            if !in_code {
                out.push_str(&rewrite_prose(&prose));
                prose.clear();
            }
            in_code = !in_code;
            out.push('`');
        } else if c == '`' {
            prose.push_str("\\`");
        } else if in_code {
            out.push(c);
        } else {
            prose.push(c);
        }
    }
    out.push_str(&rewrite_prose(&prose));
    out.lines().map(|line| line.trim_end().to_owned()).collect()
}

/// Rewrites one prose run (text outside code spans), keeping its whitespace.
fn rewrite_prose(prose: &str) -> String {
    let mut out = String::with_capacity(prose.len());
    let mut word = String::new();
    for c in prose.chars() {
        if c.is_whitespace() {
            out.push_str(&rewrite_word(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&rewrite_word(&word));
    out
}

/// Rewrites one whitespace-delimited word: URLs become autolinks, identifiers code spans, and
/// Markdown/HTML metacharacters are escaped.
fn rewrite_word(word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    let core_start = word
        .find(|c: char| c.is_ascii_alphanumeric() || c == '_')
        .unwrap_or(word.len());
    let core_end = word
        .rfind(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '/')
        .map_or(core_start, |index| index + 1)
        .max(core_start);
    let (lead, rest) = word.split_at(core_start);
    let (core, trail) = rest.split_at(core_end - core_start);
    if core.starts_with("http://") || core.starts_with("https://") {
        return format!("{}<{core}>{}", escape(lead), escape(trail));
    }
    if is_identifier_like(core) {
        return format!("{}`{core}`{}", escape(lead), escape(trail));
    }
    escape(word)
}

/// Whether `core` reads as code: it has `_` or `::`, or camel case such as `historyChunkSize`
/// and `SteamID` (but not prose words such as `JavaScript`).
fn is_identifier_like(core: &str) -> bool {
    if core.contains("::") {
        return true;
    }
    let simple = core.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !simple || PROSE_WORDS.contains(&core) {
        return false;
    }
    if core.contains('_') {
        return core.chars().any(|c| c.is_ascii_alphanumeric());
    }
    let uppercase_after_first = core.chars().skip(1).any(|c| c.is_ascii_uppercase());
    let lowercase = core.chars().any(|c| c.is_ascii_lowercase());
    uppercase_after_first && lowercase
}

/// Escapes characters Markdown or rustdoc would interpret in prose.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '[' | ']' | '<' | '>' | '*' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::lines;

    #[test]
    fn identifiers_urls_and_markup_are_made_literal() {
        assert_eq!(
            lines("Reads historyChunkSize (see https://screeps.com/a/.) for <room> [0, 1]."),
            [r"Reads `historyChunkSize` (see <https://screeps.com/a/>.) for \<room\> \[0, 1\]."]
        );
        assert_eq!(
            lines("A `snake_case` code span, SteamID and JavaScript, ranged_attack."),
            ["A `snake_case` code span, `SteamID` and JavaScript, `ranged_attack`."]
        );
    }

    #[test]
    fn unbalanced_backticks_are_escaped_and_lines_kept() {
        assert_eq!(lines("one ` tick\n\nnext"), [r"one \` tick", "", "next"]);
    }
}
