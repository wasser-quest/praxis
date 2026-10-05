//! Host services reusable by any frontend. The dashboard is an optional
//! adapter over these; headless runtimes use them directly.
pub mod agent;
pub mod admin;
pub mod auth;
pub mod graphs;
pub mod media;
pub mod profiles;
pub mod secrets;
pub mod sessions;
