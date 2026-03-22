// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tree building and traversal logic for the knowledge file tree.
//!
//! Pure-logic module — no `view!` macros. Operates on [`KnowledgeTreeEntry`]
//! flat lists and produces tree structures for rendering.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::types::KnowledgeTreeEntry;

/// A node in the knowledge file tree with depth information.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode {
    pub entry: KnowledgeTreeEntry,
    pub children: Vec<TreeNode>,
    pub depth: usize,
}

/// Build a tree from a flat list of entries.
///
/// Matches the React `buildTree()` in `KnowledgeFileTree.jsx` (lines 36-65):
/// - Entries with a `parent_id` that exists in the list become children of that parent.
/// - Entries with no `parent_id` (or a `parent_id` not in the list) become roots.
/// - Sorting: folders before files, then by `sort_order`, then alphabetically by name.
pub fn build_tree(entries: &[KnowledgeTreeEntry]) -> Vec<TreeNode> {
    // Map each entry id to its index in the slice.
    let id_to_idx: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();

    // Build parent -> children index mapping.
    let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut root_indices: Vec<usize> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        match entry.parent_id.as_deref() {
            Some(pid) if id_to_idx.contains_key(pid) => {
                children_of.entry(id_to_idx[pid]).or_default().push(i);
            }
            _ => {
                root_indices.push(i);
            }
        }
    }

    // Recursively build TreeNode vec from indices.
    fn build_nodes(
        indices: &[usize],
        entries: &[KnowledgeTreeEntry],
        children_of: &HashMap<usize, Vec<usize>>,
        depth: usize,
    ) -> Vec<TreeNode> {
        let mut nodes: Vec<TreeNode> = indices
            .iter()
            .map(|&i| {
                let child_indices = children_of.get(&i).cloned().unwrap_or_default();
                let children = build_nodes(&child_indices, entries, children_of, depth + 1);
                TreeNode {
                    entry: entries[i].clone(),
                    children,
                    depth,
                }
            })
            .collect();

        sort_nodes(&mut nodes);
        nodes
    }

    build_nodes(&root_indices, entries, &children_of, 0)
}

