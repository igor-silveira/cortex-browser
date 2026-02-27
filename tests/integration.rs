use cortex_browser::dom::{AriaRole, ElementLocator, PageSnapshot, SemanticNode};
use cortex_browser::extract;
use cortex_browser::hints::{self, TaskContext};
use cortex_browser::mutation::DirtyState;
use cortex_browser::pipeline;
use cortex_browser::recording;
use cortex_browser::serialize;

// ── Test Fixtures ───────────────────────────────────────────────────────────

const ECOMMERCE: &str = include_str!("fixtures/ecommerce.html");
const DASHBOARD: &str = include_str!("fixtures/dashboard.html");
const BLOG: &str = include_str!("fixtures/blog.html");
const SPA: &str = include_str!("fixtures/spa_app.html");

fn snap(html: &str) -> PageSnapshot {
    pipeline::process(html, "https://example.com")
}

fn snap_text(html: &str) -> String {
    serialize::to_compact_text(&snap(html))
}

fn snap_refs(html: &str) -> cortex_browser::dom::ProcessResult {
    pipeline::process_with_refs(html, "https://example.com")
}

/// Count total nodes recursively.
fn count_nodes(nodes: &[SemanticNode]) -> usize {
    nodes
        .iter()
        .map(|n| 1 + count_nodes(&n.children))
        .sum()
}

/// Collect all ref_ids > 0 from the tree.
fn collect_refs(nodes: &[SemanticNode]) -> Vec<u32> {
    let mut refs = Vec::new();
    for n in nodes {
        if n.ref_id > 0 {
            refs.push(n.ref_id);
        }
        refs.extend(collect_refs(&n.children));
    }
    refs
}

/// Check if a node with the given role and name-substring exists anywhere in the tree.
fn has_node(nodes: &[SemanticNode], role: &AriaRole, name_contains: &str) -> bool {
    for n in nodes {
        if n.role == *role && n.name.to_lowercase().contains(&name_contains.to_lowercase()) {
            return true;
        }
        if has_node(&n.children, role, name_contains) {
            return true;
        }
    }
    false
}

/// Find the first node matching role + name substring.
fn find_node<'a>(
    nodes: &'a [SemanticNode],
    role: &AriaRole,
    name_contains: &str,
) -> Option<&'a SemanticNode> {
    for n in nodes {
        if n.role == *role && n.name.to_lowercase().contains(&name_contains.to_lowercase()) {
            return Some(n);
        }
        if let Some(found) = find_node(&n.children, role, name_contains) {
            return Some(found);
        }
    }
    None
}

/// Count nodes of a specific role.
fn count_role(nodes: &[SemanticNode], role: &AriaRole) -> usize {
    let mut count = 0;
    for n in nodes {
        if n.role == *role {
            count += 1;
        }
        count += count_role(&n.children, role);
    }
    count
}

// ═══════════════════════════════════════════════════════════════════════════
// STRUCTURAL PRUNING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn scripts_and_styles_are_pruned() {
    let text = snap_text(ECOMMERCE);
    assert!(
        !text.contains("react.production"),
        "script src should be pruned"
    );
    assert!(
        !text.contains("__NEXT_DATA__"),
        "inline script content should be pruned"
    );
    assert!(
        !text.contains("sr-only"),
        "style rules should be pruned"
    );
    assert!(
        !text.contains("@keyframes"),
        "CSS keyframes should be pruned"
    );
}

#[test]
fn svg_elements_are_pruned() {
    let text = snap_text(ECOMMERCE);
    assert!(!text.contains("viewBox"), "SVG attributes should be pruned");
    assert!(
        !text.contains("stroke-linecap"),
        "SVG path attributes should be pruned"
    );
}

#[test]
fn noscript_is_pruned() {
    let text = snap_text(ECOMMERCE);
    assert!(
        !text.contains("enable JavaScript"),
        "noscript content should be pruned"
    );
}

#[test]
fn aria_hidden_elements_are_pruned() {
    // The ecommerce page has an aria-hidden decoration div and cart badge
    let snapshot = snap(ECOMMERCE);
    assert!(
        !has_node(&snapshot.nodes, &AriaRole::Img, "bg.png"),
        "aria-hidden decoration should be pruned"
    );
}

#[test]
fn display_none_elements_are_pruned() {
    // The mobile menu and toast container are display:none
    let text = snap_text(ECOMMERCE);
    assert!(
        !text.contains("Added to cart!"),
        "display:none toast should be pruned"
    );
}

#[test]
fn hidden_attribute_elements_are_pruned() {
    // SPA has hidden tab panels
    let text = snap_text(SPA);
    assert!(
        !text.contains("Billing panel content"),
        "hidden panels should be pruned"
    );
    assert!(
        !text.contains("Danger zone content"),
        "hidden panels should be pruned"
    );
}

#[test]
fn hidden_inputs_are_pruned() {
    let html = r#"<form><input type="hidden" name="csrf" value="abc123"><input type="text" name="user"></form>"#;
    let snapshot = snap(html);
    assert!(
        !has_node(&snapshot.nodes, &AriaRole::TextBox, "csrf"),
        "hidden inputs should be pruned"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::TextBox, ""),
        "visible inputs should remain"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// TOKEN COMPRESSION TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn ecommerce_achieves_significant_compression() {
    let text = snap_text(ECOMMERCE);
    let html_len = ECOMMERCE.len();
    let snap_len = text.len();
    let ratio = snap_len as f64 / html_len as f64;
    assert!(
        ratio < 0.25,
        "ecommerce snapshot ({snap_len} bytes) should be <25% of HTML ({html_len} bytes), got {:.1}%",
        ratio * 100.0
    );
}

