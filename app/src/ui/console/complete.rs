//! Console autocomplete: the identifier chain before the caret (`Game.creeps.Har`, `FIND_SO`)
//! and the ranked names that complete it.
//!
//! The names come from the API [`catalog`], from the signed-in user's game ([`GameNames`]: own
//! creeps, spawns and rooms; and the keys of `Memory`), and from earlier commands (whole
//! commands that start with the input). A dot after a known object narrows the names to its
//! members.
//! Matches that start with the typed text (same case) come first, then those that start with
//! it in any case, then those that contain it.
//!
//! Offsets are UTF-16 code units, as `selectionStart` of a text field counts them.

mod catalog;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use overseer::console::GameNames;
use serde_json::Value;

use self::catalog::{Type, Value as ConstantValue};

/// Most suggestions in a list.
pub const LIMIT: usize = 8;
/// Most earlier commands in a list.
const HISTORY_LIMIT: usize = 3;
/// Most segments of a chain (`a.b.c` has 2 before the word).
const MAX_SEGMENTS: usize = 16;

/// One suggestion: replace `start..end` (UTF-16 offsets) of the input with `insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// What the list shows.
    pub label: String,
    /// What the name is (its kind, a constant's value), shown after the label.
    pub detail: String,
    /// The text that replaces the range.
    pub insert: String,
    /// The start of the range.
    pub start: usize,
    /// The end of the range (the caret).
    pub end: usize,
}

impl Suggestion {
    /// `text` with the suggestion applied, and the caret after the inserted text (UTF-16).
    pub fn apply(&self, text: &str) -> (String, usize) {
        let start = byte_offset(text, self.start);
        let end = byte_offset(text, self.end).max(start);
        let mut applied = String::with_capacity(text.len() + self.insert.len());
        applied.push_str(&text[..start]);
        applied.push_str(&self.insert);
        applied.push_str(&text[end..]);
        (applied, self.start + utf16_len(&self.insert))
    }
}

/// The names that the completion would use if they were loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// Own creeps, spawns or rooms.
    pub objects: bool,
    /// `Memory`.
    pub memory: bool,
}

/// The suggestions for a caret, best first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completion {
    /// At most [`LIMIT`].
    pub suggestions: Vec<Suggestion>,
    /// The names it asked for.
    pub needs: Needs,
}

/// The suggestions for `text` with the caret at `caret` (UTF-16 offset), from `names`,
/// `memory` (all of `Memory`; `None` before it was read) and `history` (earlier commands,
/// oldest first).
pub fn complete(
    text: &str,
    caret: usize,
    names: &GameNames,
    memory: Option<&Value>,
    history: &[String],
) -> Completion {
    let at = byte_offset(text, caret);
    let mut completion = Completion::default();
    // Earlier commands complete the whole input, with the caret at its end.
    let commands: Vec<&String> = if at == text.len() && !text.trim().is_empty() {
        let mut commands: Vec<&String> = Vec::new();
        for command in history.iter().rev() {
            if command.len() > text.len()
                && command.starts_with(text)
                && !commands.contains(&command)
            {
                commands.push(command);
                if commands.len() == HISTORY_LIMIT {
                    break;
                }
            }
        }
        commands
    } else {
        Vec::new()
    };
    if let Some(chain) = chain(text, at) {
        let candidates = candidates(&chain, names, memory, &mut completion.needs);
        let room = LIMIT - commands.len();
        completion.suggestions = rank(chain.word, candidates, room)
            .into_iter()
            .map(|candidate| suggestion(text, &chain, at, candidate))
            .collect();
    }
    let end = utf16_len(text);
    completion
        .suggestions
        .extend(commands.into_iter().map(|command| Suggestion {
            label: command.clone(),
            detail: "history".to_owned(),
            insert: command.clone(),
            start: 0,
            end,
        }));
    completion
}

/// The byte offset of the UTF-16 offset `utf16` in `text`: the start of the character it
/// falls in, the end of `text` past it.
pub fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        let next = units + character.len_utf16();
        if next > utf16 {
            return byte;
        }
        units = next;
    }
    text.len()
}

/// The length of `text` in UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The dot before the word: where it starts, and whether it is optional (`?.`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Access {
    /// Byte offset of the `.` (or of the `?` of `?.`).
    start: usize,
    /// `?.`.
    optional: bool,
}

/// An identifier chain before the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Chain<'a> {
    /// The objects before the word, outermost first (`["Game", "creeps"]`).
    segments: Vec<&'a str>,
    /// The part of a name before the caret (can be empty after a dot).
    word: &'a str,
    /// Byte offset of the word.
    word_start: usize,
    /// The dot before the word, if any.
    access: Option<Access>,
}

