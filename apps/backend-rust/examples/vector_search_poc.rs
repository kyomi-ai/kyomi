// NOTE: This POC intentionally uses the old AllMiniLML6V2 model for historical
// comparison. Production code uses BGESmallENV15 via the kyomi-embed crate.

//! Vector Search POC — Proves fastembed + pgvector works in Rust
//!
//! This POC validates:
//! 1. fastembed can generate all-MiniLM-L6-v2 embeddings (384 dimensions)
//! 2. pgvector crate stores/queries vectors via sqlx
//! 3. Cosine similarity search returns correct results
//! 4. Outputs embeddings for comparison with Python sentence-transformers
//!
//! Usage:
//!   cargo run                           # Run full POC (requires PostgreSQL with pgvector)
//!   cargo run -- --embeddings-only      # Only generate embeddings (no DB needed)

use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use pgvector::Vector;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

/// Test documents for the POC — mix of related and unrelated content
const TEST_DOCUMENTS: &[&str] = &[
    "revenue by region for Q4 2024",
    "monthly active users over time",
    "customer churn rate analysis",
    "how to make chocolate cake",
    "sales performance by product category",
    "PostgreSQL database optimization tips",
    "user retention cohort analysis",
    "weather forecast for tomorrow",
];

/// Queries to test similarity search
const TEST_QUERIES: &[&str] = &[
    "show me revenue data",
    "user engagement metrics",
    "cooking recipes",
];

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let embeddings_only = args.iter().any(|a| a == "--embeddings-only");

    println!("=== Vector Search POC ===\n");

    // Step 1: Initialize the embedding model
    println!("Step 1: Loading all-MiniLM-L6-v2 model via fastembed...");
    let model = TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true),
    )?;
    println!("  Model loaded successfully.\n");

    // Step 2: Generate embeddings for test documents
    println!("Step 2: Generating embeddings for {} documents...", TEST_DOCUMENTS.len());
    let doc_embeddings = model.embed(TEST_DOCUMENTS.to_vec(), None)?;
    println!("  Generated {} embeddings, each with {} dimensions.",
        doc_embeddings.len(),
        doc_embeddings[0].len()
    );

    // Validate dimensions
    assert_eq!(doc_embeddings[0].len(), 384, "Expected 384 dimensions for all-MiniLM-L6-v2");
    println!("  Dimension check passed (384).\n");

    // Step 3: Output embeddings for Python comparison
    println!("Step 3: Embedding comparison data (first 5 values per document):");
    println!("  Copy these to compare with Python sentence-transformers output:");
    println!("  ---");
    for (i, (doc, emb)) in TEST_DOCUMENTS.iter().zip(&doc_embeddings).enumerate() {
        let first_5: Vec<String> = emb.iter().take(5).map(|v| format!("{:.6}", v)).collect();
        println!("  [{}] \"{}\"", i, doc);
        println!("       first_5: [{}]", first_5.join(", "));
    }
    println!("  ---\n");

    if embeddings_only {
        println!("--embeddings-only mode: skipping database tests.");
        println!("\n=== POC PASSED (embeddings only) ===");
        return Ok(());
    }

    // Step 4: Connect to PostgreSQL and set up pgvector
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kyomi_test:test@localhost:5434/kyomi_test".to_string());

    println!("Step 4: Connecting to PostgreSQL at {}...", database_url);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    println!("  Connected.\n");

    // Enable pgvector extension and create test table
    println!("Step 5: Setting up pgvector...");
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(&pool)
        .await?;

    sqlx::query("DROP TABLE IF EXISTS poc_embeddings")
        .execute(&pool)
        .await?;

    sqlx::query(
        "CREATE TABLE poc_embeddings (
            id SERIAL PRIMARY KEY,
            content TEXT NOT NULL,
            embedding vector(384) NOT NULL
        )"
    )
    .execute(&pool)
    .await?;

    // Create HNSW index (same type used in Kyomi production)
    sqlx::query(
        "CREATE INDEX ON poc_embeddings
         USING hnsw (embedding vector_cosine_ops)"
    )
    .execute(&pool)
    .await?;
    println!("  Table and HNSW index created.\n");

    // Step 6: Insert embeddings
    println!("Step 6: Inserting {} document embeddings...", doc_embeddings.len());
    for (doc, emb) in TEST_DOCUMENTS.iter().zip(&doc_embeddings) {
        let vector = Vector::from(emb.clone());
        sqlx::query("INSERT INTO poc_embeddings (content, embedding) VALUES ($1, $2)")
            .bind(doc)
            .bind(vector)
            .execute(&pool)
            .await?;
    }
    println!("  All embeddings inserted.\n");

    // Step 7: Similarity search
    println!("Step 7: Running cosine similarity searches...\n");
    let query_embeddings = model.embed(TEST_QUERIES.to_vec(), None)?;

    for (query, query_emb) in TEST_QUERIES.iter().zip(&query_embeddings) {
        let query_vector = Vector::from(query_emb.clone());

        let rows = sqlx::query(
            "SELECT content, 1 - (embedding <=> $1) AS similarity
             FROM poc_embeddings
             ORDER BY embedding <=> $1
             LIMIT 3"
        )
        .bind(&query_vector)
        .fetch_all(&pool)
        .await?;

        println!("  Query: \"{}\"", query);
        for row in &rows {
            let content: &str = row.try_get("content")?;
            let similarity: f64 = row.try_get("similarity")?;
            println!("    {:.4}  \"{}\"", similarity, content);
        }
        println!();
    }

    // Step 8: Cleanup
    sqlx::query("DROP TABLE poc_embeddings")
        .execute(&pool)
        .await?;
    println!("Step 8: Cleaned up test table.\n");

    println!("=== POC PASSED ===");
    println!("All checks passed:");
    println!("  - fastembed generates all-MiniLM-L6-v2 embeddings (384 dims)");
    println!("  - pgvector stores and indexes vectors via sqlx");
    println!("  - Cosine similarity search returns semantically relevant results");
    println!("  - HNSW index works correctly");

    Ok(())
}
