//! The messages of the registration, username and password forms. The rules are the library's
//! (`overseer::model::account::check_username` and friends), which mirror the server's so
//! that mistakes show before submitting; the server stays the authority and its error strings
//! are shown as-is.

use overseer::model::account::{
    EmailError, PasswordError, Registration as Account, UsernameError, check_email, check_password,
    check_username,
};

/// Why `username` is rejected.
pub fn username_error(username: &str) -> Option<&'static str> {
    check_username(username).err().map(|error| match error {
        UsernameError::Empty => "Enter a username",
        UsernameError::InvalidCharacter => "Use only letters, digits, _ and -",
        UsernameError::TooShort => "Use at least 3 characters",
    })
}

/// Why `email` is rejected.
pub fn email_error(email: &str) -> Option<&'static str> {
    check_email(email).err().map(|error| match error {
        EmailError::Empty => "Enter an e-mail address",
        EmailError::Invalid => "Enter a valid e-mail address",
    })
}

/// Why `password` is rejected.
pub fn password_error(password: &str) -> Option<&'static str> {
    check_password(password).err().map(|error| match error {
        PasswordError::TooShort => "Use at least 8 characters",
        PasswordError::TooLong => "Use at most 1024 characters",
    })
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

impl Registration {
    /// Whether a field breaks a rule or the confirmation does not match.
    pub fn has_errors(&self) -> bool {
        !self.account().errors().is_empty()
            || confirm_error(&self.password, &self.confirm).is_some()
    }

    /// The account to register.
    pub fn account(&self) -> Account {
        Account {
            username: self.username.clone(),
            email: self.email.clone(),
            password: self.password.clone(),
            modules: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Registration, email_error, password_error, username_error};

    #[test]
    fn each_broken_rule_has_a_message() {
        assert_eq!(username_error(""), Some("Enter a username"));
        assert_eq!(
            username_error("a b"),
            Some("Use only letters, digits, _ and -")
        );
        assert_eq!(username_error("ab"), Some("Use at least 3 characters"));
        assert_eq!(username_error("player"), None);
        assert_eq!(email_error(""), Some("Enter an e-mail address"));
        assert_eq!(email_error("a@b"), Some("Enter a valid e-mail address"));
        assert_eq!(password_error("short"), Some("Use at least 8 characters"));
        assert_eq!(
            password_error(&"x".repeat(1025)),
            Some("Use at most 1024 characters")
        );
    }

    #[test]
    fn the_form_needs_matching_passwords() {
        let mut form = Registration {
            username: "player".to_owned(),
            email: "player@example.com".to_owned(),
            password: "long enough".to_owned(),
            confirm: "long enough".to_owned(),
        };
        assert!(!form.has_errors());
        assert_eq!(form.account().username, "player");
        form.confirm = "different".to_owned();
        assert!(form.has_errors());
        assert!(Registration::default().has_errors());
    }
}
