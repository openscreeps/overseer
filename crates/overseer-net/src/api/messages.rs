//! Messages area: the signed-in user's conversations, their messages, sending, read state and
//! the unread count. The live message feeds are socket channels.

use overseer_core::api::{community, official, openscreeps};
use overseer_core::model::messages::{UserMessage, UserMessagesIndexResponse};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

/// The signed-in user's private messages. Users are named by id (`_id`), not username.
pub trait MessagesApi {
    /// The latest message of each conversation, newest first, and the public profiles
    /// (username, badge) of the other participants keyed by id
    /// (`GET /api/user/messages/index`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn conversations(&self) -> impl Future<Output = Result<UserMessagesIndexResponse, Error>>;

    /// The latest 100 messages exchanged with user `respondent`, oldest first
    /// (`GET /api/user/messages/list`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn conversation(
        &self,
        respondent: String,
    ) -> impl Future<Output = Result<Vec<UserMessage>, Error>>;

    /// Sends `text` to user `respondent` (`POST /api/user/messages/send`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send), for example the server's
    /// refusal `invalid respondent`, or `text too long` over 100 KiB.
    fn send_message(&self, respondent: &str, text: &str)
    -> impl Future<Output = Result<(), Error>>;

    /// Marks the incoming message `id` read (`POST /api/user/messages/mark-read`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn mark_read(&self, id: &str) -> impl Future<Output = Result<(), Error>>;

    /// The number of unread incoming messages (`GET /api/user/messages/unread-count`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn unread_count(&self) -> impl Future<Output = Result<u64, Error>>;
}

impl MessagesApi for Api {
    async fn conversations(&self) -> Result<UserMessagesIndexResponse, Error> {
        delegate!(self, api => api.conversations().await)
    }

    async fn conversation(&self, respondent: String) -> Result<Vec<UserMessage>, Error> {
        delegate!(self, api => api.conversation(respondent).await)
    }

    async fn send_message(&self, respondent: &str, text: &str) -> Result<(), Error> {
        delegate!(self, api => api.send_message(respondent, text).await)
    }

    async fn mark_read(&self, id: &str) -> Result<(), Error> {
        delegate!(self, api => api.mark_read(id).await)
    }

    async fn unread_count(&self) -> Result<u64, Error> {
        delegate!(self, api => api.unread_count().await)
    }
}

/// `impl MessagesApi` for the per-contract API `$api` sending the endpoints of `$contract`;
/// every contract has the same message endpoints (only the ignored mark-read acknowledgement
/// differs).
macro_rules! messages_api {
    ($api:ty, $contract:ident) => {
        impl MessagesApi for $api {
            async fn conversations(&self) -> Result<UserMessagesIndexResponse, Error> {
                let index = $contract::endpoints::UserMessagesIndex;
                Ok(self.client.send(&index).await?.body)
            }

            async fn conversation(&self, respondent: String) -> Result<Vec<UserMessage>, Error> {
                let list = $contract::endpoints::UserMessagesList { respondent };
                Ok(self.client.send(&list).await?.body.messages)
            }

            async fn send_message(&self, respondent: &str, text: &str) -> Result<(), Error> {
                let body = $contract::types::UserMessagesSendRequest {
                    respondent: respondent.into(),
                    text: text.into(),
                };
                let send = $contract::endpoints::UserMessagesSend { body };
                self.client.send(&send).await?;
                Ok(())
            }

            async fn mark_read(&self, id: &str) -> Result<(), Error> {
                let body = $contract::types::UserMessagesMarkReadRequest { id: id.into() };
                let mark = $contract::endpoints::UserMessagesMarkRead { body };
                self.client.send(&mark).await?;
                Ok(())
            }

            async fn unread_count(&self) -> Result<u64, Error> {
                let count = $contract::endpoints::UserMessagesUnreadCount;
                Ok(self.client.send(&count).await?.body.count)
            }
        }
    };
}

messages_api!(Official, official);
messages_api!(OpenScreeps, openscreeps);
messages_api!(Community, community);
