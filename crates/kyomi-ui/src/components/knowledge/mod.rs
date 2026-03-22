// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge page UI components.

pub mod create_item_modal;
pub mod file_tree;
pub mod tree_types;

pub use create_item_modal::CreateKnowledgeItemModal;
pub use file_tree::KnowledgeFileTree;
pub use tree_types::{
    build_path, build_tree, flatten_tree, get_descendant_ids, get_folder_targets, TreeNode,
};
