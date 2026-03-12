// SPDX-License-Identifier: AGPL-3.0-or-later

//! Documentation resource discovery and reading.
//!
//! Walks the docs directory (set via `DOCS_DIR` env var, default `/data/docs`)
//! to provide a catalog of `docs://kyomi/*` resources. Used by both the MCP
//! server and the internal agent's `browse_resources` / `read_resource` tools.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Default path for documentation files (overridable via `DOCS_DIR` env var).
const DEFAULT_DOCS_DIR: &str = "/data/docs";

/// URI prefix for documentation resources.
pub const DOCS_URI_PREFIX: &str = "docs://kyomi/";

/// A documentation resource entry (URI + metadata).
#[derive(Debug, Clone, Serialize)]
pub struct DocResource {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

/// Get the docs directory path from env or default.
pub fn docs_dir() -> PathBuf {
    std::env::var("DOCS_DIR")
        .unwrap_or_else(|_| DEFAULT_DOCS_DIR.to_string())
        .into()
}

/// Strip YAML frontmatter (between first pair of `---` lines) from markdown content.
/// Also converts VitePress-specific syntax to standard markdown.
pub fn strip_frontmatter(content: &str) -> String {
    let mut result = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_ended = false;

    for line in content.lines() {
        if !frontmatter_ended {
            if line.trim() == "---" {
                if !in_frontmatter {
                    in_frontmatter = true;
                    continue;
                } else {
                    frontmatter_ended = true;
                    continue;
                }
            }
            if in_frontmatter {
                continue;
            }
        }
        // Convert VitePress ::: blocks to blockquotes
        if line.starts_with("::: info") {
            let title = line.strip_prefix("::: info").unwrap_or("").trim();
            if title.is_empty() {
                result.push("> **Info:**".to_string());
            } else {
                result.push(format!("> **{title}**"));
            }
        } else if line.starts_with("::: warning") {
            let title = line.strip_prefix("::: warning").unwrap_or("").trim();
            if title.is_empty() {
                result.push("> **Warning:**".to_string());
            } else {
                result.push(format!("> **{title}**"));
            }
        } else if line.starts_with("::: tip") {
            let title = line.strip_prefix("::: tip").unwrap_or("").trim();
            if title.is_empty() {
                result.push("> **Tip:**".to_string());
            } else {
                result.push(format!("> **{title}**"));
            }
        } else if line.trim() == ":::" {
            // End of VitePress block — skip the closing marker
        } else {
            result.push(line.to_string());
        }
    }

    result.join("\n").trim().to_string()
}

/// Extract a field value from YAML frontmatter.
pub fn extract_frontmatter_field(content: &str, field: &str) -> Option<String> {
    let mut in_frontmatter = false;
    for line in content.lines() {
        if line.trim() == "---" {
            if !in_frontmatter {
                in_frontmatter = true;
                continue;
            } else {
                break;
            }
        }
        if in_frontmatter {
            let prefix = format!("{}:", field);
            if let Some(rest) = line.strip_prefix(&prefix) {
                return Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    None
}

/// List all available documentation resources by walking the docs directory.
pub fn list_doc_resources() -> Vec<DocResource> {
    let dir = docs_dir();
    if !dir.exists() {
        return vec![];
    }
    let mut resources = vec![];
    walk_docs_dir(&dir, &dir, &mut resources);
    resources
}

/// Read a documentation resource by its `docs://kyomi/*` URI.
///
/// Returns the stripped markdown content, or `None` if the URI doesn't resolve.
pub fn read_doc_resource(uri: &str) -> Option<String> {
    let doc_path = uri.strip_prefix(DOCS_URI_PREFIX)?;
    let dir = docs_dir();

    // Try exact path first, then index.md for directory paths
    let candidates = [
        dir.join(format!("{doc_path}.md")),
        dir.join(format!("{doc_path}/index.md")),
    ];

    for candidate in &candidates {
        if let Ok(content) = std::fs::read_to_string(candidate) {
            return Some(strip_frontmatter(&content));
        }
    }

    None
}

fn walk_docs_dir(base: &Path, current: &Path, resources: &mut Vec<DocResource>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };

    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk_docs_dir(base, &path, resources);
        } else if path.extension().is_some_and(|e| e == "md") {
            if let Some(resource) = doc_file_to_resource(base, &path) {
                resources.push(resource);
            }
        }
    }
}

fn doc_file_to_resource(base: &Path, file: &Path) -> Option<DocResource> {
    let relative = file.strip_prefix(base).ok()?;
    let mut uri_path = relative.with_extension("").to_string_lossy().to_string();

    // index.md maps to parent path (connect/index -> connect)
    if uri_path.ends_with("/index") {
        uri_path = uri_path.trim_end_matches("/index").to_string();
    }
    if uri_path == "index" {
        uri_path = "index".to_string();
    }

    let uri = format!("{DOCS_URI_PREFIX}{uri_path}");

    // Read file for title and description
    let content = std::fs::read_to_string(file).ok()?;
    let stripped = strip_frontmatter(&content);
    let title = stripped
        .lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l.trim_start_matches("# ").to_string())
        .unwrap_or_else(|| uri_path.replace('/', " - "));

    let description = extract_frontmatter_field(&content, "description").unwrap_or_default();

    Some(DocResource {
        uri,
        name: title,
        description,
        mime_type: "text/markdown".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_yaml() {
        let input = "---\ntitle: Test\n---\n# Hello\nContent here";
        let result = strip_frontmatter(input);
        assert_eq!(result, "# Hello\nContent here");
    }

    #[test]
    fn strip_frontmatter_no_frontmatter() {
        let input = "# Hello\nContent here";
        let result = strip_frontmatter(input);
        assert_eq!(result, "# Hello\nContent here");
    }

    #[test]
    fn strip_frontmatter_converts_vitepress_blocks() {
        let input = "---\ntitle: T\n---\n::: info\nSome info\n:::\n::: warning Custom Title\nWarn\n:::";
        let result = strip_frontmatter(input);
        assert!(result.contains("> **Info:**"));
        assert!(result.contains("> **Custom Title**"));
    }

    #[test]
    fn extract_frontmatter_field_found() {
        let input = "---\ntitle: \"My Title\"\ndescription: \"Desc here\"\n---\n# H";
        assert_eq!(
            extract_frontmatter_field(input, "description"),
            Some("Desc here".to_string())
        );
    }

    #[test]
    fn extract_frontmatter_field_missing() {
        let input = "---\ntitle: T\n---\n# H";
        assert_eq!(extract_frontmatter_field(input, "description"), None);
    }

    #[test]
    fn read_doc_resource_bad_prefix() {
        // URI without the correct prefix returns None
        assert!(read_doc_resource("invalid://something").is_none());
    }
}
