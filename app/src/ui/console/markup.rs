//! The safe subset of HTML that console output may use: text, `<span style="color: …">`,
//! `<b>`, `<i>` and `<br>`, with character references decoded. Everything else (other tags,
//! other attributes, unmatched closing tags) stays text exactly as written, so output is
//! never injected into the page as HTML: the UI renders the [`Piece`]s as text nodes.

/// A piece of a console line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Text in one style.
    Text {
        /// The text, character references decoded.
        text: String,
        /// Inside `<b>`.
        bold: bool,
        /// Inside `<i>`.
        italic: bool,
        /// The colour of the innermost coloured `<span>`: a CSS colour that passed
        /// [`colour`].
        colour: Option<String>,
    },
    /// `<br>`.
    Break,
}

/// An element open at the current position.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Open {
    Bold,
    Italic,
    /// A `<span>`, with its colour.
    Span(Option<String>),
}

/// A tag of the subset.
enum Tag {
    Open(Open),
    Close(&'static str),
    Break,
}

/// Splits `html` into styled text and line breaks (see the module docs). Elements left open
/// end with the line.
pub fn parse(html: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    let mut text = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&decode(&rest[..start]));
        let candidate = &rest[start..];
        let Some(end) = candidate.find('>') else {
            text.push_str(candidate);
            rest = "";
            break;
        };
        let raw = &candidate[..=end];
        rest = &candidate[end + 1..];
        let tag = tag(&raw[1..raw.len() - 1]);
        let applies = match &tag {
            Some(Tag::Close(name)) => open.iter().any(|element| element_name(element) == *name),
            Some(Tag::Open(_) | Tag::Break) => true,
            None => false,
        };
        if !applies {
            text.push_str(raw);
            continue;
        }
        flush(&mut pieces, &mut text, &open);
        match tag {
            Some(Tag::Open(element)) => open.push(element),
            Some(Tag::Close(name)) => {
                // Closes the innermost element of that name and every element opened in it.
                if let Some(index) = open
                    .iter()
                    .rposition(|element| element_name(element) == name)
                {
                    open.truncate(index);
                }
            }
            Some(Tag::Break) => pieces.push(Piece::Break),
            None => {}
        }
    }
    text.push_str(&decode(rest));
    flush(&mut pieces, &mut text, &open);
    pieces
}

/// Ends the current text run in the style of the `open` elements.
fn flush(pieces: &mut Vec<Piece>, text: &mut String, open: &[Open]) {
    if text.is_empty() {
        return;
    }
    let bold = open.contains(&Open::Bold);
    let italic = open.contains(&Open::Italic);
    let colour = open.iter().rev().find_map(|element| match element {
        Open::Span(colour) => colour.clone(),
        Open::Bold | Open::Italic => None,
    });
    pieces.push(Piece::Text {
        text: std::mem::take(text),
        bold,
        italic,
        colour,
    });
}

/// The tag name of an open element.
const fn element_name(element: &Open) -> &'static str {
    match element {
        Open::Bold => "b",
        Open::Italic => "i",
        Open::Span(_) => "span",
    }
}

/// The subset tag between `<` and `>`, if it is one.
fn tag(inner: &str) -> Option<Tag> {
    if let Some(name) = inner.strip_prefix('/') {
        return match name.trim_end().to_ascii_lowercase().as_str() {
            "b" => Some(Tag::Close("b")),
            "i" => Some(Tag::Close("i")),
            "span" => Some(Tag::Close("span")),
            _ => None,
        };
    }
    let inner = inner.strip_suffix('/').unwrap_or(inner);
    let name_end = inner
        .find(|character: char| !character.is_ascii_alphanumeric())
        .unwrap_or(inner.len());
    let (name, attributes) = inner.split_at(name_end);
    let attributes = parse_attributes(attributes)?;
    match name.to_ascii_lowercase().as_str() {
        "br" if attributes.is_empty() => Some(Tag::Break),
        "b" if attributes.is_empty() => Some(Tag::Open(Open::Bold)),
        "i" if attributes.is_empty() => Some(Tag::Open(Open::Italic)),
        "span" => match attributes.as_slice() {
            [] => Some(Tag::Open(Open::Span(None))),
            [(name, Some(style))] if name.eq_ignore_ascii_case("style") => {
                Some(Tag::Open(Open::Span(style_colour(style))))
            }
            _ => None,
        },
        _ => None,
    }
}

