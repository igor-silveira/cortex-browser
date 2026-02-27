use crate::dom::{AriaRole, PageSnapshot, SemanticNode};

/// Serialize a PageSnapshot into the compact text format designed for LLM consumption.
///
/// Example output:
/// ```text
/// page: "GitHub - Login" [github.com]
/// ---
/// heading[1]: "Sign in to GitHub"
/// form @e1:
///   textbox @e2 "Username or email address"
///   textbox @e3 "Password" (password)
///   checkbox @e4 "Remember me" [unchecked]
///   button @e5 "Sign in"
/// link @e6: "Forgot password?"
/// ```
pub fn to_compact_text(snapshot: &PageSnapshot) -> String {
    let mut output = String::new();

    if !snapshot.title.is_empty() || !snapshot.url.is_empty() {
        output.push_str(&format!("page: \"{}\"", snapshot.title));
        if !snapshot.url.is_empty() {
            output.push_str(&format!(" [{}]", snapshot.url));
        }
        output.push('\n');

        if let Some(vp) = &snapshot.viewport {
            let bottom = vp.scroll_y + vp.viewport_height;
            output.push_str(&format!(
                "viewport: {}-{} of {}px\n",
                vp.scroll_y, bottom, vp.document_height
            ));
        }

        output.push_str("---\n");
    }

    for node in &snapshot.nodes {
        serialize_node(node, 0, &mut output);
    }

    output
}

fn serialize_node(node: &SemanticNode, indent: usize, output: &mut String) {
    let prefix = "  ".repeat(indent);

    match &node.role {
        AriaRole::StaticText => {
            output.push_str(&format!("{}{}\n", prefix, node.name));
        }
        role => {
            output.push_str(&format!("{}{}", prefix, role));

            if node.ref_id > 0 {
                output.push_str(&format!(" @e{}", node.ref_id));
            }

            if node.offscreen == Some(true) {
                output.push_str(" [offscreen]");
            }

            // Name: skip for containers whose children convey the content.
            // But if children would be suppressed (redundant), show the name to avoid
            // losing content entirely.
            let suppress_name =
                is_container_with_children(node) && !has_redundant_children(node);
            if !node.name.is_empty() && !suppress_name {
                output.push_str(&format!(" \"{}\"", node.name));
            }

            serialize_attrs(node, output);

            if let Some(val) = &node.value {
                if !val.is_empty() {
                    output.push_str(&format!(" = \"{val}\""));
                }
            }

            if node.children.is_empty() || has_redundant_children(node) {
                output.push('\n');
            } else {
                output.push_str(":\n");
                for child in &node.children {
                    serialize_node(child, indent + 1, output);
                }
            }
        }
    }
}

fn serialize_attrs(node: &SemanticNode, output: &mut String) {
    if matches!(node.role, AriaRole::Checkbox | AriaRole::Radio) {
        let is_checked = node.attrs.iter().any(|(k, _)| k == "checked");
        if is_checked {
            output.push_str(" [checked]");
        } else {
            output.push_str(" [unchecked]");
        }
    }

    for (key, val) in &node.attrs {
        match key.as_str() {
            "type" if val != "text" => output.push_str(&format!(" ({val})")),
            "disabled" => output.push_str(" [disabled]"),
            "required" => output.push_str(" [required]"),
            "href" => output.push_str(&format!(" -> {val}")),
            "checked" => {} // handled above
            "type" => {}    // text type is default, skip
            _ => {}
        }
    }
}

/// Check if a node's children are just a single text node that repeats the node's name.
/// In that case we suppress the children to avoid duplication like:
///   link @e1 "GitHub":
///     GitHub
/// Instead we emit: link @e1 "GitHub"
fn has_redundant_children(node: &SemanticNode) -> bool {
    node.children.len() == 1
        && node.children[0].role == AriaRole::StaticText
        && node.children[0].name == node.name
}

