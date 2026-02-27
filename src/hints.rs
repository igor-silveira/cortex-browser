//! Task-context hints for focusing page snapshots on relevant content.
//!
//! When an agent sets a task context (e.g., "fill out the login form"), subsequent
//! snapshots are filtered to prioritize relevant elements and remove noise.
//! This reduces token usage by showing only what matters for the current task.

use crate::dom::{AriaRole, PageSnapshot, SemanticNode};

/// Task context that influences how snapshots are filtered.
#[derive(Debug, Clone)]
pub struct TaskContext {
    /// Description of what the agent is trying to accomplish.
    /// Stored for diagnostics and future LLM-based relevance scoring.
    #[allow(dead_code)]
    pub task: String,
    /// Text patterns to match against element names (case-insensitive substring).
    pub focus_text: Vec<String>,
    /// ARIA roles to prioritize.
    pub focus_roles: Vec<AriaRole>,
    /// If true, primarily show interactive elements and their structural parents.
    pub interactive_only: bool,
}

impl TaskContext {
    /// Score a node's relevance to this task context.
    /// Returns 0.0 for irrelevant, higher for more relevant.
    fn score_node(&self, node: &SemanticNode) -> f32 {
        let mut score: f32 = 0.0;

        if node.role.is_interactive() {
            score += 0.5;
        }
        if is_landmark(&node.role) {
            score += 0.3;
        }
        if matches!(node.role, AriaRole::Heading { .. }) {
            score += 0.4;
        }

        let name_lower = node.name.to_lowercase();
        for pattern in &self.focus_text {
            if name_lower.contains(&pattern.to_lowercase()) {
                score += 1.0;
            }
        }

        for role in &self.focus_roles {
            if node.role == *role {
                score += 1.0;
            }
        }

        if self.interactive_only
            && !node.role.is_interactive()
            && !is_landmark(&node.role)
            && !matches!(node.role, AriaRole::Heading { .. })
        {
            score *= 0.1;
        }

        score
    }

    /// Filter a PageSnapshot to only include relevant content.
    /// Keeps structural parents (landmarks, headings) of relevant nodes.
    pub fn filter_snapshot(&self, snapshot: &PageSnapshot) -> PageSnapshot {
        let threshold = if self.interactive_only { 0.3 } else { 0.2 };
        PageSnapshot {
            title: snapshot.title.clone(),
            url: snapshot.url.clone(),
            nodes: self.filter_nodes(&snapshot.nodes, threshold),
            viewport: snapshot.viewport.clone(),
        }
    }

    fn filter_nodes(&self, nodes: &[SemanticNode], threshold: f32) -> Vec<SemanticNode> {
        nodes
            .iter()
            .filter_map(|node| self.filter_node(node, threshold))
            .collect()
    }

    fn filter_node(&self, node: &SemanticNode, threshold: f32) -> Option<SemanticNode> {
        let score = self.score_node(node);

        let filtered_children = self.filter_nodes(&node.children, threshold);

        if score >= threshold || !filtered_children.is_empty() {
            Some(SemanticNode {
                ref_id: node.ref_id,
                role: node.role.clone(),
                name: node.name.clone(),
                value: node.value.clone(),
                attrs: node.attrs.clone(),
                children: filtered_children,
                offscreen: node.offscreen,
            })
        } else {
            None
        }
    }
}

/// Parse a role name string into an AriaRole.
pub fn parse_role(s: &str) -> Option<AriaRole> {
    match s.to_lowercase().as_str() {
        "button" => Some(AriaRole::Button),
        "link" => Some(AriaRole::Link),
        "textbox" | "input" => Some(AriaRole::TextBox),
        "checkbox" => Some(AriaRole::Checkbox),
        "radio" => Some(AriaRole::Radio),
        "combobox" | "select" => Some(AriaRole::ComboBox),
        "option" => Some(AriaRole::Option),
        "tab" => Some(AriaRole::Tab),
        "tabpanel" => Some(AriaRole::TabPanel),
        "dialog" => Some(AriaRole::Dialog),
        "alert" => Some(AriaRole::Alert),
        "menu" => Some(AriaRole::Menu),
        "menuitem" => Some(AriaRole::MenuItem),
        "navigation" | "nav" => Some(AriaRole::Navigation),
        "main" => Some(AriaRole::Main),
        "banner" | "header" => Some(AriaRole::Banner),
        "contentinfo" | "footer" => Some(AriaRole::ContentInfo),
        "complementary" | "aside" => Some(AriaRole::Complementary),
        "search" => Some(AriaRole::Search),
        "region" => Some(AriaRole::Region),
        "form" => Some(AriaRole::Form),
        "heading" => Some(AriaRole::Heading { level: 2 }),
        "list" => Some(AriaRole::List),
        "listitem" => Some(AriaRole::ListItem),
        "table" => Some(AriaRole::Table),
        "row" => Some(AriaRole::Row),
        "cell" => Some(AriaRole::Cell),
        "img" | "image" => Some(AriaRole::Img),
        _ => None,
    }
}

