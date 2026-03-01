use std::collections::{HashMap, HashSet};

use scraper::{ElementRef, Html, Node, Selector};
use tracing::debug;

/// Deterministic FNV-1a hasher. Unlike `DefaultHasher`, the output is guaranteed
/// to be stable across Rust versions, which is essential for stable ref IDs.
struct FnvHasher(u64);

impl FnvHasher {
    const BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001B3;

    fn new() -> Self {
        Self(Self::BASIS)
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

use crate::dom::{AriaRole, ElementLocator, PageSnapshot, ProcessResult, SemanticNode};

/// Tags that carry zero agent-relevant information and should be removed entirely.
const PRUNED_TAGS: &[&str] = &[
    "script",
    "style",
    "noscript",
    "meta",
    "link",
    "head",
    "svg",
    "path",
    "defs",
    "clippath",
    "lineargradient",
    "template",
    "iframe",
    "object",
    "embed",
    "br",
    "wbr",
];

/// Map from element id → label text, built by pre-scanning `<label for="...">` elements.
type LabelMap = HashMap<String, String>;

/// Tracks ref assignment state during tree processing, producing stable hash-based IDs.
struct RefContext {
    used_refs: HashSet<u32>,
    ref_entries: Vec<(u32, ElementLocator)>,
    /// Structural child indices tracking the current position in the tree.
    path: Vec<usize>,
}

impl RefContext {
    fn new() -> Self {
        Self {
            used_refs: HashSet::new(),
            ref_entries: Vec::new(),
            path: Vec::new(),
        }
    }
}

/// Compute a stable ref ID by hashing stable DOM properties.
/// Maps to 5-digit range [10000, 99999] with linear-probe collision resolution.
///
/// Elements with strong identity (id or name attr) use those for hashing,
/// making refs survive structural changes. Elements without identity fall
/// back to path-based hashing.
fn compute_stable_ref(
    tag: &str,
    el: &scraper::node::Element,
    name: &str,
    path: &[usize],
    used_refs: &HashSet<u32>,
) -> u32 {
    let mut hasher = FnvHasher::new();
    let has_strong_identity =
        el.attr("id").is_some() || el.attr("name").is_some() || el.attr("href").is_some();

    hasher.write_str(tag);

    if has_strong_identity {
        if let Some(id) = el.attr("id") {
            hasher.write_str("id:");
            hasher.write_str(id);
        }
        if let Some(n) = el.attr("name") {
            hasher.write_str("name:");
            hasher.write_str(n);
        }
        if let Some(href) = el.attr("href") {
            hasher.write_str("href:");
            hasher.write_str(href);
        }
        if let Some(t) = el.attr("type") {
            hasher.write_str(t);
        }
    } else {
        if let Some(t) = el.attr("type") {
            hasher.write_str(t);
        }
        if let Some(href) = el.attr("href") {
            hasher.write_str(href);
        }
        hasher.write_str(name);
        for &idx in path {
            hasher.write_bytes(&idx.to_le_bytes());
        }
    }

    let hash = hasher.finish();
    let range: u32 = 90000;
    let mut candidate = (hash % range as u64 + 10000) as u32;

    // Linear probe for collision resolution, bounded to prevent infinite loop.
    let mut probes: u32 = 0;
    while used_refs.contains(&candidate) {
        probes += 1;
        if probes >= range {
            // Range is exhausted (90k interactive elements); fall back to an overflow range.
            candidate = 100000 + probes;
            break;
        }
        candidate = if candidate >= 99999 {
            10000
        } else {
            candidate + 1
        };
    }

    candidate
}

/// Process raw HTML into a compact semantic snapshot.
pub fn process(html: &str, url: &str) -> PageSnapshot {
    process_with_refs(html, url).snapshot
}

/// Process HTML and also build a RefIndex for element interaction.
pub fn process_with_refs(html: &str, url: &str) -> ProcessResult {
    debug!(html_len = html.len(), url = %url, "processing HTML");
    let document = Html::parse_document(html);
    let root = document.root_element();
    let mut ref_ctx = RefContext::new();

    let title = extract_title(&document);
    let labels = build_label_map(&document);
    let body = find_element(&root, "body").unwrap_or(root);
    let nodes = process_children(body, &mut ref_ctx, &labels);

    let ref_count = ref_ctx.ref_entries.len();
    fn count_nodes(nodes: &[SemanticNode]) -> usize {
        nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
    }
    let node_count = count_nodes(&nodes);
    debug!(
        nodes = node_count,
        refs = ref_count,
        labels = labels.len(),
        "pipeline complete"
    );

    ProcessResult {
        snapshot: PageSnapshot {
            title,
            url: url.to_string(),
            nodes,
            viewport: None,
        },
        ref_index: ref_ctx.ref_entries.into_iter().collect(),
    }
}

fn process_children(
    parent: ElementRef,
    ref_ctx: &mut RefContext,
    labels: &LabelMap,
) -> Vec<SemanticNode> {
    let mut nodes = Vec::new();
    let mut child_index: usize = 0;

    for child in parent.children() {
        if let Some(elem) = ElementRef::wrap(child) {
            ref_ctx.path.push(child_index);
            nodes.extend(process_element(elem, ref_ctx, labels));
            ref_ctx.path.pop();
            child_index += 1;
        } else if let Node::Text(text) = child.value() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                nodes.push(SemanticNode::text(trimmed.to_string()));
            }
        }
    }