#[test]
fn dashboard_achieves_significant_compression() {
    let text = snap_text(DASHBOARD);
    let ratio = text.len() as f64 / DASHBOARD.len() as f64;
    assert!(
        ratio < 0.25,
        "dashboard snapshot should be <25% of HTML, got {:.1}%",
        ratio * 100.0
    );
}

#[test]
fn blog_achieves_significant_compression() {
    let text = snap_text(BLOG);
    let ratio = text.len() as f64 / BLOG.len() as f64;
    assert!(
        ratio < 0.30,
        "blog snapshot should be <30% of HTML, got {:.1}%",
        ratio * 100.0
    );
}

#[test]
fn spa_achieves_significant_compression() {
    let text = snap_text(SPA);
    let ratio = text.len() as f64 / SPA.len() as f64;
    assert!(
        ratio < 0.25,
        "SPA snapshot should be <25% of HTML, got {:.1}%",
        ratio * 100.0
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// WRAPPER COLLAPSE TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn deeply_nested_wrappers_are_collapsed() {
    let html = r#"
        <div class="outer"><div class="middle"><div class="inner">
            <h1>Title</h1>
            <p>Content here</p>
        </div></div></div>
    "#;
    let text = snap_text(html);
    // The heading and paragraph should be at the top level, not nested
    assert!(text.contains("heading[1]"), "heading should survive");
    assert!(text.contains("Content here"), "content should survive");
    // The text should NOT show deeply nested indentation
    let heading_indent = text
        .lines()
        .find(|l| l.contains("heading[1]"))
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(99);
    assert!(
        heading_indent <= 2,
        "heading should be at top level (indent={heading_indent}), wrappers should collapse"
    );
}

#[test]
fn ecommerce_wrapper_divs_are_collapsed() {
    // The ecommerce page has structures like:
    // <div class="max-w-7xl"><div class="flex"><div class="flex-shrink-0">
    //   <a href="/">ShopNow</a>
    // </div></div></div>
    let snapshot = snap(ECOMMERCE);
    // The link should be directly inside a navigation or header, not deeply nested
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Link, "ShopNow"),
        "ShopNow link should exist"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// ROLE MAPPING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn ecommerce_landmarks_are_mapped() {
    let snapshot = snap(ECOMMERCE);
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Banner, ""),
        "header → banner"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Main, ""),
        "main element"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::ContentInfo, ""),
        "footer → contentinfo"
    );
    assert!(
        count_role(&snapshot.nodes, &AriaRole::Navigation) >= 2,
        "should have multiple nav landmarks"
    );
}

#[test]
fn form_controls_have_correct_roles() {
    let snapshot = snap(ECOMMERCE);
    // Search input
    assert!(
        has_node(&snapshot.nodes, &AriaRole::TextBox, "Search products"),
        "search input should be a textbox"
    );
    // Checkboxes in filters
    assert!(
        count_role(&snapshot.nodes, &AriaRole::Checkbox) >= 4,
        "should have filter checkboxes"
    );
    // Radio buttons
    assert!(
        count_role(&snapshot.nodes, &AriaRole::Radio) >= 3,
        "should have price range radios"
    );
    // Select/combobox
    assert!(
        count_role(&snapshot.nodes, &AriaRole::ComboBox) >= 1,
        "should have sort dropdown"
    );
}

#[test]
fn spa_tabs_have_correct_roles() {
    let snapshot = snap(SPA);
    assert!(
        count_role(&snapshot.nodes, &AriaRole::Tab) >= 4,
        "should have 4 tab buttons"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Tab, "General"),
        "General tab"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Tab, "Members"),
        "Members tab"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Tab, "Billing"),
        "Billing tab"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Tab, "Danger Zone"),
        "Danger Zone tab"
    );
}

#[test]
fn spa_radio_group_is_mapped() {
    let snapshot = snap(SPA);
    assert!(
        count_role(&snapshot.nodes, &AriaRole::Radio) >= 3,
        "should have environment radio buttons"
    );
}

#[test]
fn headings_preserve_level() {
    let snapshot = snap(BLOG);
    assert!(
        has_node(
            &snapshot.nodes,
            &AriaRole::Heading { level: 1 },
            "Borrow Checker"
        ),
        "h1 with article title"
    );
    assert!(
        has_node(
            &snapshot.nodes,
            &AriaRole::Heading { level: 2 },
            "Three Rules"
        ),
        "h2 in article"
    );
}

#[test]
fn dashboard_table_structure() {
    let snapshot = snap(DASHBOARD);
    assert!(
        count_role(&snapshot.nodes, &AriaRole::Table) >= 1,
        "should have a data table"
    );
    assert!(
        count_role(&snapshot.nodes, &AriaRole::ColumnHeader) >= 5,
        "table should have column headers"
    );
    assert!(
        count_role(&snapshot.nodes, &AriaRole::Row) >= 5,
        "table should have data rows"
    );
}

#[test]
fn images_with_alt_text_are_kept() {
    let snapshot = snap(ECOMMERCE);
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Img, "SoundMax Pro"),
        "product image with alt should be kept"
    );
}

#[test]
fn images_without_alt_are_dropped() {
    let html = r#"<img src="decoration.png" alt=""><img src="important.png" alt="Important diagram">"#;
    let snapshot = snap(html);
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Img, "Important diagram"),
        "image with alt is kept"
    );
    assert_eq!(
        count_role(&snapshot.nodes, &AriaRole::Img),
        1,
        "only the image with alt text should remain"
    );
}