/// Sort nodes: folders before files, then by sort_order, then alphabetically.
///
/// Uses `str::cmp` (byte-order) for name comparison, which matches the default
/// behavior of JavaScript's `localeCompare` for ASCII strings in the React source.
fn sort_nodes(nodes: &mut [TreeNode]) {
    nodes.sort_by(|a, b| {
        // Folders first
        if a.entry.is_folder != b.entry.is_folder {
            return if a.entry.is_folder {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        // Then by sort_order
        let ord = a.entry.sort_order.cmp(&b.entry.sort_order);
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
        // Then alphabetically (byte-order, matching JS localeCompare for ASCII)
        a.entry.name.cmp(&b.entry.name)
    });

    for node in nodes.iter_mut() {
        if !node.children.is_empty() {
            sort_nodes(&mut node.children);
        }
    }
}

/// Flatten a tree for rendering, respecting expanded folder state.
///
/// Returns `(entry, depth, is_last_child)` tuples. Only recurses into a
/// folder's children if its ID is in `expanded`.
///
/// `is_last_child` is `true` for the last sibling at each nesting level
/// (useful for rendering tree connector lines).
pub fn flatten_tree(
    tree: &[TreeNode],
    expanded: &HashSet<String>,
) -> Vec<(KnowledgeTreeEntry, usize, bool)> {
    let mut result = Vec::new();

    fn walk(
        nodes: &[TreeNode],
        expanded: &HashSet<String>,
        result: &mut Vec<(KnowledgeTreeEntry, usize, bool)>,
    ) {
        let len = nodes.len();
        for (i, node) in nodes.iter().enumerate() {
            let is_last = i == len - 1;
            result.push((node.entry.clone(), node.depth, is_last));

            if node.entry.is_folder && expanded.contains(&node.entry.id) {
                walk(&node.children, expanded, result);
            }
        }
    }

    walk(tree, expanded, &mut result);
    result
}

/// Get all descendant IDs of a node from a flat entry list.
///
/// Used to prevent circular drag-drop (can't drop a parent onto its own
/// descendant). Walks iteratively using a BFS queue.
pub fn get_descendant_ids(entries: &[KnowledgeTreeEntry], node_id: &str) -> HashSet<String> {
    // Build parent -> children mapping.
    let mut children_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for entry in entries {
        if let Some(ref pid) = entry.parent_id {
            children_of
                .entry(pid.as_str())
                .or_default()
                .push(entry.id.as_str());
        }
    }

    let mut descendants = HashSet::new();
    let mut queue = VecDeque::new();

    // Seed with direct children.
    if let Some(direct) = children_of.get(node_id) {
        for &child_id in direct {
            queue.push_back(child_id);
        }
    }

    while let Some(current) = queue.pop_front() {
        if descendants.insert(current.to_string()) {
            if let Some(kids) = children_of.get(current) {
                for &kid in kids {
                    queue.push_back(kid);
                }
            }
        }
    }

    descendants
}

/// Build a breadcrumb path for a file: `"Folder / SubFolder / File.md"`.
///
/// Walks up the `parent_id` chain from `file_id`. Returns just the name if
/// the entry is at root level.
pub fn build_path(entries: &[KnowledgeTreeEntry], file_id: &str) -> String {
    let by_id: HashMap<&str, &KnowledgeTreeEntry> =
        entries.iter().map(|e| (e.id.as_str(), e)).collect();

    let mut parts = Vec::new();
    let mut current_id = file_id;

    while let Some(entry) = by_id.get(current_id) {
        parts.push(entry.name.as_str());
        match entry.parent_id.as_deref() {
            Some(pid) if by_id.contains_key(pid) => current_id = pid,
            _ => break,
        }
    }

    parts.reverse();
    parts.join(" / ")
}

/// Get all folder entries suitable as move targets, excluding `exclude_id`
/// and its descendants.
///
/// Returns `(id, display_path)` tuples sorted alphabetically by path.
/// Used for the "Move to" context menu. Does NOT include a root-level
/// option — callers should prepend a "Root" entry if needed.
pub fn get_folder_targets(
    entries: &[KnowledgeTreeEntry],
    exclude_id: &str,
) -> Vec<(String, String)> {
    let excluded = get_descendant_ids(entries, exclude_id);

    let mut targets: Vec<(String, String)> = entries
        .iter()
        .filter(|e| e.is_folder && e.id != exclude_id && !excluded.contains(&e.id))
        .map(|e| (e.id.clone(), build_path(entries, &e.id)))
        .collect();

    targets.sort_by(|a, b| a.1.cmp(&b.1));
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        id: &str,
        parent_id: Option<&str>,
        name: &str,
        is_folder: bool,
    ) -> KnowledgeTreeEntry {
        KnowledgeTreeEntry {
            id: id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            name: name.to_string(),
            is_folder,
            sort_order: 0,
            updated_at: String::new(),
            updated_by: None,
        }
    }

    #[test]
    fn test_build_tree_sorts_folders_first() {
        let entries = vec![
            make_entry("1", None, "file.md", false),
            make_entry("2", None, "Docs", true),
        ];
        let tree = build_tree(&entries);
        assert_eq!(tree.len(), 2);
        assert!(tree[0].entry.is_folder);
        assert!(!tree[1].entry.is_folder);
    }

    #[test]
    fn test_build_tree_nests_children() {
        let entries = vec![
            make_entry("root", None, "Root", true),
            make_entry("child", Some("root"), "Child.md", false),
        ];
        let tree = build_tree(&entries);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].entry.name, "Child.md");
        assert_eq!(tree[0].children[0].depth, 1);
    }

    #[test]
    fn test_flatten_tree_respects_expanded() {
        let entries = vec![
            make_entry("f1", None, "Folder", true),
            make_entry("c1", Some("f1"), "Child.md", false),
        ];
        let tree = build_tree(&entries);

        // Collapsed: only folder visible.
        let flat = flatten_tree(&tree, &HashSet::new());
        assert_eq!(flat.len(), 1);

        // Expanded: folder + child visible.
        let mut expanded = HashSet::new();
        expanded.insert("f1".to_string());
        let flat = flatten_tree(&tree, &expanded);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[1].0.name, "Child.md");
    }

    #[test]
    fn test_get_descendant_ids() {
        let entries = vec![
            make_entry("a", None, "A", true),
            make_entry("b", Some("a"), "B", true),
            make_entry("c", Some("b"), "C", false),
            make_entry("d", None, "D", false),
        ];
        let desc = get_descendant_ids(&entries, "a");
        assert!(desc.contains("b"));
        assert!(desc.contains("c"));
        assert!(!desc.contains("d"));
        assert!(!desc.contains("a"));
    }

    #[test]
    fn test_build_path() {
        let entries = vec![
            make_entry("root", None, "Docs", true),
            make_entry("sub", Some("root"), "API", true),
            make_entry("file", Some("sub"), "README.md", false),
        ];
        assert_eq!(build_path(&entries, "file"), "Docs / API / README.md");
        assert_eq!(build_path(&entries, "root"), "Docs");
    }

    #[test]
    fn test_build_path_unknown_id_returns_empty() {
        let entries = vec![make_entry("a", None, "A", true)];
        assert_eq!(build_path(&entries, "nonexistent"), "");
    }

    #[test]
    fn test_get_folder_targets_excludes_descendants() {
        let entries = vec![
            make_entry("a", None, "A", true),
            make_entry("b", Some("a"), "B", true),
            make_entry("c", None, "C", true),
        ];
        let targets = get_folder_targets(&entries, "a");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, "c");
    }
}
