//! The rules of a new account's username, e-mail address and password: those of `OpenScreeps`
//! (`POST /api/register/submit`) and, for the username, of every contract's
//! `POST /api/register/set-username`. A client checks them before it sends a value, so that a
//! mistake shows before the request; the server stays the authority.

use super::Registration;

/// Fewest characters of a username.
pub const USERNAME_MIN: usize = 3;
/// Fewest UTF-16 code units of a password (JavaScript `length`).
pub const PASSWORD_MIN_UNITS: usize = 8;
/// Most UTF-16 code units of a password.
pub const PASSWORD_MAX_UNITS: usize = 1024;

/// Why a username breaks the rule `^[a-zA-Z0-9_-]{3,}$`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
pub enum UsernameError {
    /// The username is empty.
    #[error("the username is empty")]
    Empty,
    /// A character is not an ASCII letter, a digit, `_` or `-`.
    #[error("the username has a character other than letters, digits, _ and -")]
    InvalidCharacter,
    /// Fewer than [`USERNAME_MIN`] characters.
    #[error("the username is shorter than 3 characters")]
    TooShort,
}

/// Why an e-mail address breaks the rule `^[\w.+&-]+@[\w.&-]+\.[\w.&-]{2,}$` (ASCII `\w`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
pub enum EmailError {
    /// The address is empty.
    #[error("the e-mail address is empty")]
    Empty,
    /// The address does not match the rule.
    #[error("the e-mail address is not valid")]
    Invalid,
}

/// Why a password breaks the rule of [`PASSWORD_MIN_UNITS`] to [`PASSWORD_MAX_UNITS`]
/// UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
pub enum PasswordError {
    /// Fewer than [`PASSWORD_MIN_UNITS`] code units.
    #[error("the password is shorter than 8 characters")]
    TooShort,
    /// More than [`PASSWORD_MAX_UNITS`] code units.
    #[error("the password is longer than 1024 characters")]
    TooLong,
}

/// Checks `username` against `^[a-zA-Z0-9_-]{3,}$`.
///
/// # Errors
///
/// Returns the first rule that `username` breaks: empty, then a wrong character, then too
/// short.
pub fn check_username(username: &str) -> Result<(), UsernameError> {
    if username.is_empty() {
        Err(UsernameError::Empty)
    } else if !username
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Err(UsernameError::InvalidCharacter)
    } else if username.len() < USERNAME_MIN {
        Err(UsernameError::TooShort)
    } else {
        Ok(())
    }
}

/// Checks `email` against `^[\w.+&-]+@[\w.&-]+\.[\w.&-]{2,}$` (ASCII `\w`).
///
/// # Errors
///
/// Returns [`EmailError::Empty`] or [`EmailError::Invalid`].
pub fn check_email(email: &str) -> Result<(), EmailError> {
    let word = |byte: u8, extra: &[u8]| {
        byte.is_ascii_alphanumeric() || byte == b'_' || extra.contains(&byte)
    };
    if email.is_empty() {
        return Err(EmailError::Empty);
    }
    let valid = email.split_once('@').is_some_and(|(local, domain)| {
        let domain_ok = domain.bytes().all(|byte| word(byte, b".&-"));
        // A dot with at least one character before it and two after it.
        let dot = domain
            .char_indices()
            .any(|(index, c)| c == '.' && index > 0 && domain.len() - index > 2);
        !local.is_empty() && local.bytes().all(|byte| word(byte, b".+&-")) && domain_ok && dot
    });
    if valid {
        Ok(())
    } else {
        Err(EmailError::Invalid)
    }
}

/// Checks that `password` has [`PASSWORD_MIN_UNITS`] to [`PASSWORD_MAX_UNITS`] UTF-16 code
/// units (JavaScript `length`: a character outside the Basic Multilingual Plane counts 2).
///
/// # Errors
///
/// Returns [`PasswordError::TooShort`] or [`PasswordError::TooLong`].
pub fn check_password(password: &str) -> Result<(), PasswordError> {
    let length = password.encode_utf16().count();
    if length < PASSWORD_MIN_UNITS {
        Err(PasswordError::TooShort)
    } else if length > PASSWORD_MAX_UNITS {
        Err(PasswordError::TooLong)
    } else {
        Ok(())
    }
}

/// The rule that each field of a [`Registration`] breaks ([`Registration::errors`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RegistrationErrors {
    /// The username's error.
    pub username: Option<UsernameError>,
    /// The e-mail address's error.
    pub email: Option<EmailError>,
    /// The password's error.
    pub password: Option<PasswordError>,
}

impl RegistrationErrors {
    /// Whether no field breaks a rule.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.username.is_none() && self.email.is_none() && self.password.is_none()
    }
}

impl Registration {
    /// The rule that each field breaks ([`check_username`], [`check_email`],
    /// [`check_password`]).
    #[must_use]
    pub fn errors(&self) -> RegistrationErrors {
        RegistrationErrors {
            username: check_username(&self.username).err(),
            email: check_email(&self.email).err(),
            password: check_password(&self.password).err(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EmailError, PasswordError, UsernameError, check_email, check_password, check_username,
    };
    use crate::model::account::Registration;

    #[test]
    fn usernames_follow_the_server_rule() {
        for good in ["abc", "A_b-9", "___", "player-one"] {
            assert_eq!(check_username(good), Ok(()), "{good}");
        }
        assert_eq!(check_username(""), Err(UsernameError::Empty));
        assert_eq!(check_username("ab"), Err(UsernameError::TooShort));
        for bad in ["a b", "Ünïcode", "name!", "x.y", "ü"] {
            assert_eq!(
                check_username(bad),
                Err(UsernameError::InvalidCharacter),
                "{bad}"
            );
        }
    }

    #[test]
    fn emails_follow_the_server_rule() {
        for good in ["a@b.cd", "first.last+tag@mail.example.org", "a&b@x-y.co"] {
            assert_eq!(check_email(good), Ok(()), "{good}");
        }
        assert_eq!(check_email(""), Err(EmailError::Empty));
        for bad in [
            "a", "a@", "@b.cd", "a@b", "a@b.c", "a@.cd", "a b@c.de", "a@b c.de", "ä@b.cd",
        ] {
            assert_eq!(check_email(bad), Err(EmailError::Invalid), "{bad}");
        }
    }

    #[test]
    fn passwords_need_8_to_1024_characters() {
        assert_eq!(check_password("1234567"), Err(PasswordError::TooShort));
        assert_eq!(check_password("12345678"), Ok(()));
        assert_eq!(check_password(&"x".repeat(1024)), Ok(()));
        assert_eq!(
            check_password(&"x".repeat(1025)),
            Err(PasswordError::TooLong)
        );
        // Counted like JavaScript's `length`: an astral character is two units.
        assert_eq!(check_password("😀😀😀😀"), Ok(()));
        assert_eq!(
            check_password(&"😀".repeat(513)),
            Err(PasswordError::TooLong)
        );
    }

    #[test]
    fn a_registration_reports_each_field() {
        let mut registration = Registration {
            username: "player".to_owned(),
            email: "player@example.com".to_owned(),
            password: "long enough".to_owned(),
            modules: None,
        };
        assert!(registration.errors().is_empty());
        registration.password = "short".to_owned();
        let errors = registration.errors();
        assert!(!errors.is_empty());
        assert_eq!(errors.password, Some(PasswordError::TooShort));
        assert_eq!(errors.username, None);
        assert_eq!(
            Registration::default().errors().username,
            Some(UsernameError::Empty)
        );
    }
}
