//! Page diff: compare two snapshots and produce a compact change summary.
//!
//! Uses stable ref IDs as identity for interactive elements and
//! role+name+depth for non-interactive elements. Produces Added/Removed/Modified
//! entries capped at 50 for token efficiency.

use std::collections::HashMap;

use crate::dom::{AriaRole, PageSnapshot, SemanticNode};

/// Maximum number of diff entries to return.
const MAX_DIFF_ENTRIES: usize = 50;

/// A single change between two snapshots.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffEntry {
    Added(NodeSummary),
    Removed(NodeSummary),
    Modified {
        node: NodeSummary,
        changes: Vec<FieldChange>,
    },
}

/// Compact summary of a node for diff output.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSummary {
    pub role: AriaRole,
    pub ref_id: u32,
    pub name: String,
}

/// What changed about a modified node.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldChange {
    ValueChanged { old: String, new: String },
    NameChanged { old: String, new: String },
    AttrsChanged,
    VisibilityChanged,
}

/// Result of diffing two snapshots.
pub struct DiffResult {
    pub entries: Vec<DiffEntry>,
    pub total_changes: usize,
}

#[derive(Debug)]
struct FlatNode {
    role: AriaRole,
    ref_id: u32,
    name: String,
    value: Option<String>,
    attrs: Vec<(String, String)>,
    offscreen: Option<bool>,
}

/// Flatten a tree into a map keyed by identity strings.
fn flatten_nodes(nodes: &[SemanticNode], depth: usize, out: &mut HashMap<String, FlatNode>) {
    for node in nodes {
        let identity = compute_identity(node, depth);
        out.insert(
            identity,
            FlatNode {
                role: node.role.clone(),
                ref_id: node.ref_id,
                name: node.name.clone(),
                value: node.value.clone(),
                attrs: node.attrs.clone(),
                offscreen: node.offscreen,
            },
        );
        flatten_nodes(&node.children, depth + 1, out);
    }
}

/// Compute identity key for a node.
fn compute_identity(node: &SemanticNode, depth: usize) -> String {
    if node.ref_id > 0 {
        // Interactive nodes: identity by ref ID (stable across snapshots)
        format!("ref:{}", node.ref_id)
    } else {
        // Non-interactive: identity by role + truncated name + depth
        let name_trunc = if node.name.len() > 30 {
            &node.name[..30]
        } else {
            &node.name
        };
        format!("{}:{}:{}", node.role, name_trunc, depth)
    }
}

/// Compare two page snapshots and return a list of changes.
pub fn diff_snapshots(old: &PageSnapshot, new: &PageSnapshot) -> DiffResult {
    let mut old_map = HashMap::new();
    let mut new_map = HashMap::new();
    flatten_nodes(&old.nodes, 0, &mut old_map);
    flatten_nodes(&new.nodes, 0, &mut new_map);

    let mut entries = Vec::new();

    for (key, new_node) in &new_map {
        if let Some(old_node) = old_map.get(key) {
            let changes = diff_node(old_node, new_node);
            if !changes.is_empty() {
                entries.push(DiffEntry::Modified {
                    node: NodeSummary {
                        role: new_node.role.clone(),
                        ref_id: new_node.ref_id,
                        name: new_node.name.clone(),
                    },
                    changes,
                });
            }
        } else {
            entries.push(DiffEntry::Added(NodeSummary {
                role: new_node.role.clone(),
                ref_id: new_node.ref_id,
                name: new_node.name.clone(),
            }));
        }
    }

    for (key, old_node) in &old_map {
        if !new_map.contains_key(key) {
            entries.push(DiffEntry::Removed(NodeSummary {
                role: old_node.role.clone(),
                ref_id: old_node.ref_id,
                name: old_node.name.clone(),
            }));
        }
    }

    let total = entries.len();
    entries.truncate(MAX_DIFF_ENTRIES);

    DiffResult {
        entries,
        total_changes: total,
    }
}