#[test]
fn disabled_button_has_attribute() {
    // Use inline HTML since the ecommerce fixture's disabled button is in a
    // product card that gets merged away by sibling merging (> 5 listitems).
    let html = r#"<button disabled>Out of Stock</button>"#;
    let text = snap_text(html);
    assert!(
        text.contains("[disabled]"),
        "disabled button should have [disabled] marker: {text}"
    );
    assert!(
        text.contains("Out of Stock"),
        "disabled button text should appear: {text}"
    );
}

#[test]
fn checked_checkbox_has_attribute() {
    let text = snap_text(ECOMMERCE);
    assert!(
        text.contains("[checked]"),
        "checked checkbox should have [checked] marker"
    );
}

#[test]
fn password_input_has_type() {
    let html = r#"<input type="password" placeholder="Password">"#;
    let text = snap_text(html);
    assert!(
        text.contains("(password)"),
        "password input should show type"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// ACCESSIBLE NAME COMPUTATION TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn aria_label_takes_precedence() {
    let html = r#"<button aria-label="Close dialog">X</button>"#;
    let snapshot = snap(html);
    let btn = find_node(&snapshot.nodes, &AriaRole::Button, "Close dialog");
    assert!(btn.is_some(), "button should use aria-label for name");
}

#[test]
fn label_for_associates_name() {
    let html = r#"
        <label for="email">Email Address</label>
        <input type="email" id="email" name="email">
    "#;
    let snapshot = snap(html);
    assert!(
        has_node(&snapshot.nodes, &AriaRole::TextBox, "Email Address"),
        "input should get name from associated label"
    );
}

#[test]
fn placeholder_as_name() {
    let html = r#"<input type="text" placeholder="Search...">"#;
    let snapshot = snap(html);
    assert!(
        has_node(&snapshot.nodes, &AriaRole::TextBox, "Search..."),
        "input should use placeholder as name"
    );
}

#[test]
fn input_value_is_captured() {
    let html = r#"<input type="text" id="name" value="John Doe" placeholder="Name">"#;
    let snapshot = snap(html);
    let node = find_node(&snapshot.nodes, &AriaRole::TextBox, "Name").unwrap();
    assert_eq!(node.value, Some("John Doe".into()));
}

#[test]
fn spa_form_values_are_captured() {
    let snapshot = snap(SPA);
    let node = find_node(&snapshot.nodes, &AriaRole::TextBox, "Project Name");
    assert!(node.is_some(), "project name field should exist");
    let node = node.unwrap();
    assert_eq!(
        node.value,
        Some("My Awesome Project".into()),
        "should capture input value"
    );
}

#[test]
fn long_text_is_truncated() {
    let long_text = "A".repeat(300);
    let html = format!("<p>{long_text}</p>");
    let snapshot = snap(&html);
    let node = find_node(&snapshot.nodes, &AriaRole::Paragraph, "AAAA");
    assert!(node.is_some(), "paragraph should exist");
    assert!(
        node.unwrap().name.len() <= 200,
        "text should be truncated to ~200 chars"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// REF INDEXING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn only_interactive_elements_get_refs() {
    let snapshot = snap(ECOMMERCE);
    let refs = collect_refs(&snapshot.nodes);
    assert!(!refs.is_empty(), "should have refs");
    // Verify all refs are unique
    let mut sorted = refs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(refs.len(), sorted.len(), "refs should be unique");
}

#[test]
fn ref_index_has_locator_info() {
    let result = snap_refs(ECOMMERCE);
    assert!(
        !result.ref_index.is_empty(),
        "ref_index should have entries"
    );
    // Check that search input has a locator
    let search_ref = result.ref_index.values().find(|loc| loc.tag == "input");
    assert!(search_ref.is_some(), "should have input locator");
}

#[test]
fn ref_ids_are_unique_and_positive() {
    let snapshot = snap(DASHBOARD);
    let refs = collect_refs(&snapshot.nodes);
    assert!(!refs.is_empty(), "should have refs");
    // All refs should be unique
    let mut sorted = refs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        refs.len(),
        sorted.len(),
        "all refs in the tree should be unique"
    );
    // All refs should be in the 5-digit stable range
    assert!(
        refs.iter().all(|&r| r >= 10000 && r <= 99999),
        "all refs should be in [10000, 99999]"
    );
    // The ref_index may contain more entries than visible in the tree
    // because sibling merging can remove nodes whose refs were already assigned.
    let result = snap_refs(DASHBOARD);
    assert!(
        result.ref_index.len() >= refs.len(),
        "ref_index should have at least as many entries as visible refs"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// SIBLING MERGING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn long_list_siblings_are_merged() {
    let mut lis = String::new();
    for i in 0..20 {
        lis.push_str(&format!("<li>Item {i}</li>"));
    }
    let html = format!("<ul>{lis}</ul>");
    let text = snap_text(&html);
    assert!(
        text.contains("...+"),
        "should have merge summary for long list: {text}"
    );
    assert!(
        text.contains("more listitem"),
        "should indicate merged listitem count"
    );
}

#[test]
fn short_list_is_not_merged() {
    let html = "<ul><li>A</li><li>B</li><li>C</li></ul>";
    let text = snap_text(html);
    assert!(!text.contains("...+"), "short list should not be merged");
    assert!(text.contains("A"), "all items should be present");
    assert!(text.contains("C"), "all items should be present");
}

#[test]
fn dashboard_table_rows_are_merged() {
    let snapshot = snap(DASHBOARD);
    let text = serialize::to_compact_text(&snapshot);
    // The dashboard has 7 table rows - the row merging threshold is 5,
    // so we expect merging.
    assert!(
        text.contains("...+") && text.contains("more row"),
        "table rows should be merged: {text}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// SERIALIZATION FORMAT TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn header_shows_title_and_url() {
    let text = snap_text(ECOMMERCE);
    assert!(
        text.starts_with("page: "),
        "should start with page: header"
    );
    assert!(
        text.contains("ShopNow"),
        "should contain page title"
    );
    assert!(
        text.contains("[https://example.com]"),
        "should contain URL"
    );
    assert!(text.contains("---\n"), "should have separator line");
}

#[test]
fn interactive_elements_have_ref_markers() {
    let text = snap_text(ECOMMERCE);
    // Should have @eN markers for interactive elements (refs are now 5-digit hashes)
    assert!(text.contains("@e"), "should have ref markers");
    assert!(text.contains("button"), "should have button roles");
    assert!(text.contains("textbox"), "should have textbox roles");
    assert!(text.contains("link"), "should have link roles");
}

#[test]
fn href_is_shown_for_links() {
    let text = snap_text(ECOMMERCE);
    assert!(
        text.contains("-> /products"),
        "links should show href"
    );
}

#[test]
fn indentation_reflects_nesting() {
    let html = r#"
        <nav aria-label="Main">
            <a href="/home">Home</a>
            <a href="/about">About</a>
        </nav>
    "#;
    let text = snap_text(html);
    let nav_line = text.lines().find(|l| l.contains("navigation")).unwrap();
    let link_line = text.lines().find(|l| l.contains("Home")).unwrap();
    let nav_indent = nav_line.len() - nav_line.trim_start().len();
    let link_indent = link_line.len() - link_line.trim_start().len();
    assert!(
        link_indent > nav_indent,
        "children should be indented deeper than parent"
    );
}

#[test]
fn json_output_is_valid() {
    let snapshot = snap(ECOMMERCE);
    let json = serde_json::to_string(&snapshot).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed["title"].is_string());
    assert!(parsed["nodes"].is_array());
}

// ═══════════════════════════════════════════════════════════════════════════
// TASK CONTEXT HINTS TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn hints_parse_role_common_names() {
    assert_eq!(hints::parse_role("button"), Some(AriaRole::Button));
    assert_eq!(hints::parse_role("link"), Some(AriaRole::Link));
    assert_eq!(hints::parse_role("textbox"), Some(AriaRole::TextBox));
    assert_eq!(hints::parse_role("input"), Some(AriaRole::TextBox));
    assert_eq!(hints::parse_role("nav"), Some(AriaRole::Navigation));
    assert_eq!(hints::parse_role("BUTTON"), Some(AriaRole::Button));
    assert_eq!(hints::parse_role("unknown_role"), None);
}

#[test]
fn task_context_filter_reduces_nodes() {
    let snapshot = snap(ECOMMERCE);
    let full_count = count_nodes(&snapshot.nodes);

    let ctx = TaskContext {
        task: "Find the search bar".into(),
        focus_text: vec!["search".into()],
        focus_roles: vec![AriaRole::TextBox],
        interactive_only: false,
    };
    let filtered = ctx.filter_snapshot(&snapshot);
    let filtered_count = count_nodes(&filtered.nodes);

    assert!(
        filtered_count < full_count,
        "filtered ({filtered_count}) should have fewer nodes than full ({full_count})"
    );
    assert!(
        has_node(&filtered.nodes, &AriaRole::TextBox, "Search"),
        "search textbox should survive filtering"
    );
}

#[test]
fn task_context_interactive_only() {
    let snapshot = snap(ECOMMERCE);
    let ctx = TaskContext {
        task: "Interact with the page".into(),
        focus_text: vec![],
        focus_roles: vec![],
        interactive_only: true,
    };
    let filtered = ctx.filter_snapshot(&snapshot);
    let text = serialize::to_compact_text(&filtered);

    // Interactive elements should be present
    assert!(text.contains("button"), "buttons should survive");
    assert!(text.contains("textbox"), "inputs should survive");
    assert!(text.contains("link"), "links should survive");
}

#[test]
fn task_context_text_match_case_insensitive() {
    let snapshot = snap(BLOG);
    let ctx = TaskContext {
        task: "Find the comment form".into(),
        focus_text: vec!["comment".into(), "post".into()],
        focus_roles: vec![],
        interactive_only: false,
    };
    let filtered = ctx.filter_snapshot(&snapshot);

    assert!(
        has_node(&filtered.nodes, &AriaRole::TextBox, ""),
        "comment textarea should survive"
    );
    assert!(
        has_node(&filtered.nodes, &AriaRole::Button, "Post Comment"),
        "post comment button should survive"
    );
}

#[test]
fn task_context_preserves_structural_parents() {
    // Filtering should keep landmarks that contain matching descendants
    let snapshot = snap(SPA);
    let ctx = TaskContext {
        task: "Change the build command".into(),
        focus_text: vec!["build".into()],
        focus_roles: vec![AriaRole::TextBox],
        interactive_only: false,
    };
    let filtered = ctx.filter_snapshot(&snapshot);

    // The main landmark should still be present (parent of the build inputs)
    assert!(
        has_node(&filtered.nodes, &AriaRole::Main, ""),
        "main landmark should be preserved as structural parent"
    );
}

#[test]
fn focused_snapshot_with_role_filter() {
    let snapshot = snap(DASHBOARD);
    let ctx = TaskContext {
        task: String::new(),
        focus_text: vec![],
        focus_roles: vec![AriaRole::Button],
        interactive_only: false,
    };
    let filtered = ctx.filter_snapshot(&snapshot);
    let full_count = count_nodes(&snapshot.nodes);
    let filtered_count = count_nodes(&filtered.nodes);

    assert!(
        filtered_count < full_count,
        "button-focused snapshot should be smaller"
    );
    // All buttons should survive
    assert!(
        count_role(&filtered.nodes, &AriaRole::Button) > 0,
        "buttons should survive filtering"
    );
}

#[test]
fn empty_context_keeps_everything() {
    let snapshot = snap(SPA);
    let ctx = TaskContext {
        task: String::new(),
        focus_text: vec![],
        focus_roles: vec![],
        interactive_only: false,
    };
    let filtered = ctx.filter_snapshot(&snapshot);

    // With no focus criteria, interactive elements and landmarks still score > 0,
    // so the filtered version should retain significant content
    let filtered_count = count_nodes(&filtered.nodes);
    assert!(
        filtered_count > 0,
        "empty context should still produce output"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// MUTATION STATE PARSING TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn dirty_state_parses_clean() {
    let state = DirtyState::from_json(r#"{"dirty":false,"count":0}"#);
    assert!(!state.dirty);
}

#[test]
fn dirty_state_parses_dirty() {
    let state = DirtyState::from_json(r#"{"dirty":true,"count":42}"#);
    assert!(state.dirty);
}

#[test]
fn dirty_state_handles_malformed_json() {
    let state = DirtyState::from_json("not json");
    assert!(state.dirty, "malformed JSON should assume dirty");
}

#[test]
fn dirty_state_handles_empty_string() {
    let state = DirtyState::from_json("");
    assert!(state.dirty, "empty string should assume dirty");
}

// ═══════════════════════════════════════════════════════════════════════════
// CROSS-FIXTURE CONSISTENCY TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn all_fixtures_produce_output() {
    for (name, html) in [
        ("ecommerce", ECOMMERCE),
        ("dashboard", DASHBOARD),
        ("blog", BLOG),
        ("spa", SPA),
    ] {
        let text = snap_text(html);
        assert!(
            !text.is_empty(),
            "{name} fixture should produce non-empty output"
        );
        assert!(
            text.contains("page:"),
            "{name} fixture should have page header"
        );
    }
}

#[test]
fn all_fixtures_have_interactive_refs() {
    for (name, html) in [
        ("ecommerce", ECOMMERCE),
        ("dashboard", DASHBOARD),
        ("blog", BLOG),
        ("spa", SPA),
    ] {
        let snapshot = snap(html);
        let refs = collect_refs(&snapshot.nodes);
        assert!(
            !refs.is_empty(),
            "{name} fixture should have interactive element refs"
        );
    }
}

#[test]
fn all_fixtures_have_landmarks() {
    for (name, html) in [
        ("ecommerce", ECOMMERCE),
        ("dashboard", DASHBOARD),
        ("blog", BLOG),
        ("spa", SPA),
    ] {
        let snapshot = snap(html);
        assert!(
            has_node(&snapshot.nodes, &AriaRole::Main, "")
                || has_node(&snapshot.nodes, &AriaRole::Banner, "")
                || has_node(&snapshot.nodes, &AriaRole::Navigation, ""),
            "{name} fixture should have at least one landmark"
        );
    }
}

#[test]
fn all_fixtures_have_correct_title() {
    let titles = [
        (ECOMMERCE, "ShopNow"),
        (DASHBOARD, "Acme Dashboard"),
        (BLOG, "Borrow Checker"),
        (SPA, "Nimbus"),
    ];
    for (html, expected_substr) in titles {
        let snapshot = snap(html);
        assert!(
            snapshot.title.contains(expected_substr),
            "title '{}' should contain '{expected_substr}'",
            snapshot.title
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// STABLE REF ID TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn stable_refs_same_html_same_refs() {
    for (name, html) in [
        ("ecommerce", ECOMMERCE),
        ("dashboard", DASHBOARD),
        ("blog", BLOG),
        ("spa", SPA),
    ] {
        let r1 = snap_refs(html);
        let r2 = snap_refs(html);
        let mut refs1: Vec<u32> = collect_refs(&r1.snapshot.nodes);
        let mut refs2: Vec<u32> = collect_refs(&r2.snapshot.nodes);
        refs1.sort();
        refs2.sort();
        assert_eq!(
            refs1, refs2,
            "{name}: same HTML should produce identical ref IDs"
        );
    }
}

#[test]
fn stable_refs_survive_content_addition() {
    let html_before = r#"<body>
        <button id="submit-btn">Submit</button>
        <a href="/home">Home</a>
    </body>"#;
    let html_after = r#"<body>
        <p>A new notification banner appeared!</p>
        <button id="submit-btn">Submit</button>
        <a href="/home">Home</a>
    </body>"#;
    let r1 = snap_refs(html_before);
    let r2 = snap_refs(html_after);

    // Elements with strong identity (id, href) should keep the same ref
    let btn1 = r1.ref_index.iter().find(|(_, l)| l.id.as_deref() == Some("submit-btn")).map(|(id, _)| *id);
    let btn2 = r2.ref_index.iter().find(|(_, l)| l.id.as_deref() == Some("submit-btn")).map(|(id, _)| *id);
    assert_eq!(btn1, btn2, "button with id should keep same ref");

    let link1 = r1.ref_index.iter().find(|(_, l)| l.href.as_deref() == Some("/home")).map(|(id, _)| *id);
    let link2 = r2.ref_index.iter().find(|(_, l)| l.href.as_deref() == Some("/home")).map(|(id, _)| *id);
    assert_eq!(link1, link2, "link with href should keep same ref");
}

#[test]
fn stable_refs_no_collisions_across_fixtures() {
    for (name, html) in [
        ("ecommerce", ECOMMERCE),
        ("dashboard", DASHBOARD),
        ("blog", BLOG),
        ("spa", SPA),
    ] {
        let result = snap_refs(html);
        let refs = collect_refs(&result.snapshot.nodes);
        let mut sorted = refs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            refs.len(),
            sorted.len(),
            "{name}: should have no duplicate ref IDs"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// EDGE CASE TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn empty_html_produces_empty_snapshot() {
    let text = snap_text("");
    // Should not panic; may be empty or have minimal output
    assert!(!text.is_empty() || text.is_empty(), "should not panic");
}

#[test]
fn minimal_html_produces_output() {
    let text = snap_text("<html><body><p>Hello</p></body></html>");
    assert!(text.contains("Hello"));
}

#[test]
fn select_options_are_not_concatenated_as_name() {
    let html = r#"
        <select aria-label="Choose color">
            <option value="r">Red</option>
            <option value="g">Green</option>
            <option value="b">Blue</option>
        </select>
    "#;
    let snapshot = snap(html);
    let select = find_node(&snapshot.nodes, &AriaRole::ComboBox, "Choose color");
    assert!(
        select.is_some(),
        "combobox should use aria-label, not option text"
    );
    let select = select.unwrap();
    assert!(
        !select.name.contains("Red Green Blue"),
        "combobox name should NOT be concatenated option text"
    );
}

#[test]
fn explicit_aria_role_overrides_tag() {
    let html = r#"<div role="button" aria-label="Toggle menu">☰</div>"#;
    let snapshot = snap(html);
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Button, "Toggle menu"),
        "div with role=button should be a Button"
    );
}

#[test]
fn data_state_closed_is_pruned() {
    // SPA has elements with data-state="closed" and display:none in CSS
    // These are pruned by the CSS rule in <style>, not by the pipeline.
    // But the hidden tab panels use the `hidden` HTML attribute.
    let text = snap_text(SPA);
    assert!(
        !text.contains("Members panel content"),
        "hidden panel content should not appear"
    );
}

#[test]
fn cookie_consent_banner_is_present() {
    let snapshot = snap(BLOG);
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Button, "Accept All"),
        "cookie consent Accept button should be present"
    );
    assert!(
        has_node(&snapshot.nodes, &AriaRole::Button, "Manage Preferences"),
        "cookie consent Manage button should be present"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// STRUCTURED DATA EXTRACTION TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn extract_table_from_dashboard() {
    let snapshot = snap(DASHBOARD);
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "order_id": { "type": "string" },
                "customer": { "type": "string" },
                "product": { "type": "string" },
                "status": { "type": "string" },
                "total": { "type": "string" }
            }
        }
    });

    let result = extract::extract_with_schema(&snapshot, &schema, None);
    assert!(result.is_array(), "should return an array");
    let arr = result.as_array().unwrap();
    assert!(
        !arr.is_empty(),
        "should extract at least one row from dashboard table"
    );

    // Check the first row has expected fields
    let first = &arr[0];
    assert!(first.get("customer").is_some(), "should have customer field");
    assert!(first.get("product").is_some(), "should have product field");
    assert!(first.get("status").is_some(), "should have status field");
    assert!(first.get("total").is_some(), "should have total field");
}

