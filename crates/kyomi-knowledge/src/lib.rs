// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-knowledge -- SQL-based knowledge retrieval.
//!
//! Provides context retrieval for the chat agent using vector embeddings
//! stored in PostgreSQL (pgvector) or SQLite (in-memory cosine similarity).
//! All data lives in the same database -- no external graph service needed.
//!
//! # Modules
//!
//! - [`models`] -- Shared types (ContextEntry, RetrievalResult, etc.)
//! - [`vector_search`] -- VectorSearch trait + Postgres/SQLite implementations
//! - [`populate`] -- Embedding generation and storage
//! - [`retrieval`] -- Vector search pipeline

pub mod knowledge_files;
pub mod models;
pub mod populate;
pub mod retrieval;
pub mod unify;
pub mod vector_search;

pub use models::{ContextEntry, ContextEntryKind, MatchedColumn, RetrievalResult, RetrievalSource};
pub use vector_search::{VectorSearch, create_vector_search};
