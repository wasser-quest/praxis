//! Shared runtime services available without feature plugins or frontends.
pub mod events;
pub mod retention;
pub mod services;
pub mod templates;

#[cfg(test)]
mod tests;