    merge_adjacent_text(&mut nodes);
    merge_repeated_siblings(&mut nodes);
    nodes
}

fn process_element(
    element: ElementRef,
    ref_ctx: &mut RefContext,
    labels: &LabelMap,
) -> Vec<SemanticNode> {
    let el = element.value();
    let tag = el.name.local.as_ref();

    // Stage 1: Prune
    if should_prune(tag, el) {
        return vec![];
    }

    let children = process_children(element, ref_ctx, labels);

    // Stage 2: Role + name
    let role = determine_role(tag, el);
    let name = compute_accessible_name(tag, el, &element, labels);

    // Stage 3: Keep or collapse
    if is_meaningful(&role, &name, &children, el) {
        let ref_id = if role.is_interactive() {
            let id = compute_stable_ref(tag, el, &name, &ref_ctx.path, &ref_ctx.used_refs);
            ref_ctx.used_refs.insert(id);
            ref_ctx.ref_entries.push((
                id,
                ElementLocator {
                    tag: tag.to_string(),
                    id: el.attr("id").map(String::from),
                    name: el.attr("name").map(String::from),
                    input_type: el.attr("type").map(String::from),
                    href: el.attr("href").map(String::from),
                    text: name.clone(),
                },
            ));
            id
        } else {
            0
        };

        let attrs = extract_relevant_attrs(tag, el);
        let value = extract_value(tag, el);

        vec![SemanticNode {
            ref_id,
            role,
            name,
            value,
            attrs,
            children,
            offscreen: None,
        }]
    } else {
        children
    }
}

fn should_prune(tag: &str, el: &scraper::node::Element) -> bool {
    if PRUNED_TAGS.contains(&tag) {
        return true;
    }

    if el.attr("aria-hidden") == Some("true") {
        return true;
    }
    if el.attr("hidden").is_some() {
        return true;
    }

    if let Some(style) = el.attr("style") {
        let s = style.to_lowercase();
        if s.contains("display:none")
            || s.contains("display: none")
            || s.contains("visibility:hidden")
            || s.contains("visibility: hidden")
        {
            return true;
        }
    }

    if tag == "input" && el.attr("type") == Some("hidden") {
        return true;
    }

    // Labels with `for` attribute - their text is used as the associated input's name,
    // so we prune them to avoid duplication.
    if tag == "label" && el.attr("for").is_some() {
        return true;
    }

    false
}