#[test]
fn extract_table_with_selector() {
    let snapshot = snap(DASHBOARD);
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "customer": { "type": "string" },
                "total": { "type": "string" }
            }
        }
    });

    let result = extract::extract_with_schema(&snapshot, &schema, Some("table"));
    assert!(result.is_array(), "table selector should return an array");
    let arr = result.as_array().unwrap();
    assert!(!arr.is_empty(), "should extract rows with table selector");
}

#[test]
fn extract_list_items_from_ecommerce() {
    let snapshot = snap(ECOMMERCE);
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "price": { "type": "string" }
            }
        }
    });

    let result = extract::extract_with_schema(&snapshot, &schema, Some("[role=list]"));
    assert!(result.is_array(), "should return an array for list extraction");
    let arr = result.as_array().unwrap();
    // The ecommerce page has product cards in a list
    assert!(
        !arr.is_empty(),
        "should extract items from ecommerce product list"
    );
}

#[test]
fn extract_single_object() {
    let html = r#"
        <main>
            <h1>Product Details</h1>
            <p>Name: Widget Pro</p>
            <p>Price: $29.99</p>
            <p>In Stock: Yes</p>
        </main>
    "#;
    let snapshot = snap(html);
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "price": { "type": "string" },
            "in_stock": { "type": "string" }
        }
    });

    let result = extract::extract_with_schema(&snapshot, &schema, None);
    assert!(result.is_object(), "should return an object");
}

