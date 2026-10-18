//! The signed-in user's private messages.

use overseer_net::api::MessagesApi as _;
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::model::messages::{MessageUpdate, UserMessage, UserMessagesIndexResponse};

use crate::{Error, Feed, Overseer};

/// The signed-in user's private messages.
///
/// Created by [`Overseer::messages`]; cheap to clone. Every operation needs a signed-in user
/// and fails with [`Error::Anonymous`] otherwise, and with [`Error::Closed`] after
/// [`Overseer::shutdown`]. Users are named by id (`_id`), not username; find ids with
/// [`Users::find`](crate::Users::find).
#[derive(Clone, Debug)]
pub struct Messages {
    overseer: Overseer,
}

impl Messages {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// The latest message of each conversation, newest first, and the public profiles
    /// (username, badge) of the other participants keyed by id
    /// (`GET /api/user/messages/index`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn conversations(&self) -> Result<UserMessagesIndexResponse, Error> {
        Ok(self.overseer.user_api()?.conversations().await?)
    }

    /// The latest 100 messages exchanged with user `respondent`, oldest first
    /// (`GET /api/user/messages/list`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn conversation(&self, respondent: &str) -> Result<Vec<UserMessage>, Error> {
        let api = self.overseer.user_api()?;
        Ok(api.conversation(respondent.to_owned()).await?)
    }

    /// Sends `text` to user `respondent` (`POST /api/user/messages/send`). Both participants'
    /// [feeds](Self::feed) and [conversation feeds](Self::conversation_feed) receive the new
    /// message.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails or the server refuses (`invalid respondent`, `text too long` over 100 KiB).
    pub async fn send(&self, respondent: &str, text: &str) -> Result<(), Error> {
        let api = self.overseer.user_api()?;
        api.send_message(respondent, text).await?;
        Ok(())
    }

    /// Marks the incoming message `id` read (`POST /api/user/messages/mark-read`); the
    /// conversation feeds of both participants receive `{_id, unread: false}`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn mark_read(&self, id: &str) -> Result<(), Error> {
        self.overseer.user_api()?.mark_read(id).await?;
        Ok(())
    }

    /// The number of unread incoming messages (`GET /api/user/messages/unread-count`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn unread_count(&self) -> Result<u64, Error> {
        Ok(self.overseer.user_api()?.unread_count().await?)
    }

    /// Every message the user receives from now on, from any sender (`user:<user>/newMessage`):
    /// complete incoming (`in`) copies.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] and [`Error::Closed`].
    pub fn feed(&self) -> Result<Feed<MessageUpdate>, Error> {
        let user = self.user()?;
        Ok(self.overseer.feed(Channel::NewMessage { user }, message))
    }

    /// Every change of the conversation with user `respondent` from now on
    /// (`user:<user>/message:<respondent>`): complete copies of the messages sent either
    /// way, and `{_id, unread: false}` when one is marked read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] and [`Error::Closed`].
    pub fn conversation_feed(&self, respondent: &str) -> Result<Feed<MessageUpdate>, Error> {
        let user = self.user()?;
        let channel = Channel::Conversation {
            user,
            respondent: respondent.to_owned(),
        };
        Ok(self.overseer.feed(channel, message))
    }

    /// The signed-in user's id for a channel.
    fn user(&self) -> Result<String, Error> {
        self.overseer.ensure_open()?;
        Ok(self.overseer.require_user()?.to_owned())
    }
}

/// The message of a message channel's payload.
fn message(payload: Payload) -> Option<MessageUpdate> {
    if let Payload::Message(message) = payload {
        Some(message)
    } else {
        None
    }
}