/// The identifier chain that ends at byte `at` of `text`; `None` in a string or a comment,
/// after a number, or after an object that is not a plain chain (`f().x`).
fn chain(text: &str, at: usize) -> Option<Chain<'_>> {
    let before = &text[..at];
    if !in_code(before) {
        return None;
    }
    let word_start = identifier_start(before);
    let word = &before[word_start..];
    if word.starts_with(|character: char| character.is_ascii_digit()) {
        return None;
    }
    let access = access_before(&before[..word_start]);
    let mut segments = Vec::new();
    // The end of the next segment: before a dot, or right before a bracket segment.
    let mut end = access.map(|access| access.start);
    while let Some(at) = end {
        let (segment, start, bracketed) = segment_before(&before[..at])?;
        segments.push(segment);
        if segments.len() > MAX_SEGMENTS {
            return None;
        }
        let rest = &before[..start];
        end = if bracketed {
            Some(rest.strip_suffix("?.").map_or(start, str::len))
        } else {
            access_before(rest).map(|access| access.start)
        };
    }
    segments.reverse();
    Some(Chain {
        segments,
        word,
        word_start,
        access,
    })
}

/// Whether the end of `before` is code: not in a string, a template or a comment.
fn in_code(before: &str) -> bool {
    /// Where the scan is.
    enum State {
        /// Code.
        Code,
        /// In a string or template that this quote ends.
        Quoted(char),
        /// In a `//` comment.
        Line,
        /// In a `/* */` comment.
        Block,
    }
    let mut state = State::Code;
    let mut characters = before.chars().peekable();
    while let Some(character) = characters.next() {
        state = match state {
            State::Code => match character {
                '\'' | '"' | '`' => State::Quoted(character),
                '/' if characters.next_if_eq(&'/').is_some() => State::Line,
                '/' if characters.next_if_eq(&'*').is_some() => State::Block,
                _ => State::Code,
            },
            State::Quoted(quote) => {
                if character == '\\' {
                    characters.next();
                    State::Quoted(quote)
                } else if character == quote || (character == '\n' && quote != '`') {
                    State::Code
                } else {
                    State::Quoted(quote)
                }
            }
            State::Line if character == '\n' => State::Code,
            State::Block if character == '*' && characters.next_if_eq(&'/').is_some() => {
                State::Code
            }
            state @ (State::Line | State::Block) => state,
        };
    }
    matches!(state, State::Code)
}

/// Whether `character` can be part of an identifier.
fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

/// Whether `name` is an identifier (so `.name` reaches it).
fn is_identifier(name: &str) -> bool {
    name.chars().all(is_identifier_char)
        && name
            .chars()
            .next()
            .is_some_and(|first| !first.is_ascii_digit())
}

/// The byte offset where the identifier characters at the end of `text` start.
fn identifier_start(text: &str) -> usize {
    text.char_indices()
        .rev()
        .take_while(|&(_, character)| is_identifier_char(character))
        .last()
        .map_or(text.len(), |(start, _)| start)
}

/// The `.` or `?.` at the end of `text` (not part of `..` or `...`).
fn access_before(text: &str) -> Option<Access> {
    let rest = text.strip_suffix('.')?;
    if rest.ends_with('.') {
        return None;
    }
    Some(match rest.strip_suffix('?') {
        Some(optional) => Access {
            start: optional.len(),
            optional: true,
        },
        None => Access {
            start: rest.len(),
            optional: false,
        },
    })
}

/// The segment at the end of `text` (an identifier, or a quoted name in brackets), its byte
/// offset, and whether it is in brackets.
fn segment_before(text: &str) -> Option<(&str, usize, bool)> {
    if let Some(rest) = text.strip_suffix(']') {
        let quote = rest.chars().next_back()?;
        if quote != '\'' && quote != '"' {
            return None;
        }
        let inner_end = rest.len() - 1;
        let open = rest[..inner_end].rfind(quote)?;
        let bracket = rest[..open].strip_suffix('[')?;
        return Some((&rest[open + 1..inner_end], bracket.len(), true));
    }
    let start = identifier_start(text);
    let segment = &text[start..];
    (!segment.is_empty() && is_identifier(segment)).then_some((segment, start, false))
}

/// What a chain's segments reach.
enum Target<'a> {
    /// The global scope (no segments).
    Global,
    /// A value of a type of the catalog.
    Typed(Type),
    /// A value in `Memory` (`None`: not loaded yet).
    Memory(Option<&'a Value>),
    /// Something the completion knows nothing about.
    Unknown,
}

/// The value that `segments` reach, with `memory` as `Memory`.
fn resolve<'a>(segments: &[&str], memory: Option<&'a Value>, needs: &mut Needs) -> Target<'a> {
    let mut target = Target::Global;
    for segment in segments {
        target = match target {
            Target::Global => catalog::GLOBALS
                .iter()
                .find(|member| member.name == *segment)
                .map_or(Target::Unknown, |member| match member.of {
                    Type::Memory => Target::Memory(memory),
                    of => Target::Typed(of),
                }),
            Target::Typed(of) => catalog::element(of)
                .or_else(|| {
                    catalog::members(of)
                        .iter()
                        .find(|member| member.name == *segment)
                        .map(|member| member.of)
                })
                .map_or(Target::Unknown, Target::Typed),
            Target::Memory(Some(value)) => value
                .get(*segment)
                .filter(|value| value.is_object())
                .map_or(Target::Unknown, |value| Target::Memory(Some(value))),
            target @ (Target::Memory(None) | Target::Unknown) => target,
        };
    }
    if let Target::Memory(_) = target {
        needs.memory = true;
    }
    target
}

