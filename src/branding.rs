//! Application identity in provider HTTP metadata, independent of bot personas.
pub const APP_NAME: &str = "Praxis";
pub const APP_URL: &str = "https://getpraxis.boo";
pub const USER_AGENT: &str = concat!(
    "Praxis/",
    env!("CARGO_PKG_VERSION"),
    " (+https://getpraxis.boo)"
);

pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().user_agent(USER_AGENT)
}

pub fn client() -> reqwest::Client {
    client_builder()
        .build()
        .expect("static Praxis HTTP client configuration")
}

/// OpenRouter-specific attribution. Other providers receive only User-Agent.
pub fn openrouter_headers() -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert("http-referer", HeaderValue::from_static(APP_URL));
    headers.insert("x-openrouter-title", HeaderValue::from_static(APP_NAME));
    headers
}