/// Container roles whose name is just the concatenation of child text -
/// we skip printing the name to avoid redundancy.
fn is_container_with_children(node: &SemanticNode) -> bool {
    if node.children.is_empty() {
        return false;
    }
    matches!(
        node.role,
        AriaRole::List
            | AriaRole::Navigation
            | AriaRole::Main
            | AriaRole::Banner
            | AriaRole::ContentInfo
            | AriaRole::Complementary
            | AriaRole::Region
            | AriaRole::Form
            | AriaRole::Table
            | AriaRole::Row
            | AriaRole::Paragraph
            | AriaRole::Group
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: AriaRole, name: &str, ref_id: u32) -> SemanticNode {
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

    fn node_with_children(
        role: AriaRole,
        name: &str,
        ref_id: u32,
        children: Vec<SemanticNode>,
    ) -> SemanticNode {
        SemanticNode {
            ref_id,
            role,
            name: name.into(),
            value: None,
            attrs: vec![],
            children,
            offscreen: None,
        }
    }

    fn node_with_attrs(
        role: AriaRole,
        name: &str,
        ref_id: u32,
        attrs: Vec<(&str, &str)>,
    ) -> SemanticNode {
        SemanticNode {
            ref_id,
            role,
            name: name.into(),
            value: None,
            attrs: attrs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            children: vec![],
            offscreen: None,
        }
    }

    #[test]
    fn header_with_title_and_url() {
        let snap = PageSnapshot {
            title: "My Page".into(),
            url: "https://example.com".into(),
            nodes: vec![],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.starts_with("page: \"My Page\" [https://example.com]"));
        assert!(text.contains("---"));
    }

    #[test]
    fn header_with_title_only() {
        let snap = PageSnapshot {
            title: "Title Only".into(),
            url: String::new(),
            nodes: vec![],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("page: \"Title Only\""));
        assert!(!text.contains("["));
    }

    #[test]
    fn no_header_when_empty() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node(AriaRole::StaticText, "Hello", 0)],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(!text.contains("page:"));
        assert!(text.contains("Hello"));
    }

    #[test]
    fn static_text_renders_plainly() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node(AriaRole::StaticText, "Just text", 0)],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert_eq!(text.trim(), "Just text");
    }

    #[test]
    fn button_with_ref() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node(AriaRole::Button, "Submit", 1)],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("button @e1 \"Submit\""));
    }

    #[test]
    fn link_with_href() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_attrs(
                AriaRole::Link,
                "Home",
                1,
                vec![("href", "/home")],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("link @e1 \"Home\" -> /home"));
    }

    #[test]
    fn checkbox_unchecked() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node(AriaRole::Checkbox, "Remember me", 1)],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("[unchecked]"));
    }

    #[test]
    fn checkbox_checked() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_attrs(
                AriaRole::Checkbox,
                "Accept terms",
                1,
                vec![("checked", "true")],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("[checked]"));
    }

    #[test]
    fn disabled_attribute() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_attrs(
                AriaRole::Button,
                "Disabled",
                1,
                vec![("disabled", "true")],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("[disabled]"));
    }

    #[test]
    fn required_attribute() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_attrs(
                AriaRole::TextBox,
                "Email",
                1,
                vec![("required", "true")],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("[required]"));
    }

    #[test]
    fn password_type_shown() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_attrs(
                AriaRole::TextBox,
                "Password",
                1,
                vec![("type", "password")],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("(password)"));
    }

    #[test]
    fn input_value_shown() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![SemanticNode {
                ref_id: 1,
                role: AriaRole::TextBox,
                name: "Name".into(),
                value: Some("John".into()),
                attrs: vec![],
                children: vec![],
                offscreen: None,
            }],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("= \"John\""));
    }

    #[test]
    fn children_indented() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_children(
                AriaRole::Navigation,
                "",
                0,
                vec![
                    node(AriaRole::Link, "Home", 1),
                    node(AriaRole::Link, "About", 2),
                ],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("navigation:\n"));
        assert!(text.contains("  link @e1 \"Home\""));
        assert!(text.contains("  link @e2 \"About\""));
    }

    #[test]
    fn redundant_child_text_suppressed() {
        // A link whose only child is text matching its name
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_children(
                AriaRole::Link,
                "Click here",
                1,
                vec![node(AriaRole::StaticText, "Click here", 0)],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        // Should NOT have "Click here" appearing as a child line
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "should be a single line, no child: {text}");
        assert!(text.contains("link @e1 \"Click here\""));
    }

    #[test]
    fn container_name_suppressed_when_has_non_redundant_children() {
        // A navigation with multiple children - the name (aggregated text) should be suppressed
        // because the children convey the content.
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_children(
                AriaRole::Navigation,
                "Home About",
                0,
                vec![
                    node(AriaRole::Link, "Home", 1),
                    node(AriaRole::Link, "About", 2),
                ],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        let nav_line = text.lines().find(|l| l.contains("navigation")).unwrap();
        assert!(
            !nav_line.contains("Home About"),
            "container name should be suppressed when children are non-redundant: {nav_line}"
        );
    }

    #[test]
    fn container_name_shown_when_children_are_redundant() {
        // A paragraph with a single text child matching its name - the name should be shown
        // (since children will be suppressed as redundant).
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![node_with_children(
                AriaRole::Paragraph,
                "Hello world",
                0,
                vec![node(AriaRole::StaticText, "Hello world", 0)],
            )],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(
            text.contains("Hello world"),
            "paragraph with single text child should show its name: {text}"
        );
    }

    #[test]
    fn heading_level_in_output() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![
                node(AriaRole::Heading { level: 1 }, "Title", 0),
                node(AriaRole::Heading { level: 3 }, "Subtitle", 0),
            ],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("heading[1] \"Title\""));
        assert!(text.contains("heading[3] \"Subtitle\""));
    }

    #[test]
    fn viewport_header_rendered() {
        let snap = PageSnapshot {
            title: "Test".into(),
            url: "https://test.com".into(),
            nodes: vec![],
            viewport: Some(crate::dom::ViewportInfo {
                scroll_y: 0,
                viewport_height: 900,
                document_height: 4200,
            }),
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("viewport: 0-900 of 4200px"), "viewport line: {text}");
    }

    #[test]
    fn viewport_header_absent_when_none() {
        let snap = PageSnapshot {
            title: "Test".into(),
            url: "https://test.com".into(),
            nodes: vec![],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(!text.contains("viewport:"), "no viewport line when None");
    }

    #[test]
    fn offscreen_annotation_rendered() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![SemanticNode {
                ref_id: 12345,
                role: AriaRole::Link,
                name: "Terms".into(),
                value: None,
                attrs: vec![("href".into(), "/terms".into())],
                children: vec![],
                offscreen: Some(true),
            }],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(text.contains("[offscreen]"), "offscreen annotation: {text}");
        assert!(text.contains("@e12345 [offscreen]"), "offscreen after ref: {text}");
    }

    #[test]
    fn onscreen_element_no_annotation() {
        let snap = PageSnapshot {
            title: String::new(),
            url: String::new(),
            nodes: vec![SemanticNode {
                ref_id: 12345,
                role: AriaRole::Button,
                name: "Submit".into(),
                value: None,
                attrs: vec![],
                children: vec![],
                offscreen: Some(false),
            }],
            viewport: None,
        };
        let text = to_compact_text(&snap);
        assert!(!text.contains("[offscreen]"), "visible elements should not have [offscreen]: {text}");
    }
}
