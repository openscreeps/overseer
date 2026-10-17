//! HTTP APIs of community server plugins: one module per plugin, generated from
//! `reference/plugins/<plugin>.openapi.json` (the plugin's contract composed on the community
//! contract) with only the operations the plugin provides, adapts or adds.

pub mod screepsmod_admin_utils;
pub mod screepsmod_auth;
