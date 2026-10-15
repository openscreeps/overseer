//! Rust identifiers for JSON names.

/// Words of a JSON or camel-case name: split at non-alphanumeric characters, at lower-to-upper
/// case changes and before the last capital of an acronym (`XMLHttp` → `XML`, `Http`). Digits
/// stay with the word they follow.
pub fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    for part in name.split(|c: char| !c.is_ascii_alphanumeric()) {
        let chars: Vec<char> = part.chars().collect();
        let mut word = String::new();
        for (index, &c) in chars.iter().enumerate() {
            let boundary = index > 0 && c.is_ascii_uppercase() && {
                let previous = chars[index - 1];
                let next_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
                previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase() && next_lower)
            };
            if boundary && !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            word.push(c);
        }
        if !word.is_empty() {
            words.push(word);
        }
    }
    words
}

/// `PascalCase` form of `name`.
pub fn pascal(name: &str) -> String {
    words(name)
        .iter()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
            })
        })
        .collect()
}

/// `snake_case` form of `name`.
pub fn snake(name: &str) -> String {
    words(name)
        .iter()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// Keywords of Rust 2024, including reserved ones.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// A field identifier for the JSON property `name`: `snake_case`, raw for keywords, prefixed
/// when it would start with a digit.
pub fn field(name: &str) -> String {
    let snake = snake(name);
    if snake.is_empty() {
        "field".to_owned()
    } else if snake.starts_with(|c: char| c.is_ascii_digit()) {
        format!("field_{snake}")
    } else if matches!(snake.as_str(), "self" | "super" | "crate") {
        format!("{snake}_")
    } else if KEYWORDS.contains(&snake.as_str()) {
        format!("r#{snake}")
    } else {
        snake
    }
}

/// A type or variant identifier for `name`: `PascalCase`, prefixed when it would start with a
/// digit.
pub fn type_name(name: &str) -> String {
    let pascal = pascal(name);
    if pascal.is_empty() {
        "Empty".to_owned()
    } else if pascal.starts_with(|c: char| c.is_ascii_digit()) {
        format!("V{pascal}")
    } else if pascal == "Self" {
        "Self_".to_owned()
    } else {
        pascal
    }
}

#[cfg(test)]
mod tests {
    use super::{field, pascal, snake, type_name};

    #[test]
    fn camel_case_and_acronyms_split_into_words() {
        assert_eq!(snake("historyChunkSize"), "history_chunk_size");
        assert_eq!(snake("XMLHttpRequest"), "xml_http_request");
        assert_eq!(snake("energyHarvested8"), "energy_harvested8");
        assert_eq!(snake("ranged_attack"), "ranged_attack");
        assert_eq!(snake("X-Server-Password"), "x_server_password");
        assert_eq!(pascal("gameRoomTerrain"), "GameRoomTerrain");
        assert_eq!(pascal("ranged_attack"), "RangedAttack");
    }

    #[test]
    fn fields_avoid_keywords_and_leading_digits() {
        assert_eq!(field("_id"), "id");
        assert_eq!(field("type"), "r#type");
        assert_eq!(field("self"), "self_");
        assert_eq!(field("0"), "field_0");
        assert_eq!(field("steamTicket"), "steam_ticket");
        assert_eq!(type_name("1"), "V1");
        assert_eq!(type_name("ranged_attack"), "RangedAttack");
        assert_eq!(type_name(""), "Empty");
    }
}