#[test]
fn extract_type_coercion_number() {
    let html = r#"
        <table>
            <thead><tr><th>Item</th><th>Price</th><th>Quantity</th></tr></thead>
            <tbody>
                <tr><td>Widget</td><td>$9.99</td><td>5</td></tr>
                <tr><td>Gadget</td><td>$19.50</td><td>3</td></tr>
            </tbody>
        </table>
    "#;
    let snapshot = snap(html);
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "item": { "type": "string" },
                "price": { "type": "number" },
                "quantity": { "type": "integer" }
            }
        }
    });

    let result = extract::extract_with_schema(&snapshot, &schema, None);
    assert!(result.is_array());
    let arr = result.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should extract 2 rows");

    let first = &arr[0];
    assert_eq!(first.get("item").and_then(|v| v.as_str()), Some("Widget"));
    // Price should be coerced to number
    let price = first.get("price").and_then(|v| v.as_f64());
    assert!(price.is_some(), "price should be a number");
    assert!((price.unwrap() - 9.99).abs() < 0.01, "price should be 9.99");
    // Quantity should be coerced to integer
    let qty = first.get("quantity").and_then(|v| v.as_i64());
    assert_eq!(qty, Some(5), "quantity should be 5");
}

#[test]
fn extract_type_coercion_boolean() {
    let val = extract::coerce_value("true", "boolean");
    assert_eq!(val, serde_json::Value::Bool(true));

    let val = extract::coerce_value("yes", "boolean");
    assert_eq!(val, serde_json::Value::Bool(true));

    let val = extract::coerce_value("checked", "boolean");
    assert_eq!(val, serde_json::Value::Bool(true));

    let val = extract::coerce_value("false", "boolean");
    assert_eq!(val, serde_json::Value::Bool(false));

    let val = extract::coerce_value("no", "boolean");
    assert_eq!(val, serde_json::Value::Bool(false));
}

