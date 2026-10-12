//! Transitional HTTP compatibility path for adapters not extracted yet.
pub use praxis_provider_http::{checked, json, metadata, payload_error, retry_after};

pub fn client() -> reqwest::Client {
    let builder = crate::branding::client_builder();
    #[cfg(test)]
    let builder = builder.no_proxy();
    praxis_provider_http::client_with_builder(builder)
}