/// Compare two flat nodes and return field changes.
fn diff_node(old: &FlatNode, new: &FlatNode) -> Vec<FieldChange> {
    let mut changes = Vec::new();

    if old.name != new.name {
        changes.push(FieldChange::NameChanged {
            old: old.name.clone(),
            new: new.name.clone(),
        });
    }

    if old.value != new.value {
        changes.push(FieldChange::ValueChanged {
            old: old.value.clone().unwrap_or_default(),
            new: new.value.clone().unwrap_or_default(),
        });
    }

    if old.attrs != new.attrs {
        changes.push(FieldChange::AttrsChanged);
    }

    if old.offscreen != new.offscreen {
        changes.push(FieldChange::VisibilityChanged);
    }

    changes
}

/// Format a diff result into compact text output.
pub fn format_diff(diff: &DiffResult) -> String {
    if diff.entries.is_empty() {
        return "no changes".into();
    }

    let mut output = format!("diff: {} changes\n", diff.total_changes);

    for entry in &diff.entries {
        match entry {
            DiffEntry::Added(node) => {
                output.push_str("+ ");
                format_node_summary(node, &mut output);
                output.push('\n');
            }
            DiffEntry::Removed(node) => {
                output.push_str("- ");
                format_node_summary(node, &mut output);
                output.push('\n');
            }
            DiffEntry::Modified { node, changes } => {
                output.push_str("~ ");
                format_node_summary(node, &mut output);
                for change in changes {
                    match change {
                        FieldChange::ValueChanged { old, new } => {
                            output.push_str(&format!(" = \"{}\" -> \"{}\"", old, new));
                        }
                        FieldChange::NameChanged { old, new } => {
                            output.push_str(&format!(" name: \"{}\" -> \"{}\"", old, new));
                        }
                        FieldChange::AttrsChanged => {
                            output.push_str(" [attrs changed]");
                        }
                        FieldChange::VisibilityChanged => {
                            output.push_str(" [visibility changed]");
                        }
                    }
                }
                output.push('\n');
            }
        }
    }

    if diff.total_changes > MAX_DIFF_ENTRIES {
        output.push_str(&format!(
            "...and {} more changes\n",
            diff.total_changes - MAX_DIFF_ENTRIES
        ));
    }

    output
}