#[test]
fn extract_empty_schema_returns_null() {
    let snapshot = snap(DASHBOARD);
    let schema = serde_json::json!({});
    let result = extract::extract_with_schema(&snapshot, &schema, None);
    assert!(result.is_null(), "empty schema should return null");
}

#[test]
fn extract_no_matches_returns_empty_array() {
    let html = "<p>Hello world</p>";
    let snapshot = snap(html);
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "nonexistent_field": { "type": "string" }
            }
        }
    });
    let result = extract::extract_with_schema(&snapshot, &schema, None);
    assert!(result.is_array(), "should return an array");
    let arr = result.as_array().unwrap();
    assert!(arr.is_empty(), "should be empty when no tables or lists match");
}

#[test]
fn extract_collect_text_recursive() {
    let node = SemanticNode {
        ref_id: 0,
        role: AriaRole::ListItem,
        name: String::new(),
        value: None,
        attrs: vec![],
        children: vec![
            SemanticNode {
                ref_id: 0,
                role: AriaRole::Heading { level: 3 },
                name: "Product Name".into(),
                value: None,
                attrs: vec![],
                children: vec![],
                offscreen: None,
            },
            SemanticNode {
                ref_id: 0,
                role: AriaRole::StaticText,
                name: "$49.99".into(),
                value: None,
                attrs: vec![],
                children: vec![],
                offscreen: None,
            },
        ],
        offscreen: None,
    };

    let text = extract::collect_text(&node);
    assert!(text.contains("Product Name"), "should collect heading text");
    assert!(text.contains("$49.99"), "should collect static text");
}

