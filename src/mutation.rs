//! DOM mutation observer management for incremental re-snapshots.
//!
//! After navigating to a page, we inject a MutationObserver via CDP that
//! tracks DOM changes. This lets us:
//! 1. Return cached snapshots when nothing changed (skip re-processing)
//! 2. Know how much the page changed (mutation count)
//! 3. Wait for the page to update after async actions

/// JavaScript to inject a MutationObserver that tracks DOM changes.
/// Safe to call multiple times - disconnects any previous observer first.
pub const INSTALL_OBSERVER_JS: &str = r#"(function() {
    if (window.__cortex_observer) {
        window.__cortex_observer.disconnect();
    }
    window.__cortex_dirty = false;
    window.__cortex_mutation_count = 0;
    window.__cortex_observer = new MutationObserver(function(mutations) {
        window.__cortex_dirty = true;
        window.__cortex_mutation_count += mutations.length;
    });
    var target = document.body || document.documentElement;
    if (target) {
        window.__cortex_observer.observe(target, {
            childList: true,
            attributes: true,
            characterData: true,
            subtree: true
        });
    }
    return 'installed';
})()"#;

/// JavaScript to check if the DOM has mutations since last reset.
/// Returns JSON: {"dirty": bool, "count": number}
pub const CHECK_DIRTY_JS: &str = r#"(function() {
    return JSON.stringify({
        dirty: !!window.__cortex_dirty,
        count: window.__cortex_mutation_count || 0
    });
})()"#;

/// JavaScript to reset the dirty state after taking a snapshot.
/// Returns the mutation count before reset.
pub const RESET_DIRTY_JS: &str = r#"(function() {
    var count = window.__cortex_mutation_count || 0;
    window.__cortex_dirty = false;
    window.__cortex_mutation_count = 0;
    return count;
})()"#;

/// JavaScript to get viewport dimensions. Returns JSON with scrollY, viewportHeight, documentHeight.
pub const GET_VIEWPORT_JS: &str = r#"(function() {
    return JSON.stringify({
        scrollY: Math.round(window.scrollY || window.pageYOffset || 0),
        viewportHeight: window.innerHeight || document.documentElement.clientHeight || 0,
        documentHeight: Math.max(
            document.body ? document.body.scrollHeight : 0,
            document.documentElement.scrollHeight || 0
        )
    });
})()"#;

/// JavaScript to scroll down by 85% of viewport height.
pub const SCROLL_DOWN_JS: &str = r#"(function() {
    var amount = Math.round(window.innerHeight * 0.85);
    window.scrollBy(0, amount);
    return amount;
})()"#;

/// JavaScript to scroll up by 85% of viewport height.
pub const SCROLL_UP_JS: &str = r#"(function() {
    var amount = Math.round(window.innerHeight * 0.85);
    window.scrollBy(0, -amount);
    return amount;
})()"#;

/// JavaScript to get visibility of elements by their ref locator JS expressions.
/// Dynamically constructed per-snapshot based on ref entries.
pub fn build_check_visibility_js(ref_expressions: &[(u32, String)]) -> String {
    let mut checks = String::from(
        "(function() { var vt = window.scrollY || 0; var vh = window.innerHeight; var result = {};",
    );
    for (ref_id, js_expr) in ref_expressions {
        checks.push_str(&format!(
            " try {{ var el = {js_expr}; if (el) {{ var r = el.getBoundingClientRect(); result['{ref_id}'] = !(r.bottom < 0 || r.top > vh); }} }} catch(e) {{}}",
        ));
    }
    checks.push_str(" return JSON.stringify(result); })()");
    checks
}

/// Parsed result of CHECK_DIRTY_JS.
#[derive(Debug)]
pub struct DirtyState {
    pub dirty: bool,
    /// Number of mutations since last reset. Available for diagnostics.
    #[allow(dead_code)]
    pub mutation_count: u64,
}

impl DirtyState {
    pub fn from_json(json: &str) -> Self {
        #[derive(serde::Deserialize)]
        struct Raw {
            dirty: bool,
            count: u64,
        }

        match serde_json::from_str::<Raw>(json) {
            Ok(raw) => DirtyState {
                dirty: raw.dirty,
                mutation_count: raw.count,
            },
            Err(_) => DirtyState {
                dirty: true, // assume dirty on parse failure (e.g. page navigated)
                mutation_count: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_constants_are_non_empty() {
        assert!(!INSTALL_OBSERVER_JS.is_empty());
        assert!(!CHECK_DIRTY_JS.is_empty());
        assert!(!RESET_DIRTY_JS.is_empty());
    }

    #[test]
    fn install_js_is_valid_iife() {
        assert!(INSTALL_OBSERVER_JS.starts_with("(function()"));
        assert!(INSTALL_OBSERVER_JS.trim_end().ends_with("()"));
    }

    #[test]
    fn dirty_state_clean() {
        let s = DirtyState::from_json(r#"{"dirty":false,"count":0}"#);
        assert!(!s.dirty);
        assert_eq!(s.mutation_count, 0);
    }

    #[test]
    fn dirty_state_dirty_with_count() {
        let s = DirtyState::from_json(r#"{"dirty":true,"count":15}"#);
        assert!(s.dirty);
        assert_eq!(s.mutation_count, 15);
    }

    #[test]
    fn dirty_state_malformed_assumes_dirty() {
        assert!(DirtyState::from_json("").dirty);
        assert!(DirtyState::from_json("null").dirty);
        assert!(DirtyState::from_json("{broken").dirty);
        assert!(DirtyState::from_json("42").dirty);
    }

    #[test]
    fn dirty_state_missing_fields() {
        // Missing "count" field
        assert!(DirtyState::from_json(r#"{"dirty":true}"#).dirty);
    }

    #[test]
    fn viewport_js_constants_are_non_empty() {
        assert!(!GET_VIEWPORT_JS.is_empty());
        assert!(!SCROLL_DOWN_JS.is_empty());
        assert!(!SCROLL_UP_JS.is_empty());
    }

    #[test]
    fn build_check_visibility_js_produces_iife() {
        let refs = vec![(12345u32, "document.getElementById('btn')".to_string())];
        let js = build_check_visibility_js(&refs);
        assert!(js.starts_with("(function()"));
        assert!(js.contains("12345"));
        assert!(js.contains("getElementById"));
    }
}