/// `name="value"`, `name='value'`, `name=value` and bare `name` attributes, values decoded;
/// `None` when they do not parse.
fn parse_attributes(mut text: &str) -> Option<Vec<(String, Option<String>)>> {
    let mut attributes = Vec::new();
    loop {
        let trimmed = text.trim_start();
        if trimmed.is_empty() {
            return Some(attributes);
        }
        // Attributes are separated by white space.
        if trimmed.len() == text.len() {
            return None;
        }
        let name_end = trimmed
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || character == '-' || character == '_')
            })
            .unwrap_or(trimmed.len());
        if name_end == 0 {
            return None;
        }
        let (name, after) = trimmed.split_at(name_end);
        let after_space = after.trim_start();
        let Some(value) = after_space.strip_prefix('=') else {
            attributes.push((name.to_owned(), None));
            text = after;
            continue;
        };
        let value = value.trim_start();
        let (raw, remaining) = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let body = &value[1..];
                let close = body.find(quote)?;
                (&body[..close], &body[close + 1..])
            }
            Some(_) => {
                let end = value
                    .find(|character: char| character.is_ascii_whitespace())
                    .unwrap_or(value.len());
                value.split_at(end)
            }
            None => return None,
        };
        attributes.push((name.to_owned(), Some(decode(raw))));
        text = remaining;
    }
}

/// The colour a `style` attribute sets, if it sets a valid one; other declarations are
/// ignored.
fn style_colour(style: &str) -> Option<String> {
    style.split(';').rev().find_map(|declaration| {
        let (property, value) = declaration.split_once(':')?;
        property
            .trim()
            .eq_ignore_ascii_case("color")
            .then(|| colour(value.trim()))
            .flatten()
    })
}

/// `value` as a CSS colour the console may use: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`,
/// `rgb()`/`rgba()`/`hsl()`/`hsla()` of numbers, or a name of ASCII letters. Anything else
/// (`url()`, `var()`, expressions, other characters) is `None`.
pub fn colour(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        return (matches!(hex.len(), 3 | 4 | 6 | 8)
            && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(value);
    }
    if let Some((function, arguments)) = value.split_once('(') {
        let numeric = arguments.strip_suffix(')').is_some_and(|arguments| {
            !arguments.is_empty()
                && arguments
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || b" .,%/-deg".contains(&byte))
        });
        return (matches!(function, "rgb" | "rgba" | "hsl" | "hsla") && numeric).then_some(value);
    }
    (!value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_lowercase()))
        .then_some(value)
}

/// Decodes the named references `&amp; &lt; &gt; &quot; &apos; &nbsp;` and numeric `&#NN;` /
/// `&#xNN;`; other `&` stay as written.
pub fn decode(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let reference = &rest[start..];
        let character = reference
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| Some((reference_char(&reference[1..end])?, end)));
        if let Some((character, end)) = character {
            decoded.push(character);
            rest = &reference[end + 1..];
        } else {
            decoded.push('&');
            rest = &reference[1..];
        }
    }
    decoded.push_str(rest);
    decoded
}

/// The character a reference names (between `&` and `;`); NUL and invalid code points are
/// none.
fn reference_char(name: &str) -> Option<char> {
    let code = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        u32::from_str_radix(hex, 16).ok()?
    } else if let Some(decimal) = name.strip_prefix('#') {
        decimal.parse().ok()?
    } else {
        return match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => None,
        };
    };
    char::from_u32(code).filter(|character| *character != '\0')
}

#[cfg(test)]
mod tests {
    use super::{Piece, colour, decode, parse};

    fn text(text: &str) -> Piece {
        Piece::Text {
            text: text.to_owned(),
            bold: false,
            italic: false,
            colour: None,
        }
    }

    fn styled(text: &str, bold: bool, italic: bool, colour: Option<&str>) -> Piece {
        Piece::Text {
            text: text.to_owned(),
            bold,
            italic,
            colour: colour.map(str::to_owned),
        }
    }

    #[test]
    fn plain_text_is_one_piece() {
        assert_eq!(parse("tick 42"), vec![text("tick 42")]);
        assert_eq!(parse(""), Vec::<Piece>::new());
        assert_eq!(parse("a\nb"), vec![text("a\nb")], "new lines are kept");
    }

    #[test]
    fn allowed_tags_style_their_text() {
        assert_eq!(
            parse("a <b>bold</b> <i>it</i><br>next<br/>last<BR />"),
            vec![
                text("a "),
                styled("bold", true, false, None),
                text(" "),
                styled("it", false, true, None),
                Piece::Break,
                text("next"),
                Piece::Break,
                text("last"),
                Piece::Break,
            ]
        );
        assert_eq!(
            parse("<b><i>both</i> b</b>"),
            vec![
                styled("both", true, true, None),
                styled(" b", true, false, None)
            ]
        );
    }

