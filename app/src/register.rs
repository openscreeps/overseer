//! Client-side rules of the registration, username and password forms. They mirror the
//! server's (`POST /api/register/submit`, `POST /api/register/set-username`) so mistakes show
//! before submitting; the server stays the authority and its error strings are shown as-is.

/// Shortest password the server accepts, in UTF-16 code units (JavaScript `length`).
const PASSWORD_MIN: usize = 8;
/// Longest password the server accepts, in UTF-16 code units.
const PASSWORD_MAX: usize = 1024;

/// Why `username` is rejected: the server rule is `^[a-zA-Z0-9_-]{3,}$`.
pub fn username_error(username: &str) -> Option<&'static str> {
    if username.is_empty() {
        Some("Enter a username")
    } else if !username
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Some("Use only letters, digits, _ and -")
    } else if username.len() < 3 {
        Some("Use at least 3 characters")
    } else {
        None
    }
}

/// Why `email` is rejected: the server rule is `^[\w.+&-]+@[\w.&-]+\.[\w.&-]{2,}$` (ASCII
/// `\w`).
pub fn email_error(email: &str) -> Option<&'static str> {
    let word = |byte: u8, extra: &[u8]| {
        byte.is_ascii_alphanumeric() || byte == b'_' || extra.contains(&byte)
    };
    if email.is_empty() {
        return Some("Enter an e-mail address");
    }
    let valid = email.split_once('@').is_some_and(|(local, domain)| {
        let domain_ok = domain.bytes().all(|byte| word(byte, b".&-"));
        // A dot with at least one character before it and two after it.
        let dot = domain
            .char_indices()
            .any(|(index, c)| c == '.' && index > 0 && domain.len() - index > 2);
        !local.is_empty() && local.bytes().all(|byte| word(byte, b".+&-")) && domain_ok && dot
    });
    (!valid).then_some("Enter a valid e-mail address")
}

/// Why `password` is rejected: 8–1024 characters.
pub fn password_error(password: &str) -> Option<&'static str> {
    let length = password.encode_utf16().count();
    if length < PASSWORD_MIN {
        Some("Use at least 8 characters")
    } else if length > PASSWORD_MAX {
        Some("Use at most 1024 characters")
    } else {
        None
    }
}

/// Why the confirmation does not match `password`.
pub fn confirm_error(password: &str, confirm: &str) -> Option<&'static str> {
    (password != confirm).then_some("The passwords do not match")
}

/// The registration form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registration {
    /// New username.
    pub username: String,
    /// E-mail address.
    pub email: String,
    /// Password.
    pub password: String,
    /// Password again.
    pub confirm: String,
}

/// The rule each field of a [`Registration`] breaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegistrationErrors {
    /// Username error.
    pub username: Option<&'static str>,
    /// E-mail error.
    pub email: Option<&'static str>,
    /// Password error.
    pub password: Option<&'static str>,
    /// Confirmation error.
    pub confirm: Option<&'static str>,
}

impl RegistrationErrors {
    /// No field breaks a rule.
    pub const fn is_empty(&self) -> bool {
        self.username.is_none()
            && self.email.is_none()
            && self.password.is_none()
            && self.confirm.is_none()
    }
}

impl Registration {
    /// Checks every field.
    pub fn errors(&self) -> RegistrationErrors {
        RegistrationErrors {
            username: username_error(&self.username),
            email: email_error(&self.email),
            password: password_error(&self.password),
            confirm: confirm_error(&self.password, &self.confirm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Registration, confirm_error, email_error, password_error, username_error};

    #[test]
    fn usernames_follow_the_server_rule() {
        for good in ["abc", "A_b-9", "___", "player-one"] {
            assert_eq!(username_error(good), None, "{good}");
        }
        for bad in ["", "ab", "a b", "Ünïcode", "name!", "x.y"] {
            assert!(username_error(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn emails_follow_the_server_rule() {
        for good in ["a@b.cd", "first.last+tag@mail.example.org", "a&b@x-y.co"] {
            assert_eq!(email_error(good), None, "{good}");
        }
        for bad in [
            "", "a", "a@", "@b.cd", "a@b", "a@b.c", "a@.cd", "a b@c.de", "a@b c.de", "ä@b.cd",
        ] {
            assert!(email_error(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn passwords_need_8_to_1024_characters() {
        assert!(password_error("1234567").is_some());
        assert_eq!(password_error("12345678"), None);
        assert_eq!(password_error(&"x".repeat(1024)), None);
        assert!(password_error(&"x".repeat(1025)).is_some());
        // Counted like JavaScript's `length`: an astral character is two units.
        assert_eq!(password_error("😀😀😀😀"), None);
        assert!(password_error(&"😀".repeat(513)).is_some());
    }

    #[test]
    fn the_form_reports_each_field() {
        let mut form = Registration {
            username: "player".to_owned(),
            email: "player@example.com".to_owned(),
            password: "long enough".to_owned(),
            confirm: "long enough".to_owned(),
        };
        assert!(form.errors().is_empty());
        form.confirm = "different".to_owned();
        let errors = form.errors();
        assert!(!errors.is_empty());
        assert_eq!(errors.confirm, confirm_error("a", "b"));
        assert_eq!(errors.username, None);
        assert!(Registration::default().errors().username.is_some());
    }
}