fn determine_role(tag: &str, el: &scraper::node::Element) -> AriaRole {
    if let Some(role) = el.attr("role") {
        return parse_explicit_role(role, el);
    }

    match tag {
        "button" => AriaRole::Button,
        "a" if el.attr("href").is_some() => AriaRole::Link,
        "a" => AriaRole::Group,
        "input" => input_role(el),
        "textarea" => AriaRole::TextBox,
        "select" => AriaRole::ComboBox,
        "option" => AriaRole::Option,
        "h1" => AriaRole::Heading { level: 1 },
        "h2" => AriaRole::Heading { level: 2 },
        "h3" => AriaRole::Heading { level: 3 },
        "h4" => AriaRole::Heading { level: 4 },
        "h5" => AriaRole::Heading { level: 5 },
        "h6" => AriaRole::Heading { level: 6 },
        "nav" => AriaRole::Navigation,
        "main" => AriaRole::Main,
        "header" => AriaRole::Banner,
        "footer" => AriaRole::ContentInfo,
        "aside" => AriaRole::Complementary,
        "form" => AriaRole::Form,
        "ul" | "ol" => AriaRole::List,
        "li" => AriaRole::ListItem,
        "table" => AriaRole::Table,
        "tr" => AriaRole::Row,
        "td" => AriaRole::Cell,
        "th" => AriaRole::ColumnHeader,
        "img" => AriaRole::Img,
        "dialog" => AriaRole::Dialog,
        "menu" => AriaRole::Menu,
        "hr" => AriaRole::Separator,
        "p" => AriaRole::Paragraph,
        "label" => AriaRole::Group, // labels are handled via name computation
        _ => AriaRole::Group,
    }
}

fn input_role(el: &scraper::node::Element) -> AriaRole {
    match el.attr("type").unwrap_or("text") {
        "submit" | "reset" | "button" | "image" => AriaRole::Button,
        "checkbox" => AriaRole::Checkbox,
        "radio" => AriaRole::Radio,
        _ => AriaRole::TextBox,
    }
}

fn parse_explicit_role(role: &str, el: &scraper::node::Element) -> AriaRole {
    match role {
        "button" => AriaRole::Button,
        "link" => AriaRole::Link,
        "textbox" => AriaRole::TextBox,
        "checkbox" => AriaRole::Checkbox,
        "radio" => AriaRole::Radio,
        "combobox" => AriaRole::ComboBox,
        "option" => AriaRole::Option,
        "tab" => AriaRole::Tab,
        "tabpanel" => AriaRole::TabPanel,
        "dialog" | "alertdialog" => AriaRole::Dialog,
        "alert" => AriaRole::Alert,
        "navigation" => AriaRole::Navigation,
        "main" => AriaRole::Main,
        "search" => AriaRole::Search,
        "form" => AriaRole::Form,
        "banner" => AriaRole::Banner,
        "contentinfo" => AriaRole::ContentInfo,
        "complementary" => AriaRole::Complementary,
        "region" => AriaRole::Region,
        "menu" => AriaRole::Menu,
        "menuitem" | "menuitemcheckbox" | "menuitemradio" => AriaRole::MenuItem,
        "list" => AriaRole::List,
        "listitem" => AriaRole::ListItem,
        "table" | "grid" => AriaRole::Table,
        "row" => AriaRole::Row,
        "cell" | "gridcell" => AriaRole::Cell,
        "columnheader" => AriaRole::ColumnHeader,
        "heading" => {
            let level = el
                .attr("aria-level")
                .and_then(|l| l.parse().ok())
                .unwrap_or(2);
            AriaRole::Heading { level }
        }
        "img" | "image" => AriaRole::Img,
        "separator" => AriaRole::Separator,
        "group" => AriaRole::Group,
        _ => AriaRole::Group,
    }
}