    #[test]
    fn a_bot_warning_line() {
        assert_eq!(
            parse(r#"<span style="color:#e5c07b">warning</span> energy 91"#),
            vec![
                styled("warning", false, false, Some("#e5c07b")),
                text(" energy 91")
            ]
        );
    }

    #[test]
    fn spans_set_colours() {
        assert_eq!(
            parse(
                r#"<span style="color: #F00">red</span> <span style='color:rgb(0, 128, 0)'>green <b>bold</b></span>"#
            ),
            vec![
                styled("red", false, false, Some("#f00")),
                text(" "),
                styled("green ", false, false, Some("rgb(0, 128, 0)")),
                styled("bold", true, false, Some("rgb(0, 128, 0)")),
            ]
        );
        assert_eq!(
            parse(
                r#"<span style="color:orange"><span>inner</span> <span style="color:#abcdef">own</span></span>"#
            ),
            vec![
                styled("inner", false, false, Some("orange")),
                styled(" ", false, false, Some("orange")),
                styled("own", false, false, Some("#abcdef")),
            ],
            "the innermost colour wins"
        );
        assert_eq!(
            parse(r#"<span style="font-weight: bold; color: lime;">x</span>"#),
            vec![styled("x", false, false, Some("lime"))],
            "other declarations are ignored"
        );
        assert_eq!(
            parse(r#"<span style="color: url(javascript:alert(1))">x</span>"#),
            vec![text("x")],
            "an unsafe colour leaves the span uncoloured"
        );
    }

    #[test]
    fn colours_are_validated() {
        for valid in [
            "#fff",
            "#ffff",
            "#a0b1c2",
            "#A0B1C2FF",
            "rgb(1,2,3)",
            "rgba(1, 2, 3, 0.5)",
            "hsl(120deg 50% 50%)",
            "Red",
        ] {
            assert!(colour(valid).is_some(), "{valid}");
        }
        for invalid in [
            "",
            "#ff",
            "#ggg",
            "#fffff",
            "url(x)",
            "var(--x)",
            "expression(alert(1))",
            "red;background:url(x)",
            "rgb(1,2,3",
            "calc(1)",
            "red blue",
            "\"red\"",
        ] {
            assert_eq!(colour(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            decode("&lt;b&gt; &amp; &quot;q&quot; &apos; &#65;&#x42;&#X43; &nbsp;"),
            "<b> & \"q\" ' ABC \u{a0}"
        );
        assert_eq!(
            decode("a & b &unknown; &#0; &#xZZ; &"),
            "a & b &unknown; &#0; &#xZZ; &"
        );
        assert_eq!(
            parse("&lt;script&gt; <b>&amp;</b>"),
            vec![text("<script> "), styled("&", true, false, None)],
            "decoded references never become tags"
        );
    }

    #[test]
    fn malicious_input_stays_text() {
        for html in [
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(1)>",
            r#"<a href="javascript:alert(1)">x</a>"#,
            r#"<span onclick="alert(1)">x</span>"#,
            r#"<span style="color:red" onmouseover="alert(1)">x</span>"#,
            r#"<b class="x">x</b>"#,
            "<iframe src=//evil></iframe>",
            "<svg/onload=alert(1)>",
            "<style>body{display:none}</style>",
        ] {
            let pieces = parse(html);
            let joined: String = pieces
                .iter()
                .map(|piece| match piece {
                    Piece::Text {
                        text,
                        bold,
                        italic,
                        colour,
                    } => {
                        assert!(!bold && !italic && colour.is_none(), "{html}: {piece:?}");
                        text.as_str()
                    }
                    Piece::Break => panic!("{html}: a break"),
                })
                .collect();
            assert_eq!(joined, html, "kept as written");
        }
    }

    #[test]
    fn unmatched_and_unclosed_tags() {
        assert_eq!(
            parse("a</b>c"),
            vec![text("a</b>c")],
            "a stray closing tag is text"
        );
        assert_eq!(
            parse("<b>open"),
            vec![styled("open", true, false, None)],
            "open elements end with the line"
        );
        assert_eq!(
            parse("<b>x<i>y</b>z"),
            vec![
                styled("x", true, false, None),
                styled("y", true, true, None),
                text("z")
            ],
            "closing an outer element closes the inner ones"
        );
        assert_eq!(parse("1 < 2 > 0"), vec![text("1 < 2 > 0")]);
        assert_eq!(
            parse("a <b"),
            vec![text("a <b")],
            "an unterminated tag is text"
        );
    }
}
