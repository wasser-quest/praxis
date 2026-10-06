use serde::{Deserialize, Serialize};
use praxis_provider_api::EmbeddingProvider as _;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorChunk {
    pub id: i64,
    pub document_id: String,
    pub chunk_index: usize,
    pub content: String,
    pub embedding: Vec<f32>,
    pub metadata: Option<serde_json::Value>,
    pub token_count: Option<usize>,
    pub similarity: f32,
}

/// Compute embedding using neural model (OpenAI/Ollama)
/// Falls back to hash-based embedding if provider is not configured
pub async fn compute_embedding(text: &str) -> Vec<f32> {
    // Try neural embedding first
    match crate::gateway::llm::embeddings::create_embedding_provider() {
        Ok(provider) => match provider.embed(text).await {
            Ok(embedding) => return embedding,
            Err(e) => {
                tracing::warn!("Neural embedding failed, falling back to hash: {}", e);
            }
        },
        Err(e) => {
            tracing::debug!("No embedding provider configured, using hash: {}", e);
        }
    }

    // Fallback: Simple TF-IDF-like embedding using word hashing
    compute_embedding_hash(text)
}

/// Hash-based embedding fallback (256 dimensions)
fn compute_embedding_hash(text: &str) -> Vec<f32> {
    let mut embedding = vec![0.0f32; 256];

    let words: Vec<&str> = text.split_whitespace().collect();
    for word in &words {
        let hash = hash_word(word);
        let idx = (hash % 256) as usize;
        embedding[idx] += 1.0;
    }

    // Normalize
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for val in &mut embedding {
            *val /= norm;
        }
    }

    embedding
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

fn hash_word(word: &str) -> u32 {
    let mut hash: u32 = 5381;
    for byte in word.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u32);
    }
    hash
}

pub async fn store_embedding(
    db: &crate::db::Database,
    chunk_id: i64,
    embedding: &[f32],
) -> anyhow::Result<()> {
    let embedding_bytes = embedding_to_bytes(embedding);
    let conn = db.conn();
    conn.execute(
        "UPDATE document_chunks SET embedding = ?1 WHERE id = ?2",
        rusqlite::params![embedding_bytes, chunk_id],
    )?;
    Ok(())
}

pub async fn vector_search(
    db: &crate::db::Database,
    user_id: &str,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<VectorChunk>> {
    let query_embedding = compute_embedding(query).await;
    let conn = db.conn();

    let mut stmt = conn.prepare(
        "SELECT id, document_id, chunk_index, content, embedding, metadata, token_count 
         FROM document_chunks 
         WHERE user_id = ?1 AND embedding IS NOT NULL",
    )?;

    let mut results: Vec<VectorChunk> = stmt
        .query_map(rusqlite::params![user_id], |row| {
            let embedding_blob: Option<Vec<u8>> = row.get(4)?;
            let content: String = row.get(3)?;

            let embedding = embedding_blob
                .and_then(|blob| bytes_to_embedding(&blob))
                .unwrap_or_default();

            Ok(VectorChunk {
                id: row.get(0)?,
                document_id: row.get(1)?,
                chunk_index: row.get::<_, i64>(2)? as usize,
                content,
                embedding,
                metadata: row
                    .get::<_, Option<String>>(5)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                token_count: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                similarity: 0.0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Calculate similarities
    for chunk in &mut results {
        chunk.similarity = cosine_similarity(&query_embedding, &chunk.embedding);
    }

    // Sort by similarity (highest first)
    results.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Return top N
    results.truncate(limit);

    Ok(results)
}

pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for val in embedding {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

pub fn bytes_to_embedding(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() % 4 != 0 || bytes.is_empty() {
        return None;
    }

    let mut embedding = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let val = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        embedding.push(val);
    }

    Some(embedding)
}

pub async fn ingest_with_embeddings(
    db: &crate::db::Database,
    user_id: &str,
    filename: &str,
    content: &str,
    file_type: &str,
) -> anyhow::Result<String> {
    let document_id = uuid::Uuid::new_v4().to_string();
    let chunks = crate::tools::rag_query::chunk_text(content, 500, 50);
    let chunk_count = chunks.len();

    // Compute all embeddings first (outside of DB lock)
    let mut embeddings: Vec<Vec<f32>> = Vec::new();
    for chunk in &chunks {
        embeddings.push(compute_embedding(chunk).await);
    }

    // Now insert into database
    let conn = db.conn();
    conn.execute(
        "INSERT INTO documents (id, user_id, filename, file_type, file_size, chunk_count, status) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'completed')",
        rusqlite::params![
            document_id,
            user_id,
            filename,
            file_type,
            content.len() as i64,
            chunk_count as i64
        ],
    )?;

    for (i, chunk) in chunks.iter().enumerate() {
        let embedding_bytes = embedding_to_bytes(&embeddings[i]);

        conn.execute(
            "INSERT INTO document_chunks (document_id, user_id, chunk_index, content, embedding, token_count) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                document_id,
                user_id,
                i as i64,
                chunk,
                embedding_bytes,
                chunk.split_whitespace().count() as i64
            ],
        )?;
    }

    tracing::info!(
        "Ingested document {} with {} chunks and embeddings for user {}",
        filename,
        chunk_count,
        user_id
    );

    Ok(document_id)
}

#[cfg(test)]
mod vector_tests {
    use super::*;

    #[tokio::test]
    async fn test_compute_embedding() {
        // Test with hash-based fallback (no neural provider configured)
        let embedding = compute_embedding_hash("hello world");
        assert_eq!(embedding.len(), 256);
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_same() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b)).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_embedding_roundtrip() {
        let embedding = vec![1.0, 2.5, -3.7, 0.0];
        let bytes = embedding_to_bytes(&embedding);
        let decoded = bytes_to_embedding(&bytes).unwrap();
        assert_eq!(embedding.len(), decoded.len());
        for (a, b) in embedding.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 0.001);
        }
    }

    #[test]
    fn test_hash_word_consistent() {
        assert_eq!(hash_word("hello"), hash_word("hello"));
        assert_ne!(hash_word("hello"), hash_word("world"));
    }
}