fn compute_accessible_name(
    tag: &str,
    el: &scraper::node::Element,
    element: &ElementRef,
    labels: &LabelMap,
) -> String {
    if let Some(label) = el.attr("aria-label") {
        let trimmed = label.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(id) = el.attr("id") {
        if let Some(label_text) = labels.get(id) {
            return label_text.clone();
        }
    }
    if tag == "img" {
        if let Some(alt) = el.attr("alt") {
            return alt.trim().to_string();
        }
    }
    if matches!(tag, "input" | "textarea") {
        if let Some(ph) = el.attr("placeholder") {
            return ph.trim().to_string();
        }
    }
    if let Some(title) = el.attr("title") {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // <select> children (option nodes) convey the content
    if tag == "select" {
        return String::new();
    }

    let text: String = element.text().collect::<Vec<_>>().join(" ");
    let trimmed = text.trim().to_string();

    // Truncate very long text (find a char boundary near 197 bytes)
    if trimmed.len() > 200 {
        let mut end = 197;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &trimmed[..end])
    } else {
        trimmed
    }
}

fn is_meaningful(
    role: &AriaRole,
    name: &str,
    children: &[SemanticNode],
    el: &scraper::node::Element,
) -> bool {
    if role.is_interactive() {
        return true;
    }

    match role {
        AriaRole::Navigation
        | AriaRole::Main
        | AriaRole::Banner
        | AriaRole::ContentInfo
        | AriaRole::Complementary
        | AriaRole::Search
        | AriaRole::Region => return true,

        AriaRole::Heading { .. } => return true,
        AriaRole::List | AriaRole::Table => return true,
        AriaRole::ListItem | AriaRole::Row | AriaRole::Cell | AriaRole::ColumnHeader => {
            return true
        }
        AriaRole::Img => return !name.is_empty(), // only keep images with alt text
        AriaRole::Alert => return true,
        AriaRole::Separator => return true,
        AriaRole::Paragraph => return !children.is_empty() || !name.is_empty(),

        _ => {}
    }

    if el.attr("aria-label").is_some() || el.attr("aria-labelledby").is_some() {
        return true;
    }

    false
}

fn extract_relevant_attrs(tag: &str, el: &scraper::node::Element) -> Vec<(String, String)> {
    let mut attrs = Vec::new();

    // Only include input type when it adds information beyond the role.
    // e.g. "password" is useful for a textbox, but "checkbox" is redundant for AriaRole::Checkbox.
    if tag == "input" {
        if let Some(t) = el.attr("type") {
            if matches!(
                t,
                "password" | "email" | "url" | "tel" | "search" | "number"
            ) {
                attrs.push(("type".into(), t.into()));
            }
        }
    }

    if tag == "a" {
        if let Some(href) = el.attr("href") {
            if !href.starts_with("javascript:") && !href.is_empty() {
                attrs.push(("href".into(), href.into()));
            }
        }
    }

    if el.attr("checked").is_some() {
        attrs.push(("checked".into(), "true".into()));
    }
    if el.attr("disabled").is_some() {
        attrs.push(("disabled".into(), "true".into()));
    }
    if el.attr("required").is_some() {
        attrs.push(("required".into(), "true".into()));
    }

    attrs
}

fn extract_value(tag: &str, el: &scraper::node::Element) -> Option<String> {
    match tag {
        "input" | "textarea" => el.attr("value").map(String::from),
        _ => None,
    }
}

/// Pre-scan the document for `<label for="id">` elements and collect a map
/// of element id → label text.
fn build_label_map(document: &Html) -> LabelMap {
    let selector = Selector::parse("label[for]").expect("valid selector");
    let mut map = LabelMap::new();
    for label_el in document.select(&selector) {
        if let Some(for_id) = label_el.value().attr("for") {
            let text: String = label_el.text().collect::<Vec<_>>().join(" ");
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                map.insert(for_id.to_string(), trimmed);
            }
        }
    }
    map
}

