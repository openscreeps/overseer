//! The HTTP API of the community contract (the official one composed with the community profile:
//! the stock Screeps server without mods), generated from `reference/community.openapi.json`:
//! [`endpoints`] holds one [`Endpoint`](crate::api::Endpoint) per operation and [`types`] their
//! schemas.

pub mod endpoints;
pub mod types;
