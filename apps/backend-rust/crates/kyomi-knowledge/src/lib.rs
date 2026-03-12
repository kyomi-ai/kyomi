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
//! - [`references`] -- Learning-to-entity reference materialization
//! - [`sql_references`] -- SQL parsing to extract table names
//! - [`retrieval`] -- Vector search pipeline
//! - [`expansion`] -- Graph-style expansion via SQL JOINs
//! - [`context`] -- Per-session conversation context (Redis-backed)
//! - [`episodic`] -- Post-conversation recording and contradiction detection

pub mod context;
pub mod episodic;
pub mod expansion;
pub mod models;
pub mod populate;
pub mod references;
pub mod retrieval;
pub mod sql_references;
pub mod vector_search;

pub use context::ConversationContext;
pub use models::{ContextEntry, ContextEntryKind, MatchedColumn, RetrievalResult, RetrievalSource};
pub use vector_search::{VectorSearch, create_vector_search};
