use serde_json::{Map, Value};

use crate::dom::{AriaRole, PageSnapshot, SemanticNode};

pub fn extract_with_schema(
    snapshot: &PageSnapshot,
    schema: &Value,
    selector: Option<&str>,
) -> Value {
    let nodes = if let Some(sel) = selector {
        find_by_selector(&snapshot.nodes, sel)
    } else {
        snapshot.nodes.iter().collect()
    };

    if nodes.is_empty() {
        return Value::Null;
    }

    let schema_type = schema.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match schema_type {
        "array" => {
            let items_schema = schema.get("items").cloned().unwrap_or(Value::Null);
            let properties = items_schema
                .get("properties")
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();

            let tables = find_tables(&nodes);
            if let Some(table) = tables.first() {
                let rows = extract_table(table, &properties);
                if !rows.is_empty() {
                    return Value::Array(rows);
                }
            }

            let lists = find_repeated_lists(&nodes);
            if let Some(items) = lists.first() {
                let extracted = extract_list_items(items, &properties);
                if !extracted.is_empty() {
                    return Value::Array(extracted);
                }
            }

            let all_items = collect_all_items(&nodes, &properties);
            Value::Array(all_items)
        }
        "object" => {
            let properties = schema
                .get("properties")
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();
            extract_single_object(&nodes, &properties)
        }
        _ => Value::Null,
    }
}

fn find_by_selector<'a>(nodes: &'a [SemanticNode], selector: &str) -> Vec<&'a SemanticNode> {
    let mut results = Vec::new();
    find_by_selector_recursive(nodes, selector, &mut results);
    results
}

fn find_by_selector_recursive<'a>(
    nodes: &'a [SemanticNode],
    selector: &str,
    results: &mut Vec<&'a SemanticNode>,
) {
    for node in nodes {
        if matches_selector(node, selector) {
            results.push(node);
        }
        find_by_selector_recursive(&node.children, selector, results);
    }
}

fn matches_selector(node: &SemanticNode, selector: &str) -> bool {
    let sel = selector.trim();

    if sel.starts_with("[role=") && sel.ends_with(']') {
        let role_name = &sel[6..sel.len() - 1].trim_matches('"').trim_matches('\'');
        let node_role = format!("{}", node.role);
        return node_role.eq_ignore_ascii_case(role_name);
    }

    let node_role = format!("{}", node.role);
    node_role.eq_ignore_ascii_case(sel)
}

pub fn find_tables<'a>(nodes: &[&'a SemanticNode]) -> Vec<&'a SemanticNode> {
    let mut tables = Vec::new();
    for node in nodes {
        find_tables_recursive(node, &mut tables);
    }
    tables
}

fn find_tables_recursive<'a>(node: &'a SemanticNode, tables: &mut Vec<&'a SemanticNode>) {
    if node.role == AriaRole::Table {
        tables.push(node);
    }
    for child in &node.children {
        find_tables_recursive(child, tables);
    }
}

pub fn find_repeated_lists<'a>(nodes: &[&'a SemanticNode]) -> Vec<Vec<&'a SemanticNode>> {
    let mut lists = Vec::new();
    for node in nodes {
        find_repeated_lists_recursive(node, &mut lists);
    }
    lists
}

fn find_repeated_lists_recursive<'a>(
    node: &'a SemanticNode,
    lists: &mut Vec<Vec<&'a SemanticNode>>,
) {
    if node.role == AriaRole::List {
        let items: Vec<&SemanticNode> = node
            .children
            .iter()
            .filter(|c| c.role == AriaRole::ListItem)
            .collect();
        if items.len() >= 2 {
            lists.push(items);
        }
    }

    let list_items: Vec<&SemanticNode> = node
        .children
        .iter()
        .filter(|c| c.role == AriaRole::ListItem)
        .collect();
    if list_items.len() >= 2 && node.role != AriaRole::List {
        lists.push(list_items);
    }

    for child in &node.children {
        find_repeated_lists_recursive(child, lists);
    }
}

