//! Messages: conversations, their messages, the unread count and the socket's message feed.

/// Every contract's private messages (`GET /api/user/messages/list`), their direction and the
/// conversation index (`GET /api/user/messages/index`: the latest message of each conversation
/// and the participants' public profiles) are these types (codegen re-exports the official ones
/// from the others).
pub use crate::api::official::types::{
    UserMessage, UserMessageType, UserMessagesIndexResponse, UserMessagesIndexResponseMessagesItem,
    UserPublicSummary,
};

/// A message of the socket's message channels (`user:<user>/newMessage` and
/// `user:<user>/message:<respondent>`): complete when sent, only `_id` and `unread` when marked
/// read. Every socket contract defines it identically (codegen re-exports the official one); it
/// is not the HTTP [`UserMessage`], whose fields are all present.
pub use crate::socket::official::types::UserMessage as MessageUpdate;

/// The direction of a [`MessageUpdate`]: the socket contracts' own `in`/`out` enum, identical
/// in every one of them.
pub use crate::socket::official::types::UserMessageType as MessageUpdateType;
