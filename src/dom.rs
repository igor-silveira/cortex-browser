use std::collections::HashMap;
use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum AriaRole {
    // Landmarks
    Banner,
    Navigation,
    Main,
    Complementary,
    ContentInfo,
    Search,
    Region,
    Form,

    // Document structure
    Heading { level: u8 },
    List,
    ListItem,
    Table,
    Row,
    Cell,
    ColumnHeader,
    Paragraph,

    // Widgets
    Button,
    Link,
    TextBox,
    Checkbox,
    Radio,
    ComboBox,
    Option,
    Tab,
    TabPanel,
    Dialog,
    Alert,
    Menu,
    MenuItem,
    Img,
    Separator,

    // Text (pseudo-role for plain text nodes)
    StaticText,

    // Generic container kept for structure
    Group,
}

impl AriaRole {
    /// Whether this role represents an interactive element that agents can act on.
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            AriaRole::Button
                | AriaRole::Link
                | AriaRole::TextBox
                | AriaRole::Checkbox
                | AriaRole::Radio
                | AriaRole::ComboBox
                | AriaRole::Option
                | AriaRole::Tab
                | AriaRole::MenuItem
                | AriaRole::Dialog
                | AriaRole::Form
        )
    }
}

impl fmt::Display for AriaRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heading { level } => write!(f, "heading[{level}]"),
            Self::StaticText => write!(f, "text"),
            Self::TextBox => write!(f, "textbox"),
            Self::ComboBox => write!(f, "combobox"),
            Self::ListItem => write!(f, "listitem"),
            Self::ColumnHeader => write!(f, "columnheader"),
            Self::TabPanel => write!(f, "tabpanel"),
            Self::MenuItem => write!(f, "menuitem"),
            Self::ContentInfo => write!(f, "contentinfo"),
            Self::Img => write!(f, "img"),
            Self::Paragraph => write!(f, "paragraph"),
            other => write!(f, "{}", format!("{other:?}").to_lowercase()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticNode {
    pub ref_id: u32,
    pub role: AriaRole,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SemanticNode>,
    /// Whether this element is outside the current viewport (set by MCP layer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offscreen: Option<bool>,
}

impl SemanticNode {
    pub fn text(content: String) -> Self {
        Self {
            ref_id: 0,
            role: AriaRole::StaticText,
            name: content,
            value: None,
            attrs: vec![],
            children: vec![],
            offscreen: None,
        }
    }
}

/// Viewport metadata for the current page state.
#[derive(Debug, Clone, Serialize)]
pub struct ViewportInfo {
    pub scroll_y: u32,
    pub viewport_height: u32,
    pub document_height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageSnapshot {
    pub title: String,
    pub url: String,
    pub nodes: Vec<SemanticNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewport: Option<ViewportInfo>,
}

/// Maps ref_id → element locator for finding elements in the live DOM.
pub type RefIndex = HashMap<u32, ElementLocator>;

/// Stores enough info about a ref'd element to locate it in the live browser DOM.
#[derive(Debug, Clone)]
pub struct ElementLocator {
    pub tag: String,
    pub id: Option<String>,
    pub name: Option<String>,
    pub input_type: Option<String>,
    pub href: Option<String>,
    pub text: String,
}

impl ElementLocator {
    /// Generate a JavaScript expression that finds this element in the live DOM.
    pub fn to_js_expression(&self) -> String {
        if let Some(id) = &self.id {
            return format!("document.getElementById('{}')", js_escape(id));
        }
        if let Some(name) = &self.name {
            let type_sel = self.input_type.as_ref()
                .map(|t| format!("[type=\"{}\"]", js_escape(t)))
                .unwrap_or_default();
            return format!(
                "document.querySelector('{}[name=\"{}\"]{}') ",
                self.tag, js_escape(name), type_sel
            );
        }
        if let Some(href) = &self.href {
            return format!("document.querySelector('a[href=\"{}\"]')", js_escape(href));
        }
        // Fallback: match by text content
        let text = js_escape(&self.text);
        format!(
            "Array.from(document.querySelectorAll('{}')).find(el => el.textContent.trim() === '{}')",
            self.tag, text
        )
    }
}

fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Result of processing HTML, including both the snapshot and the ref index.
pub struct ProcessResult {
    pub snapshot: PageSnapshot,
    pub ref_index: RefIndex,
}