fn format_node_summary(node: &NodeSummary, output: &mut String) {
    output.push_str(&format!("{}", node.role));
    if node.ref_id > 0 {
        output.push_str(&format!(" @e{}", node.ref_id));
    }
    if !node.name.is_empty() {
        let display_name = if node.name.len() > 40 {
            format!("{}...", &node.name[..37])
        } else {
            node.name.clone()
        };
        output.push_str(&format!(" \"{}\"", display_name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(nodes: Vec<SemanticNode>) -> PageSnapshot {
        PageSnapshot {
            title: "Test".into(),
            url: "https://test.com".into(),
            nodes,
            viewport: None,
        }
    }

    fn make_node(role: AriaRole, name: &str, ref_id: u32) -> SemanticNode {
        SemanticNode {
            ref_id,
            role,
            name: name.into(),
            value: None,
            attrs: vec![],
            children: vec![],
            offscreen: None,
        }
    }

    fn make_node_with_value(role: AriaRole, name: &str, ref_id: u32, value: &str) -> SemanticNode {
        SemanticNode {
            ref_id,
            role,
            name: name.into(),
            value: Some(value.into()),
            attrs: vec![],
            children: vec![],
            offscreen: None,
        }
    }

    #[test]
    fn no_changes_produces_empty_diff() {
        let snap = make_snapshot(vec![make_node(AriaRole::Button, "Submit", 12345)]);
        let diff = diff_snapshots(&snap, &snap);
        assert!(diff.entries.is_empty());
        assert_eq!(diff.total_changes, 0);
    }

    #[test]
    fn added_element_detected() {
        let old = make_snapshot(vec![make_node(AriaRole::Button, "Submit", 12345)]);
        let new = make_snapshot(vec![
            make_node(AriaRole::Button, "Submit", 12345),
            make_node(AriaRole::Button, "Cancel", 23456),
        ]);
        let diff = diff_snapshots(&old, &new);
        assert!(diff
            .entries
            .iter()
            .any(|e| matches!(e, DiffEntry::Added(n) if n.name == "Cancel")));
    }

    #[test]
    fn removed_element_detected() {
        let old = make_snapshot(vec![
            make_node(AriaRole::Button, "Submit", 12345),
            make_node(AriaRole::Button, "Cancel", 23456),
        ]);
        let new = make_snapshot(vec![make_node(AriaRole::Button, "Submit", 12345)]);
        let diff = diff_snapshots(&old, &new);
        assert!(diff
            .entries
            .iter()
            .any(|e| matches!(e, DiffEntry::Removed(n) if n.name == "Cancel")));
    }

    #[test]
    fn value_change_detected() {
        let old = make_snapshot(vec![make_node_with_value(
            AriaRole::TextBox,
            "Name",
            12345,
            "",
        )]);
        let new = make_snapshot(vec![make_node_with_value(
            AriaRole::TextBox,
            "Name",
            12345,
            "John",
        )]);
        let diff = diff_snapshots(&old, &new);
        assert_eq!(diff.total_changes, 1);
        match &diff.entries[0] {
            DiffEntry::Modified { changes, .. } => {
                assert!(changes
                    .iter()
                    .any(|c| matches!(c, FieldChange::ValueChanged { new, .. } if new == "John")));
            }
            _ => panic!("expected Modified"),
        }
    }

    #[test]
    fn format_diff_added() {
        let diff = DiffResult {
            entries: vec![DiffEntry::Added(NodeSummary {
                role: AriaRole::Button,
                ref_id: 12345,
                name: "Submit".into(),
            })],
            total_changes: 1,
        };
        let text = format_diff(&diff);
        assert!(
            text.contains("+ button @e12345 \"Submit\""),
            "output: {text}"
        );
    }

    #[test]
    fn format_diff_removed() {
        let diff = DiffResult {
            entries: vec![DiffEntry::Removed(NodeSummary {
                role: AriaRole::Button,
                ref_id: 12345,
                name: "Submit".into(),
            })],
            total_changes: 1,
        };
        let text = format_diff(&diff);
        assert!(
            text.contains("- button @e12345 \"Submit\""),
            "output: {text}"
        );
    }

    #[test]
    fn format_diff_modified() {
        let diff = DiffResult {
            entries: vec![DiffEntry::Modified {
                node: NodeSummary {
                    role: AriaRole::TextBox,
                    ref_id: 23456,
                    name: "Name".into(),
                },
                changes: vec![FieldChange::ValueChanged {
                    old: "".into(),
                    new: "John".into(),
                }],
            }],
            total_changes: 1,
        };
        let text = format_diff(&diff);
        assert!(
            text.contains("~ textbox @e23456 \"Name\""),
            "output: {text}"
        );
        assert!(text.contains("\"\" -> \"John\""), "output: {text}");
    }

    #[test]
    fn format_no_changes() {
        let diff = DiffResult {
            entries: vec![],
            total_changes: 0,
        };
        assert_eq!(format_diff(&diff), "no changes");
    }

    #[test]
    fn diff_with_non_interactive_nodes() {
        let old = make_snapshot(vec![make_node(
            AriaRole::Heading { level: 1 },
            "Old Title",
            0,
        )]);
        let new = make_snapshot(vec![make_node(
            AriaRole::Heading { level: 1 },
            "New Title",
            0,
        )]);
        let diff = diff_snapshots(&old, &new);
        assert!(
            diff.total_changes >= 1,
            "heading name change should be detected"
        );
    }
}