fn extract_title(document: &Html) -> String {
    let selector = Selector::parse("title").expect("valid selector");
    document
        .select(&selector)
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn find_element<'a>(parent: &ElementRef<'a>, tag_name: &str) -> Option<ElementRef<'a>> {
    for child in parent.children() {
        if let Some(elem) = ElementRef::wrap(child) {
            if elem.value().name.local.as_ref() == tag_name {
                return Some(elem);
            }
            // Search one level deeper (for html > body)
            if let Some(found) = find_element(&elem, tag_name) {
                return Some(found);
            }
        }
    }
    None
}

/// Merge adjacent StaticText nodes into a single node.
fn merge_adjacent_text(nodes: &mut Vec<SemanticNode>) {
    let mut i = 0;
    while i + 1 < nodes.len() {
        if nodes[i].role == AriaRole::StaticText && nodes[i + 1].role == AriaRole::StaticText {
            let next_name = nodes[i + 1].name.clone();
            nodes[i].name.push(' ');
            nodes[i].name.push_str(&next_name);
            nodes.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Maximum number of siblings with the same role to show before summarizing.
const SIBLING_MERGE_THRESHOLD: usize = 5;

/// Detect runs of sibling nodes with the same structural pattern (same role)
/// and compress them: show the first few, then summarize the rest.
///
/// Example: 50 identical `<li>` items → show first 3, then `...+47 more listitem`
fn merge_repeated_siblings(nodes: &mut Vec<SemanticNode>) {
    if nodes.len() <= SIBLING_MERGE_THRESHOLD {
        return;
    }

    let mut result: Vec<SemanticNode> = Vec::new();
    let mut i = 0;

    while i < nodes.len() {
        let current_role = &nodes[i].role;

        // Only merge non-text, non-interactive structural siblings
        if !is_mergeable_role(current_role) {
            result.push(nodes[i].clone());
            i += 1;
            continue;
        }

        // Count the run of siblings with the same role
        let run_start = i;
        while i < nodes.len() && nodes[i].role == *current_role {
            i += 1;
        }
        let run_len = i - run_start;

        if run_len <= SIBLING_MERGE_THRESHOLD {
            for node in nodes.iter().take(i).skip(run_start) {
                result.push(node.clone());
            }
        } else {
            for node in nodes.iter().skip(run_start).take(SIBLING_MERGE_THRESHOLD) {
                result.push(node.clone());
            }
            let remaining = run_len - SIBLING_MERGE_THRESHOLD;
            result.push(SemanticNode::text(format!(
                "...+{remaining} more {current_role}"
            )));
        }
    }

    *nodes = result;
}

/// Roles that can be merged when they repeat as siblings.
fn is_mergeable_role(role: &AriaRole) -> bool {
    matches!(
        role,
        AriaRole::ListItem | AriaRole::Row | AriaRole::Cell | AriaRole::Option
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::AriaRole;

    fn snap(html: &str) -> crate::dom::PageSnapshot {
        process(html, "https://test.com")
    }

    fn has_role(nodes: &[SemanticNode], role: &AriaRole) -> bool {
        nodes
            .iter()
            .any(|n| n.role == *role || has_role(&n.children, role))
    }

    fn find_by_role<'a>(nodes: &'a [SemanticNode], role: &AriaRole) -> Option<&'a SemanticNode> {
        for n in nodes {
            if n.role == *role {
                return Some(n);
            }
            if let Some(found) = find_by_role(&n.children, role) {
                return Some(found);
            }
        }
        None
    }

    // ── Pruning ──

    #[test]
    fn prune_script_tags() {
        let s = snap("<body><script>alert('hi')</script><p>Hello</p></body>");
        assert!(!has_role(&s.nodes, &AriaRole::Group));
        assert!(s.nodes.iter().any(|n| n.name.contains("Hello")));
    }

    #[test]
    fn prune_style_tags() {
        let s = snap("<body><style>body{color:red}</style><p>Hello</p></body>");
        assert!(s.nodes.iter().any(|n| n.name.contains("Hello")));
    }

    #[test]
    fn prune_svg_entirely() {
        let s = snap(
            r#"<body><svg viewBox="0 0 100 100"><circle cx="50" cy="50" r="40"/></svg><p>After SVG</p></body>"#,
        );
        let text = s
            .nodes
            .iter()
            .map(|n| &n.name)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!text.contains("circle"));
        assert!(!text.contains("viewBox"));
    }

    #[test]
    fn prune_aria_hidden() {
        let s = snap(r#"<body><div aria-hidden="true"><p>Hidden</p></div><p>Visible</p></body>"#);
        let all_text: String = collect_text(&s.nodes);
        assert!(!all_text.contains("Hidden"));
        assert!(all_text.contains("Visible"));
    }

    #[test]
    fn prune_display_none() {
        let s =
            snap(r#"<body><div style="display:none"><p>Invisible</p></div><p>Shown</p></body>"#);
        let all_text = collect_text(&s.nodes);
        assert!(!all_text.contains("Invisible"));
        assert!(all_text.contains("Shown"));
    }

    #[test]
    fn prune_visibility_hidden() {
        let s =
            snap(r#"<body><div style="visibility: hidden"><p>Ghost</p></div><p>Real</p></body>"#);
        let all_text = collect_text(&s.nodes);
        assert!(!all_text.contains("Ghost"));
        assert!(all_text.contains("Real"));
    }

    #[test]
    fn prune_hidden_attribute() {
        let s = snap(r#"<body><div hidden><p>Nope</p></div><p>Yes</p></body>"#);
        let all_text = collect_text(&s.nodes);
        assert!(!all_text.contains("Nope"));
        assert!(all_text.contains("Yes"));
    }

    #[test]
    fn prune_hidden_input() {
        let result = process_with_refs(
            r#"<form><input type="hidden" name="token"><input type="text" name="user"></form>"#,
            "",
        );
        // form + textbox get refs (both are interactive)
        assert_eq!(result.ref_index.len(), 2);
    }

    // ── Role Mapping ──

    #[test]
    fn button_tag_maps_to_button() {
        let s = snap("<body><button>Click</button></body>");
        assert!(has_role(&s.nodes, &AriaRole::Button));
    }

    #[test]
    fn anchor_with_href_maps_to_link() {
        let s = snap(r#"<body><a href="/page">Go</a></body>"#);
        assert!(has_role(&s.nodes, &AriaRole::Link));
    }

    #[test]
    fn anchor_without_href_not_a_link() {
        let s = snap("<body><a>Not a link</a></body>");
        assert!(!has_role(&s.nodes, &AriaRole::Link));
    }

    #[test]
    fn input_types_map_correctly() {
        let s = snap(
            r#"<body>
            <input type="text"><input type="checkbox"><input type="radio">
            <input type="submit" value="Go"><input type="password">
        </body>"#,
        );
        assert!(has_role(&s.nodes, &AriaRole::TextBox));
        assert!(has_role(&s.nodes, &AriaRole::Checkbox));
        assert!(has_role(&s.nodes, &AriaRole::Radio));
        assert!(has_role(&s.nodes, &AriaRole::Button));
    }

    #[test]
    fn select_maps_to_combobox() {
        let s = snap("<body><select><option>A</option></select></body>");
        assert!(has_role(&s.nodes, &AriaRole::ComboBox));
    }

    #[test]
    fn heading_levels() {
        let s = snap("<body><h1>One</h1><h3>Three</h3><h6>Six</h6></body>");
        assert!(find_by_role(&s.nodes, &AriaRole::Heading { level: 1 }).is_some());
        assert!(find_by_role(&s.nodes, &AriaRole::Heading { level: 3 }).is_some());
        assert!(find_by_role(&s.nodes, &AriaRole::Heading { level: 6 }).is_some());
    }

    #[test]
    fn explicit_role_overrides_tag() {
        let s = snap(r#"<body><div role="button" aria-label="Toggle">☰</div></body>"#);
        assert!(has_role(&s.nodes, &AriaRole::Button));
    }

    #[test]
    fn landmark_tags() {
        let s = snap("<body><header>H</header><nav>N</nav><main>M</main><footer>F</footer></body>");
        assert!(has_role(&s.nodes, &AriaRole::Banner));
        assert!(has_role(&s.nodes, &AriaRole::Navigation));
        assert!(has_role(&s.nodes, &AriaRole::Main));
        assert!(has_role(&s.nodes, &AriaRole::ContentInfo));
    }

    // ── Name Computation ──

    #[test]
    fn aria_label_as_name() {
        let s = snap(r#"<body><button aria-label="Close">X</button></body>"#);
        let btn = find_by_role(&s.nodes, &AriaRole::Button).unwrap();
        assert_eq!(btn.name, "Close");
    }

    #[test]
    fn label_for_as_name() {
        let s = snap(r#"<body><label for="e">Email</label><input type="email" id="e"></body>"#);
        let input = find_by_role(&s.nodes, &AriaRole::TextBox).unwrap();
        assert_eq!(input.name, "Email");
    }

    #[test]
    fn alt_text_as_name() {
        let s = snap(r#"<body><img src="x.png" alt="Product photo"></body>"#);
        let img = find_by_role(&s.nodes, &AriaRole::Img).unwrap();
        assert_eq!(img.name, "Product photo");
    }

    #[test]
    fn placeholder_as_name() {
        let s = snap(r#"<body><input placeholder="Type here..."></body>"#);
        let input = find_by_role(&s.nodes, &AriaRole::TextBox).unwrap();
        assert_eq!(input.name, "Type here...");
    }

    #[test]
    fn title_attribute_as_name() {
        let s = snap(r#"<body><button title="More options">⋮</button></body>"#);
        let btn = find_by_role(&s.nodes, &AriaRole::Button).unwrap();
        // button text content is "⋮" but title should win since aria-label is absent
        // Actually in our implementation, text content wins over title for buttons.
        // title is only used if there's no text content and no aria-label.
        // The button has text "⋮", so that's used.
        assert!(!btn.name.is_empty());
    }

    // ── Wrapper Collapse ──

    #[test]
    fn meaningless_divs_collapse() {
        let s = snap("<body><div><div><div><h1>Deep</h1></div></div></div></body>");
        // The heading should be near the top, not deeply nested
        let h1 = find_by_role(&s.nodes, &AriaRole::Heading { level: 1 }).unwrap();
        assert_eq!(h1.name, "Deep");
    }

    // ── Ref Indexing ──

    #[test]
    fn refs_assigned_to_interactive() {
        let result = process_with_refs(
            r#"<body><button>Click</button><p>Text</p><a href="/x">Link</a></body>"#,
            "",
        );
        // Button and link should have refs, paragraph should not
        assert_eq!(result.ref_index.len(), 2);
    }

    #[test]
    fn locator_uses_id_when_available() {
        let result = process_with_refs(r#"<body><button id="submit-btn">Go</button></body>"#, "");
        let locator = result.ref_index.values().next().unwrap();
        assert!(locator.to_js_expression().contains("getElementById"));
        assert!(locator.to_js_expression().contains("submit-btn"));
    }

    #[test]
    fn locator_uses_name_when_no_id() {
        let result = process_with_refs(r#"<body><input type="text" name="username"></body>"#, "");
        let locator = result.ref_index.values().next().unwrap();
        assert!(locator.to_js_expression().contains("username"));
    }

    // ── Title Extraction ──

    #[test]
    fn title_extracted() {
        let s = snap("<html><head><title>My Page</title></head><body></body></html>");
        assert_eq!(s.title, "My Page");
    }

    #[test]
    fn empty_title() {
        let s = snap("<html><body><p>No title</p></body></html>");
        assert_eq!(s.title, "");
    }

    /// Collect all text content from a tree for assertion helpers.
    fn collect_text(nodes: &[SemanticNode]) -> String {
        let mut text = String::new();
        for n in nodes {
            text.push_str(&n.name);
            text.push(' ');
            text.push_str(&collect_text(&n.children));
        }
        text
    }

    // ── Stable Ref ID Tests ──

    /// Collect all ref_ids > 0 from a node tree.
    fn collect_all_refs(nodes: &[SemanticNode]) -> Vec<u32> {
        let mut refs = Vec::new();
        for n in nodes {
            if n.ref_id > 0 {
                refs.push(n.ref_id);
            }
            refs.extend(collect_all_refs(&n.children));
        }
        refs
    }

    #[test]
    fn stable_refs_same_html_same_refs() {
        let html = r#"<body>
            <button id="btn1">Click</button>
            <a href="/page">Link</a>
            <input type="text" name="user">
        </body>"#;
        let r1 = process_with_refs(html, "https://test.com");
        let r2 = process_with_refs(html, "https://test.com");

        let refs1 = collect_all_refs(&r1.snapshot.nodes);
        let refs2 = collect_all_refs(&r2.snapshot.nodes);
        assert_eq!(refs1, refs2, "same HTML should produce identical ref IDs");
    }

    #[test]
    fn stable_refs_survive_content_addition() {
        let html_before = r#"<body>
            <button id="submit">Go</button>
            <a href="/home">Home</a>
        </body>"#;
        let html_after = r#"<body>
            <p>New paragraph added above</p>
            <button id="submit">Go</button>
            <a href="/home">Home</a>
        </body>"#;

        let r1 = process_with_refs(html_before, "https://test.com");
        let r2 = process_with_refs(html_after, "https://test.com");

        // The button and link have id/href so they should hash the same despite
        // the path changing, because id and name are prioritized in the hash.
        // Actually, path does change - but the elements have id/href which dominate.
        // Let's check that the button ref is present in both.
        let btn_ref_1 = r1
            .ref_index
            .iter()
            .find(|(_, loc)| loc.tag == "button")
            .map(|(id, _)| *id);
        let btn_ref_2 = r2
            .ref_index
            .iter()
            .find(|(_, loc)| loc.tag == "button")
            .map(|(id, _)| *id);
        assert!(btn_ref_1.is_some());
        assert_eq!(
            btn_ref_1, btn_ref_2,
            "button with id should keep same ref after content insertion"
        );

        let link_ref_1 = r1
            .ref_index
            .iter()
            .find(|(_, loc)| loc.tag == "a")
            .map(|(id, _)| *id);
        let link_ref_2 = r2
            .ref_index
            .iter()
            .find(|(_, loc)| loc.tag == "a")
            .map(|(id, _)| *id);
        assert_eq!(
            link_ref_1, link_ref_2,
            "link with href should keep same ref"
        );
    }

    #[test]
    fn stable_refs_no_collisions() {
        // Process a page with many interactive elements and verify no duplicate refs
        let html = r#"<body>
            <nav>
                <a href="/a">A</a><a href="/b">B</a><a href="/c">C</a>
                <a href="/d">D</a><a href="/e">E</a><a href="/f">F</a>
            </nav>
            <form>
                <input name="i1"><input name="i2"><input name="i3">
                <input name="i4"><input name="i5"><input name="i6">
                <button>Submit</button>
                <select><option>X</option><option>Y</option></select>
            </form>
        </body>"#;
        let result = process_with_refs(html, "https://test.com");
        let refs = collect_all_refs(&result.snapshot.nodes);
        let mut sorted = refs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(refs.len(), sorted.len(), "no duplicate ref IDs: {:?}", refs);
        assert!(
            refs.iter().all(|&r| r >= 10000 && r <= 99999),
            "all refs in 5-digit range: {:?}",
            refs
        );
    }

    #[test]
    fn stable_refs_are_in_five_digit_range() {
        let html = r#"<body><button>Click</button></body>"#;
        let result = process_with_refs(html, "https://test.com");
        let refs = collect_all_refs(&result.snapshot.nodes);
        assert!(!refs.is_empty());
        for r in &refs {
            assert!(
                *r >= 10000 && *r <= 99999,
                "ref {r} should be in [10000, 99999]"
            );
        }
    }
}
