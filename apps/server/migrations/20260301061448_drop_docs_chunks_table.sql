-- Drop the docs_chunks table and its index.
-- Documentation is now served via MCP resources from disk,
-- replacing the vector similarity search approach.
DROP TABLE IF EXISTS docs_chunks;