pub fn extract_table(table: &SemanticNode, properties: &Map<String, Value>) -> Vec<Value> {
    let headers = collect_column_headers(table);
    if headers.is_empty() {
        return Vec::new();
    }

    let column_map = map_properties_to_columns(properties, &headers);
    let rows = collect_rows(table);
    let mut results = Vec::new();

    for row in &rows {
        let cells = collect_cells(row);
        let mut obj = Map::new();

        for (prop_name, col_idx) in &column_map {
            if let Some(cell) = cells.get(*col_idx) {
                let prop_schema = properties.get(prop_name);
                let schema_type = prop_schema
                    .and_then(|s| s.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("string");
                let text = collect_text(cell);
                let value = coerce_value(&text, schema_type);
                obj.insert(prop_name.clone(), value);
            }
        }

        if !obj.is_empty() {
            results.push(Value::Object(obj));
        }
    }

    results
}

pub fn extract_list_items(items: &[&SemanticNode], properties: &Map<String, Value>) -> Vec<Value> {
    let mut results = Vec::new();

    for item in items {
        let obj = extract_object_from_node(item, properties);
        if !obj.as_object().is_none_or(|o| o.is_empty()) {
            results.push(obj);
        }
    }

    results
}

pub fn extract_single_object(nodes: &[&SemanticNode], properties: &Map<String, Value>) -> Value {
    let mut obj = Map::new();

    for (prop_name, prop_schema) in properties {
        let schema_type = prop_schema
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("string");

        let mut best_score: f32 = 0.0;
        let mut best_text = String::new();

        for node in nodes {
            scan_for_field(node, prop_name, &mut best_score, &mut best_text);
        }

        if best_score > 0.0 {
            obj.insert(prop_name.clone(), coerce_value(&best_text, schema_type));
        }
    }

    Value::Object(obj)
}

pub fn match_field(name: &str, node: &SemanticNode) -> f32 {
    let name_lower = name.to_lowercase();
    let node_name_lower = node.name.to_lowercase();

    if node_name_lower.is_empty() {
        return 0.0;
    }

    let mut score: f32 = 0.0;

    if node_name_lower == name_lower {
        score += 10.0;
    } else if node_name_lower.contains(&name_lower) || name_lower.contains(&node_name_lower) {
        score += 5.0;
    }

    score += role_hint_score(&name_lower, node);

    score
}

pub fn coerce_value(text: &str, schema_type: &str) -> Value {
    let trimmed = text.trim();
    match schema_type {
        "number" | "integer" => {
            let numeric: String = trimmed
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            if schema_type == "integer" {
                numeric.parse::<i64>().map(Value::from).unwrap_or_else(|_| {
                    numeric
                        .parse::<f64>()
                        .map(|f| Value::from(f as i64))
                        .unwrap_or(Value::Null)
                })
            } else {
                numeric
                    .parse::<f64>()
                    .map(Value::from)
                    .unwrap_or(Value::Null)
            }
        }
        "boolean" => {
            let lower = trimmed.to_lowercase();
            let is_true = matches!(lower.as_str(), "true" | "yes" | "checked" | "1");
            Value::Bool(is_true)
        }
        _ => Value::String(trimmed.to_string()),
    }
}

pub fn collect_text(node: &SemanticNode) -> String {
    let mut parts = Vec::new();
    collect_text_recursive(node, &mut parts);
    parts.join(" ").trim().to_string()
}

fn collect_text_recursive(node: &SemanticNode, parts: &mut Vec<String>) {
    if !node.name.is_empty() {
        parts.push(node.name.clone());
        // The pipeline sets parent name from child text, so recursing would duplicate.
        return;
    }
    for child in &node.children {
        collect_text_recursive(child, parts);
    }
}

fn collect_column_headers(table: &SemanticNode) -> Vec<String> {
    let mut headers = Vec::new();
    collect_column_headers_recursive(table, &mut headers);
    headers
}

fn collect_column_headers_recursive(node: &SemanticNode, headers: &mut Vec<String>) {
    if node.role == AriaRole::ColumnHeader {
        headers.push(node.name.clone());
    }
    for child in &node.children {
        collect_column_headers_recursive(child, headers);
    }
}

fn collect_rows(table: &SemanticNode) -> Vec<&SemanticNode> {
    let mut rows = Vec::new();
    collect_rows_recursive(table, &mut rows);
    rows
}

fn collect_rows_recursive<'a>(node: &'a SemanticNode, rows: &mut Vec<&'a SemanticNode>) {
    if node.role == AriaRole::Row {
        let has_cells = node.children.iter().any(|c| c.role == AriaRole::Cell);
        if has_cells {
            rows.push(node);
        }
    }
    for child in &node.children {
        collect_rows_recursive(child, rows);
    }
}

fn collect_cells(row: &SemanticNode) -> Vec<&SemanticNode> {
    row.children
        .iter()
        .filter(|c| c.role == AriaRole::Cell)
        .collect()
}

fn map_properties_to_columns(
    properties: &Map<String, Value>,
    headers: &[String],
) -> Vec<(String, usize)> {
    let mut mapping = Vec::new();

    for prop_name in properties.keys() {
        let prop_lower = prop_name.to_lowercase();
        let prop_words = split_name_words(&prop_lower);

        let mut best_idx = None;
        let mut best_score: f32 = 0.0;

        for (idx, header) in headers.iter().enumerate() {
            let header_lower = header.to_lowercase();
            let header_words = split_name_words(&header_lower);

            let mut score: f32 = 0.0;

            if header_lower == prop_lower {
                score += 10.0;
            } else if header_lower.contains(&prop_lower) || prop_lower.contains(&header_lower) {
                score += 5.0;
            } else {
                let overlap = prop_words
                    .iter()
                    .filter(|w| {
                        header_words
                            .iter()
                            .any(|hw| hw.contains(w.as_str()) || w.contains(hw.as_str()))
                    })
                    .count();
                if overlap > 0 {
                    score += overlap as f32 * 3.0;
                }
            }

            if score > best_score {
                best_score = score;
                best_idx = Some(idx);
            }
        }

        if let Some(idx) = best_idx {
            if best_score > 0.0 {
                mapping.push((prop_name.clone(), idx));
            }
        }
    }

    mapping
}