fn is_landmark(role: &AriaRole) -> bool {
    matches!(
        role,
        AriaRole::Navigation
            | AriaRole::Main
            | AriaRole::Banner
            | AriaRole::ContentInfo
            | AriaRole::Complementary
            | AriaRole::Search
            | AriaRole::Region
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(role: AriaRole, name: &str) -> SemanticNode {
        SemanticNode {
            ref_id: 0,
            role,
            name: name.into(),
            value: None,
            attrs: vec![],
            children: vec![],
            offscreen: None,
        }
    }

    fn make_ctx(
        focus_text: Vec<&str>,
        focus_roles: Vec<AriaRole>,
        interactive_only: bool,
    ) -> TaskContext {
        TaskContext {
            task: "test".into(),
            focus_text: focus_text.into_iter().map(String::from).collect(),
            focus_roles,
            interactive_only,
        }
    }

    #[test]
    fn score_interactive_gets_base() {
        let ctx = make_ctx(vec![], vec![], false);
        let node = make_node(AriaRole::Button, "Submit");
        assert!(ctx.score_node(&node) >= 0.5);
    }

    #[test]
    fn score_landmark_gets_base() {
        let ctx = make_ctx(vec![], vec![], false);
        let node = make_node(AriaRole::Main, "");
        assert!(ctx.score_node(&node) >= 0.3);
    }

    #[test]
    fn score_heading_gets_base() {
        let ctx = make_ctx(vec![], vec![], false);
        let node = make_node(AriaRole::Heading { level: 1 }, "Title");
        assert!(ctx.score_node(&node) >= 0.4);
    }

    #[test]
    fn score_text_match_boosts() {
        let ctx = make_ctx(vec!["login"], vec![], false);
        let matching = make_node(AriaRole::Button, "Login Now");
        let non_matching = make_node(AriaRole::Button, "Submit");
        assert!(ctx.score_node(&matching) > ctx.score_node(&non_matching));
    }

    #[test]
    fn score_text_match_case_insensitive() {
        let ctx = make_ctx(vec!["LOGIN"], vec![], false);
        let node = make_node(AriaRole::Button, "login button");
        assert!(ctx.score_node(&node) >= 1.0);
    }

    #[test]
    fn score_role_match_boosts() {
        let ctx = make_ctx(vec![], vec![AriaRole::TextBox], false);
        let textbox = make_node(AriaRole::TextBox, "Email");
        let button = make_node(AriaRole::Button, "Submit");
        assert!(ctx.score_node(&textbox) > ctx.score_node(&button));
    }

    #[test]
    fn interactive_only_penalizes_static() {
        let ctx = make_ctx(vec![], vec![], true);
        let text = make_node(AriaRole::StaticText, "Some text");
        let button = make_node(AriaRole::Button, "Click me");
        assert!(
            ctx.score_node(&button) > ctx.score_node(&text),
            "interactive elements should score higher than static text in interactive_only mode"
        );
    }

    #[test]
    fn filter_removes_irrelevant_nodes() {
        let snapshot = PageSnapshot {
            title: "Test".into(),
            url: "https://test.com".into(),
            nodes: vec![
                make_node(AriaRole::Button, "Login"),
                make_node(AriaRole::StaticText, "some noise"),
                make_node(AriaRole::StaticText, "more noise"),
            ],
            viewport: None,
        };
        let ctx = make_ctx(vec!["login"], vec![], false);
        let filtered = ctx.filter_snapshot(&snapshot);
        assert!(filtered.nodes.iter().any(|n| n.name.contains("Login")));
    }

    #[test]
    fn filter_preserves_parent_of_matching_child() {
        let snapshot = PageSnapshot {
            title: "Test".into(),
            url: "https://test.com".into(),
            nodes: vec![SemanticNode {
                ref_id: 0,
                role: AriaRole::Main,
                name: String::new(),
                value: None,
                attrs: vec![],
                children: vec![make_node(AriaRole::Button, "Submit")],
                offscreen: None,
            }],
            viewport: None,
        };
        let ctx = make_ctx(vec!["submit"], vec![], false);
        let filtered = ctx.filter_snapshot(&snapshot);
        assert_eq!(filtered.nodes.len(), 1, "main should be preserved");
        assert_eq!(filtered.nodes[0].role, AriaRole::Main);
        assert_eq!(filtered.nodes[0].children.len(), 1, "child should survive");
    }

    #[test]
    fn parse_role_aliases() {
        assert_eq!(parse_role("input"), Some(AriaRole::TextBox));
        assert_eq!(parse_role("select"), Some(AriaRole::ComboBox));
        assert_eq!(parse_role("nav"), Some(AriaRole::Navigation));
        assert_eq!(parse_role("header"), Some(AriaRole::Banner));
        assert_eq!(parse_role("footer"), Some(AriaRole::ContentInfo));
        assert_eq!(parse_role("aside"), Some(AriaRole::Complementary));
        assert_eq!(parse_role("image"), Some(AriaRole::Img));
    }

    #[test]
    fn parse_role_case_insensitive() {
        assert_eq!(parse_role("BUTTON"), Some(AriaRole::Button));
        assert_eq!(parse_role("Link"), Some(AriaRole::Link));
        assert_eq!(parse_role("TextBox"), Some(AriaRole::TextBox));
    }

    #[test]
    fn parse_role_unknown_returns_none() {
        assert_eq!(parse_role("foobar"), None);
        assert_eq!(parse_role(""), None);
    }

    #[test]
    fn is_landmark_checks() {
        assert!(is_landmark(&AriaRole::Navigation));
        assert!(is_landmark(&AriaRole::Main));
        assert!(is_landmark(&AriaRole::Banner));
        assert!(!is_landmark(&AriaRole::Button));
        assert!(!is_landmark(&AriaRole::TextBox));
        assert!(!is_landmark(&AriaRole::StaticText));
    }
}
