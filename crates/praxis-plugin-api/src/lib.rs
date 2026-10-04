//! Local feature-service transport. Installed processes remain operator-trusted.
mod client;
mod server;
mod wire;
pub use client::{Client, LaunchSpec};
pub use server::{serve, Service, ServiceInfo};
pub use wire::*;