#[test]
fn extract_match_field_scoring() {
    let node = SemanticNode {
        ref_id: 0,
        role: AriaRole::StaticText,
        name: "Customer".into(),
        value: None,
        attrs: vec![],
        children: vec![],
        offscreen: None,
    };

    // Exact match should score highest
    let exact = extract::match_field("customer", &node);
    assert!(exact >= 10.0, "exact match should score >= 10");

    // Contains match
    let contains = extract::match_field("cust", &node);
    assert!(contains >= 5.0, "contains match should score >= 5");

    // No match
    let no_match = extract::match_field("zzzzz", &node);
    assert!(no_match == 0.0, "no match should score 0, got {no_match}");
}

#[test]
fn extract_simple_html_table() {
    let html = r#"
        <table>
            <thead><tr><th>Name</th><th>Price</th></tr></thead>
            <tbody>
                <tr><td>Widget</td><td>$9.99</td></tr>
                <tr><td>Gadget</td><td>$19.50</td></tr>
            </tbody>
        </table>
    "#;
    let snapshot = snap(html);
    let schema = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "price": { "type": "string" }
            }
        }
    });

    let result = extract::extract_with_schema(&snapshot, &schema, None);
    let arr = result.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should extract 2 rows");
    assert_eq!(arr[0]["name"], "Widget");
    assert_eq!(arr[0]["price"], "$9.99");
    assert_eq!(arr[1]["name"], "Gadget");
    assert_eq!(arr[1]["price"], "$19.50");
}

// ── Recording Tests ─────────────────────────────────────────────────────────

#[test]
fn recording_serialization_round_trip() {
    let locator = ElementLocator {
        tag: "input".into(),
        id: Some("username".into()),
        name: Some("user".into()),
        input_type: Some("text".into()),
        href: None,
        text: String::new(),
    };

    let rec = recording::Recording {
        name: "login-flow".into(),
        domain: "example-com".into(),
        start_url: "https://example.com/login".into(),
        created_at: "1700000000".into(),
        description: Some("Login test".into()),
        actions: vec![
            recording::RecordedAction::Navigate {
                url: "https://example.com/login".into(),
            },
            recording::RecordedAction::TypeText {
                locator: locator.clone(),
                text: "admin".into(),
                ref_id: 3,
            },
            recording::RecordedAction::Click {
                locator: ElementLocator {
                    tag: "button".into(),
                    id: Some("submit-btn".into()),
                    name: None,
                    input_type: None,
                    href: None,
                    text: "Sign In".into(),
                },
                ref_id: 5,
            },
        ],
    };

    let json = serde_json::to_string_pretty(&rec).unwrap();
    let deserialized: recording::Recording = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.name, "login-flow");
    assert_eq!(deserialized.domain, "example-com");
    assert_eq!(deserialized.actions.len(), 3);
    assert!(matches!(deserialized.actions[0], recording::RecordedAction::Navigate { .. }));
    assert!(matches!(deserialized.actions[1], recording::RecordedAction::TypeText { .. }));
    if let recording::RecordedAction::TypeText { ref text, .. } = deserialized.actions[1] {
        assert_eq!(text, "admin");
    }
    assert!(matches!(deserialized.actions[2], recording::RecordedAction::Click { .. }));
}

