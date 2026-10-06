use uuid::Uuid;

pub async fn rag_ingest(
    db: &crate::db::Database,
    user_id: &str,
    filename: &str,
    content: &str,
    file_type: &str,
) -> anyhow::Result<String> {
    let document_id = Uuid::new_v4().to_string();
    let chunks = crate::tools::rag_query::chunk_text(content, 500, 50);
    let chunk_count = chunks.len();

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
        conn.execute(
            "INSERT INTO document_chunks (document_id, user_id, chunk_index, content, token_count) 
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                document_id,
                user_id,
                i as i64,
                chunk,
                chunk.split_whitespace().count() as i64
            ],
        )?;
    }

    tracing::info!(
        "Ingested document {} with {} chunks for user {}",
        filename,
        chunk_count,
        user_id
    );

    Ok(document_id)
}

pub async fn list_documents(
    db: &crate::db::Database,
    user_id: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT id, filename, file_type, file_size, chunk_count, status, created_at 
         FROM documents WHERE user_id = ?1 ORDER BY created_at DESC",
    )?;

    let docs = stmt
        .query_map(rusqlite::params![user_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "filename": row.get::<_, String>(1)?,
                "file_type": row.get::<_, String>(2)?,
                "file_size": row.get::<_, i64>(3)?,
                "chunk_count": row.get::<_, i64>(4)?,
                "status": row.get::<_, String>(5)?,
                "created_at": row.get::<_, String>(6)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(docs)
}

pub async fn delete_document(
    db: &crate::db::Database,
    user_id: &str,
    document_id: &str,
) -> anyhow::Result<()> {
    let conn = db.conn();
    conn.execute(
        "DELETE FROM document_chunks WHERE document_id = ?1 AND user_id = ?2",
        rusqlite::params![document_id, user_id],
    )?;
    conn.execute(
        "DELETE FROM documents WHERE id = ?1 AND user_id = ?2",
        rusqlite::params![document_id, user_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod rag_tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (crate::db::Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let ctx = crate::db::contexts::Context {
            user_id: "user1".to_string(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();
        (db, dir)
    }

    #[tokio::test]
    async fn test_rag_ingest() {
        let (db, _dir) = test_db();
        let doc_id = rag_ingest(
            &db,
            "user1",
            "test.txt",
            "Hello world this is a test",
            "txt",
        )
        .await
        .unwrap();
        assert!(!doc_id.is_empty());

        let docs = list_documents(&db, "user1").await.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["filename"], "test.txt");
    }

    #[tokio::test]
    async fn test_rag_query() {
        let (db, _dir) = test_db();
        rag_ingest(
            &db,
            "user1",
            "test.txt",
            "Rust is a systems programming language",
            "txt",
        )
        .await
        .unwrap();

        let results = crate::tools::rag_query::rag_query(&db, "user1", "Rust", 10)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(results[0].content.contains("Rust"));
    }

    #[tokio::test]
    async fn test_delete_document() {
        let (db, _dir) = test_db();
        let doc_id = rag_ingest(&db, "user1", "test.txt", "content", "txt")
            .await
            .unwrap();
        delete_document(&db, "user1", &doc_id).await.unwrap();
        let docs = list_documents(&db, "user1").await.unwrap();
        assert!(docs.is_empty());
    }
}

/// The whole knowledge-base namespace: one implementation shared by the
/// package's `builtin` handlers and any internal caller. Result formats are
/// the historical ones.
pub async fn run(
    db: &crate::db::Database,
    user: &str,
    name: &str,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    Ok(match name {
        "rag_search" => {
            let query = args["query"].as_str().unwrap_or("");
            let limit = args["limit"].as_u64().unwrap_or(5) as usize;
            match crate::tools::vector::vector_search(db, user, query, limit).await {
                Ok(results) => {
                    if results.is_empty() {
                        "No relevant documents found.".to_string()
                    } else {
                        let mut output = String::new();
                        for (i, chunk) in results.iter().enumerate() {
                            output.push_str(&format!(
                                "--- Result {} (similarity: {:.3}) ---\n{}\n\n",
                                i + 1,
                                chunk.similarity,
                                chunk.content
                            ));
                        }
                        output
                    }
                }
                Err(error) => format!("Error: {}", error),
            }
        }
        "rag_ingest" => {
            let filename = args["filename"].as_str().unwrap_or("untitled");
            let content = args["content"].as_str().unwrap_or("");
            let file_type = args["file_type"].as_str().unwrap_or("txt");
            match crate::tools::vector::ingest_with_embeddings(db, user, filename, content, file_type).await {
                Ok(doc_id) => format!("Document ingested successfully. ID: {}", doc_id),
                Err(error) => format!("Error: {}", error),
            }
        }
        "rag_list" => match list_documents(db, user).await {
            Ok(docs) => {
                if docs.is_empty() {
                    "No documents in knowledge base.".to_string()
                } else {
                    let mut output = String::from("Documents in knowledge base:\n");
                    for doc in &docs {
                        output.push_str(&format!(
                            "- {} ({}) - {} chunks - ID: {}\n",
                            doc["filename"], doc["file_type"], doc["chunk_count"], doc["id"]
                        ));
                    }
                    output
                }
            }
            Err(error) => format!("Error: {}", error),
        },
        "rag_delete" => {
            let document_id = args["document_id"].as_str().unwrap_or("");
            match delete_document(db, user, document_id).await {
                Ok(_) => format!("Document {} deleted.", document_id),
                Err(error) => format!("Error: {}", error),
            }
        }
        other => anyhow::bail!("Unknown knowledge-base operation: {other}"),
    })
}