/// What a candidate is.
#[derive(Clone, Copy)]
enum Detail {
    /// A word (`method`, `creep`).
    Word(&'static str),
    /// A constant's value.
    Value(ConstantValue),
}

impl Detail {
    /// The text shown after the name.
    fn text(self) -> String {
        match self {
            Self::Word(word) => word.to_owned(),
            Self::Value(ConstantValue::Number(number)) => number.to_string(),
            Self::Value(ConstantValue::Text(text)) => format!("\"{text}\""),
        }
    }
}

/// A name that may complete the word.
#[derive(Clone, Copy)]
struct Candidate<'a> {
    /// The name.
    name: &'a str,
    /// What it is.
    detail: Detail,
}

/// Every name the chain's object has (before ranking).
fn candidates<'a>(
    chain: &Chain<'_>,
    names: &'a GameNames,
    memory: Option<&'a Value>,
    needs: &mut Needs,
) -> Vec<Candidate<'a>> {
    match resolve(&chain.segments, memory, needs) {
        // A list after every space would be noise: the global scope needs a word.
        Target::Global if chain.word.is_empty() => Vec::new(),
        Target::Global => catalog::GLOBALS
            .iter()
            .map(|member| Candidate {
                name: member.name,
                detail: Detail::Word(member.kind.label()),
            })
            .chain(catalog::CONSTANTS.iter().map(|constant| Candidate {
                name: constant.name,
                detail: Detail::Value(constant.value),
            }))
            .collect(),
        Target::Typed(of) => {
            let (dynamic, detail): (&[String], &'static str) = match of {
                Type::Creeps => (&names.creeps, "creep"),
                Type::Spawns => (&names.spawns, "spawn"),
                Type::Rooms => (&names.rooms, "room"),
                _ => (&[], ""),
            };
            if !detail.is_empty() {
                needs.objects = true;
            }
            dynamic
                .iter()
                .map(|name| Candidate {
                    name,
                    detail: Detail::Word(detail),
                })
                .chain(catalog::members(of).iter().map(|member| Candidate {
                    name: member.name,
                    detail: Detail::Word(member.kind.label()),
                }))
                .collect()
        }
        Target::Memory(Some(Value::Object(map))) => map
            .iter()
            .map(|(key, value)| Candidate {
                name: key,
                detail: Detail::Word(json_kind(value)),
            })
            .collect(),
        Target::Memory(_) | Target::Unknown => Vec::new(),
    }
}

/// The kind of a JSON value, as the list shows it.
const fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The best `limit` candidates that match `word`, best first, each name once: those that
/// start with it (same case), then those that start with it in any case, then (for 2
/// characters or more) those that contain it in any case. In each group the order of
/// `candidates` stays. A name equal to the word is no suggestion.
fn rank<'a>(word: &str, candidates: Vec<Candidate<'a>>, limit: usize) -> Vec<Candidate<'a>> {
    let lower = word.to_lowercase();
    let mut ranked: Vec<(u8, Candidate<'a>)> = candidates
        .into_iter()
        .filter(|candidate| candidate.name != word)
        .filter_map(|candidate| {
            let group = if candidate.name.starts_with(word) {
                0
            } else {
                let name = candidate.name.to_lowercase();
                if name.starts_with(&lower) {
                    1
                } else if lower.chars().nth(1).is_some() && name.contains(&lower) {
                    2
                } else {
                    return None;
                }
            };
            Some((group, candidate))
        })
        .collect();
    ranked.sort_by_key(|&(group, _)| group);
    let mut seen = HashSet::new();
    ranked
        .into_iter()
        .map(|(_, candidate)| candidate)
        .filter(|candidate| seen.insert(candidate.name))
        .take(limit)
        .collect()
}

/// The suggestion of `candidate` for `chain` in `text` with the caret at byte `at`. A name
/// that is no identifier goes in brackets (`['my creep']`) in place of the dot.
fn suggestion(text: &str, chain: &Chain<'_>, at: usize, candidate: Candidate<'_>) -> Suggestion {
    let (start, insert) = match chain.access {
        Some(access) if !is_identifier(candidate.name) => {
            let quoted = candidate.name.replace('\\', "\\\\").replace('\'', "\\'");
            let optional = if access.optional { "?." } else { "" };
            (access.start, format!("{optional}['{quoted}']"))
        }
        _ => (chain.word_start, candidate.name.to_owned()),
    };
    Suggestion {
        label: candidate.name.to_owned(),
        detail: candidate.detail.text(),
        insert,
        start: utf16_len(&text[..start]),
        end: utf16_len(&text[..at]),
    }
}