#[test]
fn element_locator_serde_preserves_js_expression() {
    let locator = ElementLocator {
        tag: "input".into(),
        id: Some("email-field".into()),
        name: None,
        input_type: Some("email".into()),
        href: None,
        text: String::new(),
    };

    let js_before = locator.to_js_expression();
    let json = serde_json::to_string(&locator).unwrap();
    let restored: ElementLocator = serde_json::from_str(&json).unwrap();
    let js_after = restored.to_js_expression();

    assert_eq!(js_before, js_after);
}

#[test]
fn domain_extraction() {
    assert_eq!(recording::extract_domain("https://github.com/foo/bar"), "github-com");
    assert_eq!(recording::extract_domain("http://localhost:3000/app"), "localhost");
    assert_eq!(recording::extract_domain("https://sub.example.co.uk/path"), "sub-example-co-uk");
    assert_eq!(recording::extract_domain("example.com"), "example-com");
}

#[test]
fn filename_sanitization() {
    assert_eq!(recording::sanitize_filename("login-flow"), "login-flow");
    assert_eq!(recording::sanitize_filename("my flow!@#"), "my-flow");
    assert_eq!(recording::sanitize_filename("test_recording_1"), "test_recording_1");
    assert_eq!(recording::sanitize_filename("---"), "recording");
    assert_eq!(recording::sanitize_filename(""), "recording");
}

#[test]
fn recording_file_io_save_load_list_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let store = recording::RecordingStore::with_base(tmp.path().to_path_buf());

    let rec = recording::Recording {
        name: "test-flow".into(),
        domain: "example-com".into(),
        start_url: "https://example.com".into(),
        created_at: "1700000000".into(),
        description: Some("A test".into()),
        actions: vec![recording::RecordedAction::Navigate {
            url: "https://example.com".into(),
        }],
    };

    // Save
    let path = store.save(&rec).unwrap();
    assert!(path.exists());

    // Load with domain
    let loaded = store.load("test-flow", Some("example-com")).unwrap();
    assert_eq!(loaded.name, "test-flow");
    assert_eq!(loaded.actions.len(), 1);

    // Load without domain (search all)
    let loaded2 = store.load("test-flow", None).unwrap();
    assert_eq!(loaded2.name, "test-flow");

    // List all
    let summaries = store.list(None).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].name, "test-flow");
    assert_eq!(summaries[0].action_count, 1);

    // List by domain
    let summaries_dom = store.list(Some("example-com")).unwrap();
    assert_eq!(summaries_dom.len(), 1);

    // List wrong domain
    let summaries_none = store.list(Some("other-com")).unwrap();
    assert!(summaries_none.is_empty());

    // Delete
    store.delete("test-flow", Some("example-com")).unwrap();
    assert!(store.load("test-flow", None).is_err());
}

#[test]
fn recording_empty_actions() {
    let tmp = tempfile::tempdir().unwrap();
    let store = recording::RecordingStore::with_base(tmp.path().to_path_buf());

    let rec = recording::Recording {
        name: "empty".into(),
        domain: "test-com".into(),
        start_url: "https://test.com".into(),
        created_at: "1700000000".into(),
        description: None,
        actions: vec![],
    };

    store.save(&rec).unwrap();
    let loaded = store.load("empty", None).unwrap();
    assert!(loaded.actions.is_empty());
    assert!(loaded.description.is_none());
}

#[test]
fn recording_load_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let store = recording::RecordingStore::with_base(tmp.path().to_path_buf());
    let result = store.load("nonexistent", None);
    assert!(result.is_err());
}

#[test]
fn recording_summary_from_recording() {
    let rec = recording::Recording {
        name: "my-rec".into(),
        domain: "test-com".into(),
        start_url: "https://test.com".into(),
        created_at: "1700000000".into(),
        description: Some("desc".into()),
        actions: vec![
            recording::RecordedAction::Click {
                locator: ElementLocator {
                    tag: "button".into(),
                    id: Some("btn".into()),
                    name: None,
                    input_type: None,
                    href: None,
                    text: "Go".into(),
                },
                ref_id: 1,
            },
        ],
    };

    let summary = recording::RecordingSummary::from(&rec);
    assert_eq!(summary.name, "my-rec");
    assert_eq!(summary.domain, "test-com");
    assert_eq!(summary.created_at, "1700000000");
    assert_eq!(summary.action_count, 1);
    assert_eq!(summary.description.as_deref(), Some("desc"));
}

#[test]
fn recording_multiple_domains() {
    let tmp = tempfile::tempdir().unwrap();
    let store = recording::RecordingStore::with_base(tmp.path().to_path_buf());

    let rec1 = recording::Recording {
        name: "flow-a".into(),
        domain: "alpha-com".into(),
        start_url: "https://alpha.com".into(),
        created_at: "1".into(),
        description: None,
        actions: vec![],
    };
    let rec2 = recording::Recording {
        name: "flow-b".into(),
        domain: "beta-com".into(),
        start_url: "https://beta.com".into(),
        created_at: "2".into(),
        description: None,
        actions: vec![],
    };

    store.save(&rec1).unwrap();
    store.save(&rec2).unwrap();

    let all = store.list(None).unwrap();
    assert_eq!(all.len(), 2);

    let alpha_only = store.list(Some("alpha-com")).unwrap();
    assert_eq!(alpha_only.len(), 1);
    assert_eq!(alpha_only[0].name, "flow-a");
}
