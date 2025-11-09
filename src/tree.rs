//! Hierarchical project tree structure with lazy loading support
//!
//! This module provides a tree-based view of the file system where:
//! - Directories can be expanded/collapsed
//! - Projects (Cargo.toml files) are leaf nodes
//! - Subdirectories are only scanned when expanded (lazy loading)
//! - The tree state is persisted in AppState

use crate::project::Project;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A node in the project tree, either a directory or a project
#[derive(Debug, Clone)]
pub enum TreeNodeType {
    /// A directory that can be expanded to show subdirectories and projects
    Directory {
        /// Name of the directory
        name: String,
        /// Absolute path to the directory
        path: PathBuf,
    },
    /// A leaf node representing a Rust project
    Project(Project),
}

impl TreeNodeType {
    /// Get the display name of this node
    pub fn name(&self) -> &str {
        match self {
            TreeNodeType::Directory { name, .. } => name,
            TreeNodeType::Project(p) => &p.name,
        }
    }

    /// Get the path of this node
    pub fn path(&self) -> &Path {
        match self {
            TreeNodeType::Directory { path, .. } => path,
            TreeNodeType::Project(p) => &p.path,
        }
    }

    /// Check if this is a directory node
    pub fn is_directory(&self) -> bool {
        matches!(self, TreeNodeType::Directory { .. })
    }

    /// Check if this is a project node
    pub fn is_project(&self) -> bool {
        matches!(self, TreeNodeType::Project(_))
    }
}

/// A node in the tree with its metadata and children
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// The node content (directory or project)
    pub node_type: TreeNodeType,
    /// Child nodes (only populated if expanded)
    pub children: Vec<TreeNode>,
    /// Whether this directory node is expanded
    pub expanded: bool,
    /// Whether children have been loaded from disk (for lazy loading)
    pub children_loaded: bool,
    /// Depth in the tree (for indentation)
    pub depth: usize,
}

impl TreeNode {
    /// Create a new directory node
    pub fn directory(name: String, path: PathBuf, depth: usize) -> Self {
        Self {
            node_type: TreeNodeType::Directory { name, path },
            children: Vec::new(),
            expanded: depth == 0, // Only expand root directory by default
            children_loaded: false,
            depth,
        }
    }

    /// Create a new project node
    pub fn project(project: Project, depth: usize) -> Self {
        Self {
            node_type: TreeNodeType::Project(project),
            children: Vec::new(),
            expanded: false,
            children_loaded: true, // Projects don't have children
            depth,
        }
    }

    /// Toggle the expanded state of this directory node
    pub fn toggle_expanded(&mut self) {
        if self.node_type.is_directory() {
            self.expanded = !self.expanded;
        }
    }

    /// Get all projects in this subtree
    pub fn collect_projects(&self) -> Vec<&Project> {
        let mut projects = Vec::new();

        match &self.node_type {
            TreeNodeType::Project(p) => {
                projects.push(p);
            }
            TreeNodeType::Directory { .. } => {
                for child in &self.children {
                    projects.extend(child.collect_projects());
                }
            }
        }

        projects
    }
}

/// Flattened view of the tree for rendering and navigation
#[derive(Debug, Clone)]
pub struct FlattenedTree {
    /// Flat list of nodes in display order
    pub items: Vec<(TreeNode, usize)>, // (node, index in flat list)
    /// Map from path to flat index for quick lookup
    pub path_to_index: HashMap<PathBuf, usize>,
}

impl FlattenedTree {
    /// Create an empty flattened tree
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            path_to_index: HashMap::new(),
        }
    }

    /// Build a flattened view from a tree, respecting expanded state
    pub fn from_tree(root: &TreeNode) -> Self {
        let mut flattened = FlattenedTree::new();
        let mut index = 0;
        flattened.flatten_recursive(root, &mut index);
        flattened
    }

    fn flatten_recursive(&mut self, node: &TreeNode, index: &mut usize) {
        // Add this node to the flat list
        self.items.push((node.clone(), *index));
        self.path_to_index
            .insert(node.node_type.path().to_path_buf(), *index);
        *index += 1;

        // If this is an expanded directory, add its children
        if node.node_type.is_directory() && node.expanded {
            for child in &node.children {
                self.flatten_recursive(child, index);
            }
        }
    }

    /// Get the flat index of a node by its path
    pub fn get_index(&self, path: &Path) -> Option<usize> {
        self.path_to_index.get(path).copied()
    }
}

/// Manages the selection state of a tree node
#[derive(Debug, Clone)]
pub struct TreeSelectionState {
    /// Index of the currently selected item in the flattened tree
    pub selected_index: Option<usize>,
    /// Set of selected project paths (for multi-select)
    pub selected_projects: HashSet<String>,
}

impl TreeSelectionState {
    /// Create a new selection state
    pub fn new() -> Self {
        Self {
            selected_index: Some(0),
            selected_projects: HashSet::new(),
        }
    }

    /// Select the next item
    pub fn select_next(&mut self, max_index: usize) {
        if let Some(idx) = self.selected_index {
            if idx < max_index {
                self.selected_index = Some(idx + 1);
            }
        }
    }

    /// Select the previous item
    pub fn select_prev(&mut self) {
        if let Some(idx) = self.selected_index {
            if idx > 0 {
                self.selected_index = Some(idx - 1);
            }
        }
    }

    /// Toggle selection of a project
    pub fn toggle_project(&mut self, name: String) {
        if self.selected_projects.contains(&name) {
            self.selected_projects.remove(&name);
        } else {
            self.selected_projects.insert(name);
        }
    }

    /// Check if a project is selected
    pub fn is_project_selected(&self, name: &str) -> bool {
        self.selected_projects.contains(name)
    }
}

impl Default for TreeSelectionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_node_creation() {
        let node = TreeNode::directory("test".to_string(), PathBuf::from("/test"), 0);
        assert!(node.node_type.is_directory());
        assert_eq!(node.node_type.name(), "test");
    }

    #[test]
    fn test_tree_toggle_expanded() {
        let mut node = TreeNode::directory("test".to_string(), PathBuf::from("/test"), 0);
        assert!(node.expanded);
        node.toggle_expanded();
        assert!(!node.expanded);
        node.toggle_expanded();
        assert!(node.expanded);
    }

    #[test]
    fn test_selection_state() {
        let mut state = TreeSelectionState::new();
        assert_eq!(state.selected_index, Some(0));

        state.select_next(5);
        assert_eq!(state.selected_index, Some(1));

        state.select_prev();
        assert_eq!(state.selected_index, Some(0));

        state.toggle_project("proj1".to_string());
        assert!(state.is_project_selected("proj1"));

        state.toggle_project("proj1".to_string());
        assert!(!state.is_project_selected("proj1"));
    }
}