fn split_name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in name.chars() {
        if ch == '_' || ch == ' ' || ch == '-' {
            if !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
        } else if ch.is_uppercase() && !current.is_empty() {
            words.push(current.clone());
            current.clear();
            current.push(ch.to_lowercase().next().unwrap_or(ch));
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn role_hint_score(name: &str, node: &SemanticNode) -> f32 {
    let mut score: f32 = 0.0;
    let text = node.name.to_lowercase();

    if (name.contains("price") || name.contains("cost") || name.contains("total"))
        && (text.contains('$') || text.contains('€') || text.contains('£'))
    {
        score += 3.0;
    }

    if name.contains("link") || name.contains("url") || name.contains("href") {
        if node.role == AriaRole::Link {
            score += 3.0;
        }
        if node.attrs.iter().any(|(k, _)| k == "href") {
            score += 3.0;
        }
    }

    if (name.contains("rating") || name.contains("score") || name.contains("stars"))
        && text.chars().any(|c| c.is_ascii_digit())
        && text.len() <= 5
    {
        score += 3.0;
    }

    if name.contains("status") || name.contains("state") {
        let status_words = [
            "delivered",
            "shipped",
            "processing",
            "pending",
            "cancelled",
            "active",
            "inactive",
            "completed",
        ];
        if status_words.iter().any(|w| text.contains(w)) {
            score += 3.0;
        }
    }

    score
}

fn scan_for_field(
    node: &SemanticNode,
    prop_name: &str,
    best_score: &mut f32,
    best_text: &mut String,
) {
    let score = match_field(prop_name, node);
    if score > *best_score {
        *best_score = score;
        *best_text = collect_text(node);
    }

    if is_label_like(node) {
        let label_score = match_field(prop_name, node);
        if label_score > 0.0 {
            for child in &node.children {
                if !child.name.is_empty() && child.role != node.role {
                    let adjusted_score = label_score + 2.0;
                    if adjusted_score > *best_score {
                        *best_score = adjusted_score;
                        *best_text = collect_text(child);
                    }
                }
            }
        }
    }

    for child in &node.children {
        scan_for_field(child, prop_name, best_score, best_text);
    }
}

fn is_label_like(node: &SemanticNode) -> bool {
    matches!(
        node.role,
        AriaRole::Heading { .. } | AriaRole::ColumnHeader | AriaRole::StaticText
    )
}

fn extract_object_from_node(node: &SemanticNode, properties: &Map<String, Value>) -> Value {
    let mut obj = Map::new();

    let mut flat_nodes = Vec::new();
    flatten_node(node, &mut flat_nodes);

    for (prop_name, prop_schema) in properties {
        let schema_type = prop_schema
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("string");

        let mut best_score: f32 = 0.0;
        let mut best_text = String::new();

        for descendant in &flat_nodes {
            let score = match_field(prop_name, descendant);
            if score > best_score {
                best_score = score;
                best_text = collect_text(descendant);
            }

            let hint_score = role_hint_score(&prop_name.to_lowercase(), descendant);
            if hint_score > 0.0 && (hint_score + 1.0) > best_score {
                let text = collect_text(descendant);
                if !text.is_empty() && text != best_text {
                    let combined = hint_score + 1.0;
                    if combined > best_score {
                        best_score = combined;
                        best_text = text;
                    }
                }
            }
        }

        if best_score > 0.0 {
            obj.insert(prop_name.clone(), coerce_value(&best_text, schema_type));
        }
    }

    Value::Object(obj)
}

fn flatten_node<'a>(node: &'a SemanticNode, out: &mut Vec<&'a SemanticNode>) {
    out.push(node);
    for child in &node.children {
        flatten_node(child, out);
    }
}

fn collect_all_items(nodes: &[&SemanticNode], properties: &Map<String, Value>) -> Vec<Value> {
    for node in nodes {
        let lists = find_repeated_lists_in_node(node);
        if let Some(items) = lists.first() {
            let extracted = extract_list_items(items, properties);
            if !extracted.is_empty() {
                return extracted;
            }
        }
    }
    Vec::new()
}

fn find_repeated_lists_in_node(node: &SemanticNode) -> Vec<Vec<&SemanticNode>> {
    let mut lists = Vec::new();
    find_repeated_lists_recursive(node, &mut lists);
    lists
}
