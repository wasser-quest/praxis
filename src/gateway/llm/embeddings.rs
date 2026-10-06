use anyhow::Result;
use praxis_provider_api::{ErrorKind, ProviderError};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
}

pub struct EmbeddingProvider {
    config: EmbeddingConfig,
    client: Client,
}

impl EmbeddingProvider {
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            client: crate::branding::client(),
        }
    }


    async fn embed_openai(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/embeddings", self.config.base_url);

        let body = serde_json::json!({
            "model": self.config.model,
            "input": text,
        });

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI embedding API error {}: {}", status, text);
        }

        let data: serde_json::Value = resp.json().await?;
        let embedding = data["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No embedding in response"))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(embedding)
    }

    async fn embed_batch_openai(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.config.base_url);

        let body = serde_json::json!({
            "model": self.config.model,
            "input": texts,
        });

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI embedding API error {}: {}", status, text);
        }

        let data: serde_json::Value = resp.json().await?;
        let embeddings = data["data"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No data in response"))?
            .iter()
            .map(|item| {
                item["embedding"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                    .collect()
            })
            .collect();

        Ok(embeddings)
    }

    async fn embed_ollama(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embeddings", self.config.base_url);

        let body = serde_json::json!({
            "model": self.config.model,
            "prompt": text,
        });

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.config.api_key {
            if !key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", key));
            }
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama embedding API error {}: {}", status, text);
        }

        let data: serde_json::Value = resp.json().await?;
        let embedding = data["embedding"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No embedding in response"))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(embedding)
    }
}

/// Create embedding provider from environment variables
pub fn create_embedding_provider() -> Result<EmbeddingProvider> {
    let provider = std::env::var("EMBEDDING_PROVIDER")
        .unwrap_or_else(|_| "ollama".to_string());

    let (model, base_url, api_key) = match provider.as_str() {
        "openai" => {
            let model = std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".to_string());
            let base_url = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let api_key = std::env::var("OPENAI_API_KEY").ok();
            (model, base_url, api_key)
        }
        "ollama" => {
            let model = std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".to_string());
            let base_url = std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let api_key = std::env::var("OLLAMA_API_KEY").ok();
            (model, base_url, api_key)
        }
        _ => anyhow::bail!("Unsupported embedding provider: {}", provider),
    };

    Ok(EmbeddingProvider::new(EmbeddingConfig {
        provider,
        model,
        base_url,
        api_key,
    }))
}

/// The common embedding interface (`praxis-provider-api`). Batched calls fall
/// back to one-by-one inside the adapter where the wire API has no batch.
#[async_trait::async_trait]
impl praxis_provider_api::EmbeddingProvider for EmbeddingProvider {
    fn name(&self) -> &str {
        &self.config.provider
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        Ok(match self.config.provider.as_str() {
            "openai" => self.embed_openai(text).await?,
            "ollama" => self.embed_ollama(text).await?,
            _ => {
                return Err(ProviderError::with_cause(
                    ErrorKind::Unsupported,
                    "unsupported embedding provider",
                ))
            }
        })
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        Ok(match self.config.provider.as_str() {
            "openai" => self.embed_batch_openai(texts).await?,
            "ollama" => {
                // Ollama has no batch embedding endpoint: one by one.
                let mut results = Vec::new();
                for text in texts {
                    results.push(self.embed_ollama(text).await?);
                }
                results
            }
            _ => {
                return Err(ProviderError::with_cause(
                    ErrorKind::Unsupported,
                    "unsupported embedding provider",
                ))
            }
        })
    }
}
