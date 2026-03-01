use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use base64::Engine as _;
use chromiumoxide::Browser;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::RwLock;

use tracing::{debug, info, warn};

use crate::dom::RefIndex;
use crate::{browser, diff, extract, hints, mutation, pipeline, recording, serialize};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NavigateParams {
    /// The URL to navigate to (e.g., "https://example.com")
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickParams {
    /// The ref ID of the element to click (the number N from @eN in the snapshot)
    pub r#ref: u32,
    /// If true, return a compact diff instead of a full snapshot (compares with previous snapshot)
    #[serde(default)]
    pub return_diff: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TypeTextParams {
    /// The ref ID of the input element (the number N from @eN)
    pub r#ref: u32,
    /// The text to type into the element
    pub text: String,
    /// If true, return a compact diff instead of a full snapshot
    #[serde(default)]
    pub return_diff: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SelectOptionParams {
    /// The ref ID of the select/combobox element (the number N from @eN)
    pub r#ref: u32,
    /// The value or visible text of the option to select
    pub value: String,
    /// If true, return a compact diff instead of a full snapshot
    #[serde(default)]
    pub return_diff: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitForChangesParams {
    /// Maximum time to wait for DOM changes in milliseconds (default: 5000)
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetTaskContextParams {
    /// Description of the current task (e.g., "Find and fill the login form")
    pub task: String,
    /// Text patterns to focus on - elements whose name contains any of these
    /// strings will be prioritized (e.g., ["login", "sign in", "password"])
    #[serde(default)]
    pub focus_text: Vec<String>,
    /// ARIA roles to prioritize (e.g., ["button", "textbox", "form"])
    #[serde(default)]
    pub focus_roles: Vec<String>,
    /// If true, primarily show interactive elements and their structural parents
    #[serde(default)]
    pub interactive_only: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FocusedSnapshotParams {
    /// Text patterns to focus on (e.g., ["search", "query"])
    #[serde(default)]
    pub focus_text: Vec<String>,
    /// ARIA roles to prioritize (e.g., ["button", "link"])
    #[serde(default)]
    pub focus_roles: Vec<String>,
    /// If true, only show interactive elements
    #[serde(default)]
    pub interactive_only: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OpenTabParams {
    /// The URL to open in the new tab
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SwitchTabParams {
    /// The tab ID to switch to (from list_tabs)
    pub tab_id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CloseTabParams {
    /// The tab ID to close
    pub tab_id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScrollToRefParams {
    /// The ref ID of the element to scroll into view (the number N from @eN)
    pub r#ref: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtractParams {
    /// A JSON Schema object describing the desired output shape. Supports objects with
    /// properties, arrays of objects, and primitive types (string, number, boolean).
    /// Example: {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "price": {"type": "number"}}}}
    pub schema: serde_json::Value,
    /// Optional CSS selector to scope extraction to a specific region (e.g., "table", "[role=list]").
    /// If omitted, extracts from the full page.
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartRecordingParams {
    /// A short name for this recording (e.g., "login-flow")
    pub name: String,
    /// Optional description of what this recording does
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplayRecordingParams {
    /// The name of the recording to replay
    pub name: String,
    /// Optional domain to narrow the search (e.g., "github-com")
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRecordingsParams {
    /// Optional domain to filter by (e.g., "github-com")
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteRecordingParams {
    /// The name of the recording to delete
    pub name: String,
    /// Optional domain to narrow the search
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    /// If true, capture the entire scrollable page instead of just the viewport (default: false)
    #[serde(default)]
    pub full_page: Option<bool>,
    /// If true, overlay interactive elements with red borders and @eN labels (default: false)
    #[serde(default)]
    pub annotate: Option<bool>,
}

struct TabState {
    page: chromiumoxide::Page,
    ref_index: RefIndex,
    current_url: String,
    cached_snapshot: Option<String>,
    observer_installed: bool,
    task_context: Option<hints::TaskContext>,
    /// Previous snapshot tree for page diff computation.
    previous_snapshot: Option<crate::dom::PageSnapshot>,
}

struct BrowserState {
    browser: Option<Browser>,
    tabs: HashMap<u32, TabState>,
    active_tab: u32,
    next_tab_id: u32,
    active_recording: Option<recording::Recording>,
    replaying: bool,
}

impl BrowserState {
    fn new() -> Self {
        Self {
            browser: None,
            tabs: HashMap::new(),
            active_tab: 0,
            next_tab_id: 1,
            active_recording: None,
            replaying: false,
        }
    }

    fn active_tab(&self) -> anyhow::Result<&TabState> {
        self.tabs
            .get(&self.active_tab)
            .with_context(|| "No active tab. Use navigate or open_tab first.")
    }

    fn active_tab_mut(&mut self) -> anyhow::Result<&mut TabState> {
        let id = self.active_tab;
        self.tabs
            .get_mut(&id)
            .with_context(|| "No active tab. Use navigate or open_tab first.")
    }

    /// Push an action to the active recording (if any and not replaying).
    fn record(&mut self, action: recording::RecordedAction) {
        if self.replaying {
            return;
        }
        if let Some(ref mut rec) = self.active_recording {
            rec.actions.push(action);
        }
    }
}

#[derive(Clone)]
pub struct CortexBrowserServer {
    tool_router: ToolRouter<Self>,
    state: Arc<RwLock<BrowserState>>,
    store: Arc<recording::RecordingStore>,
    launch_browser: bool,
    port: u16,
}

#[tool_router]
impl CortexBrowserServer {
    pub fn new(launch_browser: bool, port: u16) -> Self {
        Self {
            tool_router: Self::tool_router(),
            state: Arc::new(RwLock::new(BrowserState::new())),
            store: Arc::new(recording::RecordingStore::new()),
            launch_browser,
            port,
        }
    }

    #[tool(
        description = "Navigate to a URL and return a compact page snapshot. Interactive elements are labeled @eN - use these refs with click, type_text, select_option."
    )]
    async fn navigate(&self, Parameters(params): Parameters<NavigateParams>) -> String {
        match self.do_navigate(&params.url).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Navigation failed: {e}"),
        }
    }

    #[tool(
        description = "Return a snapshot of the current page without navigating. Uses a cached version if the DOM hasn't changed since the last snapshot."
    )]
    async fn snapshot(&self) -> String {
        match self.do_snapshot().await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Snapshot failed: {e}"),
        }
    }

    #[tool(
        description = "Click an element by ref ID (the N from @eN in the snapshot). Returns updated page snapshot, or a compact diff if return_diff is true."
    )]
    async fn click(&self, Parameters(params): Parameters<ClickParams>) -> String {
        match self
            .do_click(params.r#ref, params.return_diff.unwrap_or(false))
            .await
        {
            Ok(text) => text,
            Err(e) => format!("ERROR: Click @e{} failed: {e}", params.r#ref),
        }
    }

    #[tool(
        description = "Type text into an input field by ref ID (the N from @eN). Returns updated page snapshot, or a compact diff if return_diff is true."
    )]
    async fn type_text(&self, Parameters(params): Parameters<TypeTextParams>) -> String {
        match self
            .do_type_text(
                params.r#ref,
                &params.text,
                params.return_diff.unwrap_or(false),
            )
            .await
        {
            Ok(text) => text,
            Err(e) => format!("ERROR: Type into @e{} failed: {e}", params.r#ref),
        }
    }

    #[tool(
        description = "Select an option in a dropdown by ref ID (the N from @eN). Returns updated page snapshot, or a compact diff if return_diff is true."
    )]
    async fn select_option(&self, Parameters(params): Parameters<SelectOptionParams>) -> String {
        match self
            .do_select(
                params.r#ref,
                &params.value,
                params.return_diff.unwrap_or(false),
            )
            .await
        {
            Ok(text) => text,
            Err(e) => format!("ERROR: Select in @e{} failed: {e}", params.r#ref),
        }
    }

    #[tool(
        description = "Wait for the page DOM to change (e.g., after an async update or SPA transition), then return a fresh snapshot. Useful when a previous action triggers deferred updates."
    )]
    async fn wait_for_changes(
        &self,
        Parameters(params): Parameters<WaitForChangesParams>,
    ) -> String {
        match self
            .do_wait_for_changes(params.timeout_ms.unwrap_or(5000))
            .await
        {
            Ok(text) => text,
            Err(e) => format!("ERROR: Wait failed: {e}"),
        }
    }

    #[tool(
        description = "Set task context to focus subsequent snapshots on relevant page regions. Reduces snapshot size by filtering out content unrelated to the current task. The context persists until cleared."
    )]
    async fn set_task_context(
        &self,
        Parameters(params): Parameters<SetTaskContextParams>,
    ) -> String {
        match self.do_set_task_context(params).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: {e}"),
        }
    }

    #[tool(
        description = "Clear the current task context. Subsequent snapshots will show the full unfiltered page."
    )]
    async fn clear_task_context(&self) -> String {
        let mut state = self.state.write().await;
        if let Ok(tab) = state.active_tab_mut() {
            tab.task_context = None;
            tab.cached_snapshot = None;
        }
        "Task context cleared. Snapshots will now show the full page.".into()
    }

    #[tool(
        description = "Get a one-time focused snapshot filtered by the given criteria, without changing the persistent task context. Useful for quick targeted queries like 'show me only the form elements'."
    )]
    async fn focused_snapshot(
        &self,
        Parameters(params): Parameters<FocusedSnapshotParams>,
    ) -> String {
        match self.do_focused_snapshot(params).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Focused snapshot failed: {e}"),
        }
    }

    #[tool(
        description = "Open a new tab and navigate to the given URL. Returns the new tab's ID and page snapshot."
    )]
    async fn open_tab(&self, Parameters(params): Parameters<OpenTabParams>) -> String {
        match self.do_open_tab(&params.url).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Open tab failed: {e}"),
        }
    }

    #[tool(
        description = "List all open tabs with their IDs, titles, and URLs. The active tab is marked."
    )]
    async fn list_tabs(&self) -> String {
        match self.do_list_tabs().await {
            Ok(text) => text,
            Err(e) => format!("ERROR: List tabs failed: {e}"),
        }
    }

    #[tool(description = "Switch to a different tab by ID. Returns that tab's current snapshot.")]
    async fn switch_tab(&self, Parameters(params): Parameters<SwitchTabParams>) -> String {
        match self.do_switch_tab(params.tab_id).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Switch tab failed: {e}"),
        }
    }

    #[tool(
        description = "Close a tab by ID. If the closed tab was active, switches to another open tab."
    )]
    async fn close_tab(&self, Parameters(params): Parameters<CloseTabParams>) -> String {
        match self.do_close_tab(params.tab_id).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Close tab failed: {e}"),
        }
    }

    #[tool(
        description = "Scroll down by roughly one viewport height. Returns an updated snapshot with viewport position."
    )]
    async fn scroll_down(&self) -> String {
        match self.do_scroll(mutation::SCROLL_DOWN_JS).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Scroll down failed: {e}"),
        }
    }

    #[tool(
        description = "Scroll up by roughly one viewport height. Returns an updated snapshot with viewport position."
    )]
    async fn scroll_up(&self) -> String {
        match self.do_scroll(mutation::SCROLL_UP_JS).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Scroll up failed: {e}"),
        }
    }

    #[tool(
        description = "Scroll a specific element into view by ref ID (the N from @eN). Returns an updated snapshot."
    )]
    async fn scroll_to_ref(&self, Parameters(params): Parameters<ScrollToRefParams>) -> String {
        match self.do_scroll_to_ref(params.r#ref).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Scroll to @e{} failed: {e}", params.r#ref),
        }
    }

    #[tool(
        description = "Compare the current page to its previous snapshot and return a compact diff showing what changed. Useful after actions to see only what's different without a full snapshot."
    )]
    async fn page_diff(&self) -> String {
        match self.do_page_diff().await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Diff failed: {e}"),
        }
    }

    #[tool(
        description = "Extract structured data from the current page as JSON, using a JSON Schema to describe the desired output shape. Deterministic extraction from the semantic tree - no LLM needed. Supports table extraction (maps column headers to schema properties), list extraction (repeated items), and single-object extraction (labeled values). Use 'selector' to scope to a page region (e.g., \"table\", \"[role=list]\")."
    )]
    async fn extract(&self, Parameters(params): Parameters<ExtractParams>) -> String {
        match self.do_extract(params).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Extract failed: {e}"),
        }
    }

    #[tool(
        description = "Start recording browser actions for the current domain. Actions (navigate, click, type, select) will be captured until stop_recording is called. Only one recording can be active at a time."
    )]
    async fn start_recording(
        &self,
        Parameters(params): Parameters<StartRecordingParams>,
    ) -> String {
        match self.do_start_recording(params).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Start recording failed: {e}"),
        }
    }

    #[tool(
        description = "Stop the active recording, save it to disk, and return a summary. The recording can later be replayed with replay_recording."
    )]
    async fn stop_recording(&self) -> String {
        match self.do_stop_recording().await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Stop recording failed: {e}"),
        }
    }

    #[tool(
        description = "Replay a saved recording deterministically. Each action is re-executed using stored element locators - no LLM needed. Aborts on first element not found."
    )]
    async fn replay_recording(
        &self,
        Parameters(params): Parameters<ReplayRecordingParams>,
    ) -> String {
        match self.do_replay_recording(params).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Replay failed: {e}"),
        }
    }

    #[tool(description = "List saved recordings, optionally filtered by domain.")]
    async fn list_recordings(
        &self,
        Parameters(params): Parameters<ListRecordingsParams>,
    ) -> String {
        match self.do_list_recordings(params).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: List recordings failed: {e}"),
        }
    }

    #[tool(description = "Delete a saved recording by name.")]
    async fn delete_recording(
        &self,
        Parameters(params): Parameters<DeleteRecordingParams>,
    ) -> String {
        match self.do_delete_recording(params).await {
            Ok(text) => text,
            Err(e) => format!("ERROR: Delete recording failed: {e}"),
        }
    }

    #[tool(
        description = "Take a PNG screenshot of the current page. Returns a base64-encoded image. Use full_page to capture the entire scrollable page. Use annotate to overlay interactive elements with red borders and @eN labels for visual debugging."
    )]
    async fn screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match self.do_screenshot(params).await {
            Ok(result) => Ok(result),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "ERROR: Screenshot failed: {e}"
            ))])),
        }
    }

}

#[tool_handler]
impl ServerHandler for CortexBrowserServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "cortex-browser: compact browser perception layer for AI agents. \
                 Use 'navigate' to open a URL and get a page snapshot. Interactive elements \
                 are labeled @eN. Use 'click', 'type_text', 'select_option' with the ref \
                 number N to interact. Use 'snapshot' to refresh the current view. \
                 Use 'set_task_context' to focus snapshots on what matters for your current task. \
                 Use 'wait_for_changes' after actions that trigger async page updates. \
                 Use 'focused_snapshot' for one-time filtered views. \
                 Use 'open_tab', 'list_tabs', 'switch_tab', 'close_tab' for multi-tab workflows. \
                 Use 'scroll_down', 'scroll_up', 'scroll_to_ref' to navigate within long pages. \
                 Elements marked [offscreen] are outside the current viewport. \
                 Use 'page_diff' to see what changed since the last snapshot, or pass return_diff:true to click/type_text/select_option. \
                 Use 'extract' with a JSON Schema to pull structured data (tables, lists, objects) from the page as JSON. \
                 Use 'start_recording' / 'stop_recording' to capture action sequences, then 'replay_recording' to replay them deterministically without LLM decisions. \
                 Use 'list_recordings' and 'delete_recording' to manage saved recordings. \
                 Use 'screenshot' to capture a PNG of the current page, with optional full_page and annotate flags for visual debugging."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Annotate nodes with offscreen status based on visibility data from the browser.
fn annotate_viewport(nodes: &mut [crate::dom::SemanticNode], visibility: &HashMap<u32, bool>) {
    for node in nodes {
        if node.ref_id > 0 {
            if let Some(&visible) = visibility.get(&node.ref_id) {
                node.offscreen = Some(!visible);
            }
        }
        annotate_viewport(&mut node.children, visibility);
    }
}

/// Parse the viewport JSON from GET_VIEWPORT_JS.
fn parse_viewport_json(json: &str) -> Option<crate::dom::ViewportInfo> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(rename = "scrollY")]
        scroll_y: u32,
        #[serde(rename = "viewportHeight")]
        viewport_height: u32,
        #[serde(rename = "documentHeight")]
        document_height: u32,
    }
    serde_json::from_str::<Raw>(json)
        .ok()
        .map(|r| crate::dom::ViewportInfo {
            scroll_y: r.scroll_y,
            viewport_height: r.viewport_height,
            document_height: r.document_height,
        })
}

/// Apply task context filtering to a snapshot if a context is set.
fn apply_task_context(
    task_context: &Option<hints::TaskContext>,
    snapshot: &crate::dom::PageSnapshot,
) -> String {
    match task_context {
        Some(ctx) => {
            let filtered = ctx.filter_snapshot(snapshot);
            serialize::to_compact_text(&filtered)
        }
        None => serialize::to_compact_text(snapshot),
    }
}

/// Build a TaskContext from MCP focus parameters.
fn build_task_context(
    task: String,
    focus_text: Vec<String>,
    focus_roles: Vec<String>,
    interactive_only: bool,
) -> hints::TaskContext {
    let parsed_roles = focus_roles
        .iter()
        .filter_map(|s| hints::parse_role(s))
        .collect();
    hints::TaskContext {
        task,
        focus_text,
        focus_roles: parsed_roles,
        interactive_only,
    }
}

fn build_annotation_js(ref_index: &RefIndex, full_page: bool) -> String {
    let position = if full_page { "absolute" } else { "fixed" };
    let scroll_offset = if full_page {
        "window.scrollX, window.scrollY"
    } else {
        "0, 0"
    };

    let mut entries = String::new();
    for (ref_id, locator) in ref_index {
        entries.push_str(&format!(
            "{{ ref_id: {ref_id}, find: function() {{ return {find}; }} }},\n",
            find = locator.to_js_expression()
        ));
    }

    format!(
        r#"(function() {{
    var container = document.createElement('div');
    container.id = '__cortex_annotations';
    container.style.cssText = 'pointer-events:none; position:{position}; top:0; left:0; width:0; height:0; z-index:2147483647;';
    var entries = [{entries}];
    var offsets = [{scroll_offset}];
    entries.forEach(function(e) {{
        var el = e.find();
        if (!el) return;
        var r = el.getBoundingClientRect();
        if (r.width === 0 && r.height === 0) return;
        var overlay = document.createElement('div');
        overlay.style.cssText = 'position:{position}; border:2px solid red; pointer-events:none;'
            + 'left:' + (r.left + offsets[0]) + 'px;'
            + 'top:' + (r.top + offsets[1]) + 'px;'
            + 'width:' + r.width + 'px;'
            + 'height:' + r.height + 'px;';
        var label = document.createElement('span');
        label.textContent = '@e' + e.ref_id;
        label.style.cssText = 'position:absolute; top:-16px; left:0; background:red; color:white; font:bold 11px monospace; padding:1px 3px; white-space:nowrap;';
        overlay.appendChild(label);
        container.appendChild(overlay);
    }});
    document.body.appendChild(container);
}})()"#,
    )
}

const REMOVE_ANNOTATIONS_JS: &str =
    "(function() { var el = document.getElementById('__cortex_annotations'); if (el) el.remove(); })()";

impl CortexBrowserServer {
    async fn ensure_browser(&self) -> anyhow::Result<()> {
        let mut state = self.state.write().await;
        if state.browser.is_some() {
            return Ok(());
        }

        info!(
            launch = self.launch_browser,
            port = self.port,
            "initializing browser connection"
        );
        let b = if self.launch_browser {
            browser::launch().await?
        } else {
            browser::connect(self.port).await?
        };

        state.browser = Some(b);
        info!("browser ready");
        Ok(())
    }

    async fn do_navigate(&self, url: &str) -> anyhow::Result<String> {
        info!(url = %url, "navigate");
        self.ensure_browser().await?;

        let mut state = self.state.write().await;
        let browser = state.browser.as_ref().context("No browser")?;

        let page = browser
            .new_page(url)
            .await
            .with_context(|| format!("Failed to navigate to {url}"))?;

        page.wait_for_navigation().await.ok();

        let html = page.content().await.context("Failed to get page content")?;
        let final_url = page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| url.to_string());

        page.evaluate(mutation::INSTALL_OBSERVER_JS).await.ok();

        let viewport_json = page
            .evaluate(mutation::GET_VIEWPORT_JS)
            .await
            .ok()
            .and_then(|v| v.into_value::<String>().ok())
            .unwrap_or_default();
        let viewport = parse_viewport_json(&viewport_json);

        let mut result = pipeline::process_with_refs(&html, &final_url);
        result.snapshot.viewport = viewport;

        let ref_exprs: Vec<(u32, String)> = result
            .ref_index
            .iter()
            .map(|(id, loc)| (*id, loc.to_js_expression()))
            .collect();
        if !ref_exprs.is_empty() {
            let vis_js = mutation::build_check_visibility_js(&ref_exprs);
            let vis_json = page
                .evaluate(vis_js.as_str())
                .await
                .ok()
                .and_then(|v| v.into_value::<String>().ok())
                .unwrap_or_default();
            if let Ok(vis_map) = serde_json::from_str::<HashMap<String, bool>>(&vis_json) {
                let vis: HashMap<u32, bool> = vis_map
                    .into_iter()
                    .filter_map(|(k, v)| k.parse::<u32>().ok().map(|id| (id, v)))
                    .collect();
                annotate_viewport(&mut result.snapshot.nodes, &vis);
            }
        }

        state.record(recording::RecordedAction::Navigate {
            url: url.to_string(),
        });

        // page is moved into state below - no more CDP calls
        if state.tabs.is_empty() {
            let tab_id = state.next_tab_id;
            state.next_tab_id += 1;
            let text = apply_task_context(&None, &result.snapshot);
            info!(tab_id = tab_id, url = %final_url, refs = result.ref_index.len(), "created initial tab");
            state.tabs.insert(
                tab_id,
                TabState {
                    page,
                    ref_index: result.ref_index,
                    current_url: final_url,
                    cached_snapshot: Some(text.clone()),
                    observer_installed: true,
                    task_context: None,
                    previous_snapshot: None,
                },
            );
            state.active_tab = tab_id;
            Ok(text)
        } else {
            let tab = state.active_tab_mut()?;
            let text = apply_task_context(&tab.task_context, &result.snapshot);
            tab.previous_snapshot = None;
            tab.page = page;
            tab.ref_index = result.ref_index;
            tab.current_url = final_url.clone();
            tab.cached_snapshot = Some(text.clone());
            tab.observer_installed = true;
            info!(tab_id = state.active_tab, url = %final_url, "navigated active tab");
            Ok(text)
        }
    }

    async fn do_snapshot(&self) -> anyhow::Result<String> {
        debug!("snapshot requested");
        let mut state = self.state.write().await;
        let tab = state.active_tab()?;

        if tab.observer_installed {
            let dirty_json = tab
                .page
                .evaluate(mutation::CHECK_DIRTY_JS)
                .await
                .ok()
                .and_then(|v| v.into_value::<String>().ok())
                .unwrap_or_default();
            let dirty_state = mutation::DirtyState::from_json(&dirty_json);

            if !dirty_state.dirty {
                if let Some(cached) = &tab.cached_snapshot {
                    debug!("returning cached snapshot (DOM unchanged)");
                    return Ok(cached.clone());
                }
            }
            debug!(
                mutations = dirty_state.mutation_count,
                "DOM dirty, re-snapshotting"
            );
        }

        let tab = state.active_tab()?;
        let html = tab
            .page
            .content()
            .await
            .context("Failed to get page content")?;
        let url = tab
            .page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| tab.current_url.clone());

        let tab = state.active_tab()?;
        tab.page.evaluate(mutation::RESET_DIRTY_JS).await.ok();
        if !tab.observer_installed {
            tab.page.evaluate(mutation::INSTALL_OBSERVER_JS).await.ok();
        }

        let tab = state.active_tab()?;
        let viewport_json = tab
            .page
            .evaluate(mutation::GET_VIEWPORT_JS)
            .await
            .ok()
            .and_then(|v| v.into_value::<String>().ok())
            .unwrap_or_default();
        let viewport = parse_viewport_json(&viewport_json);

        let mut result = pipeline::process_with_refs(&html, &url);
        result.snapshot.viewport = viewport;

        let tab = state.active_tab()?;
        let ref_exprs: Vec<(u32, String)> = result
            .ref_index
            .iter()
            .map(|(id, loc)| (*id, loc.to_js_expression()))
            .collect();
        if !ref_exprs.is_empty() {
            let vis_js = mutation::build_check_visibility_js(&ref_exprs);
            let vis_json = tab
                .page
                .evaluate(vis_js.as_str())
                .await
                .ok()
                .and_then(|v| v.into_value::<String>().ok())
                .unwrap_or_default();
            if let Ok(vis_map) = serde_json::from_str::<HashMap<String, bool>>(&vis_json) {
                let vis: HashMap<u32, bool> = vis_map
                    .into_iter()
                    .filter_map(|(k, v)| k.parse::<u32>().ok().map(|id| (id, v)))
                    .collect();
                annotate_viewport(&mut result.snapshot.nodes, &vis);
            }
        }

        let tab = state.active_tab_mut()?;
        let text = apply_task_context(&tab.task_context, &result.snapshot);
        tab.previous_snapshot = Some(result.snapshot);
        tab.ref_index = result.ref_index;
        tab.current_url = url;
        tab.cached_snapshot = Some(text.clone());
        tab.observer_installed = true;

        Ok(text)
    }

    async fn do_click(&self, ref_id: u32, return_diff: bool) -> anyhow::Result<String> {
        info!(ref_id = ref_id, return_diff = return_diff, "click");
        let (js, captured_locator) = {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            let locator = tab
                .ref_index
                .get(&ref_id)
                .with_context(|| format!("Unknown ref @e{ref_id}"))?;
            (locator.click_js(), locator.clone())
        };
        let result = self.execute_and_snapshot(&js, ref_id, return_diff).await?;
        {
            let mut state = self.state.write().await;
            state.record(recording::RecordedAction::Click {
                locator: captured_locator,
                ref_id,
            });
        }
        Ok(result)
    }

    async fn do_type_text(
        &self,
        ref_id: u32,
        text: &str,
        return_diff: bool,
    ) -> anyhow::Result<String> {
        info!(ref_id = ref_id, text = %text, return_diff = return_diff, "type_text");
        let (js, captured_locator) = {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            let locator = tab
                .ref_index
                .get(&ref_id)
                .with_context(|| format!("Unknown ref @e{ref_id}"))?;
            (locator.type_js(text), locator.clone())
        };
        let result = self.execute_and_snapshot(&js, ref_id, return_diff).await?;
        {
            let mut state = self.state.write().await;
            state.record(recording::RecordedAction::TypeText {
                locator: captured_locator,
                text: text.to_string(),
                ref_id,
            });
        }
        Ok(result)
    }

    async fn do_select(
        &self,
        ref_id: u32,
        value: &str,
        return_diff: bool,
    ) -> anyhow::Result<String> {
        info!(ref_id = ref_id, value = %value, return_diff = return_diff, "select_option");
        let (js, captured_locator) = {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            let locator = tab
                .ref_index
                .get(&ref_id)
                .with_context(|| format!("Unknown ref @e{ref_id}"))?;
            (locator.select_js(value), locator.clone())
        };
        let result = self.execute_and_snapshot(&js, ref_id, return_diff).await?;
        {
            let mut state = self.state.write().await;
            state.record(recording::RecordedAction::SelectOption {
                locator: captured_locator,
                value: value.to_string(),
                ref_id,
            });
        }
        Ok(result)
    }

    async fn execute_and_snapshot(
        &self,
        js: &str,
        ref_id: u32,
        return_diff: bool,
    ) -> anyhow::Result<String> {
        let prev_snapshot = if return_diff {
            let state = self.state.read().await;
            state
                .active_tab()
                .ok()
                .and_then(|t| t.previous_snapshot.clone())
        } else {
            None
        };

        let result_value = {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            let eval = tab
                .page
                .evaluate(js)
                .await
                .context("Failed to execute action")?;
            eval.into_value::<String>().unwrap_or_default()
        };

        if result_value == "NOT_FOUND" {
            warn!(ref_id = ref_id, "element not found in live DOM");
            anyhow::bail!("Element @e{ref_id} not found in the live DOM");
        }

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let full_snapshot = self.do_snapshot().await?;

        if return_diff {
            if let Some(old) = prev_snapshot {
                let state = self.state.read().await;
                if let Ok(tab) = state.active_tab() {
                    if let Some(new) = &tab.previous_snapshot {
                        let diff_result = diff::diff_snapshots(&old, new);
                        return Ok(diff::format_diff(&diff_result));
                    }
                }
            }
            Ok(full_snapshot)
        } else {
            Ok(full_snapshot)
        }
    }

    async fn do_wait_for_changes(&self, timeout_ms: u64) -> anyhow::Result<String> {
        debug!(timeout_ms = timeout_ms, "waiting for DOM changes");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let poll_interval = std::time::Duration::from_millis(100);

        loop {
            {
                let state = self.state.read().await;
                let tab = state.active_tab()?;
                let dirty_json = tab
                    .page
                    .evaluate(mutation::CHECK_DIRTY_JS)
                    .await
                    .ok()
                    .and_then(|v| v.into_value::<String>().ok())
                    .unwrap_or_default();
                let dirty = mutation::DirtyState::from_json(&dirty_json);
                if dirty.dirty {
                    debug!(mutations = dirty.mutation_count, "DOM changes detected");
                    break;
                }
            }

            if tokio::time::Instant::now() >= deadline {
                debug!(timeout_ms = timeout_ms, "wait timed out with no changes");
                let state = self.state.read().await;
                let tab = state.active_tab()?;
                return Ok(tab
                    .cached_snapshot
                    .clone()
                    .unwrap_or_else(|| "(no snapshot available)".into()));
            }

            tokio::time::sleep(poll_interval).await;
        }

        self.do_snapshot().await
    }

    async fn do_set_task_context(&self, params: SetTaskContextParams) -> anyhow::Result<String> {
        let mut msg = format!("Task context set: \"{}\"", params.task);
        if !params.focus_text.is_empty() {
            msg.push_str(&format!("\nFocus text: {}", params.focus_text.join(", ")));
        }
        if !params.focus_roles.is_empty() {
            msg.push_str(&format!("\nFocus roles: {}", params.focus_roles.join(", ")));
        }
        if params.interactive_only {
            msg.push_str("\nMode: interactive elements only");
        }
        msg.push_str("\nSubsequent snapshots will be filtered accordingly.");

        let ctx = build_task_context(
            params.task,
            params.focus_text,
            params.focus_roles,
            params.interactive_only,
        );

        let mut state = self.state.write().await;
        let tab = state.active_tab_mut()?;
        tab.task_context = Some(ctx);
        tab.cached_snapshot = None;

        Ok(msg)
    }

    async fn do_focused_snapshot(&self, params: FocusedSnapshotParams) -> anyhow::Result<String> {
        let ctx = build_task_context(
            String::new(),
            params.focus_text,
            params.focus_roles,
            params.interactive_only,
        );

        let mut state = self.state.write().await;
        let tab = state.active_tab()?;

        let html = tab
            .page
            .content()
            .await
            .context("Failed to get page content")?;
        let url = tab
            .page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| tab.current_url.clone());

        let result = pipeline::process_with_refs(&html, &url);
        let filtered = ctx.filter_snapshot(&result.snapshot);
        let text = serialize::to_compact_text(&filtered);

        let tab = state.active_tab_mut()?;
        tab.ref_index = result.ref_index;
        tab.current_url = url;

        Ok(text)
    }

    async fn do_open_tab(&self, url: &str) -> anyhow::Result<String> {
        info!(url = %url, "open_tab");
        self.ensure_browser().await?;

        let mut state = self.state.write().await;
        let browser = state.browser.as_ref().context("No browser")?;

        let page = browser
            .new_page(url)
            .await
            .with_context(|| format!("Failed to open tab for {url}"))?;

        page.wait_for_navigation().await.ok();

        let html = page.content().await.context("Failed to get page content")?;
        let final_url = page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| url.to_string());

        page.evaluate(mutation::INSTALL_OBSERVER_JS).await.ok();

        let result = pipeline::process_with_refs(&html, &final_url);
        let text = serialize::to_compact_text(&result.snapshot);

        let tab_id = state.next_tab_id;
        state.next_tab_id += 1;

        state.tabs.insert(
            tab_id,
            TabState {
                page,
                ref_index: result.ref_index,
                current_url: final_url,
                cached_snapshot: Some(text.clone()),
                observer_installed: true,
                task_context: None,
                previous_snapshot: None,
            },
        );
        state.active_tab = tab_id;

        info!(tab_id = tab_id, "tab opened");
        Ok(format!("Tab {tab_id} opened.\n{text}"))
    }

    async fn do_list_tabs(&self) -> anyhow::Result<String> {
        let state = self.state.read().await;
        if state.tabs.is_empty() {
            return Ok("No tabs open.".into());
        }

        let mut lines = Vec::new();
        let mut tab_ids: Vec<u32> = state.tabs.keys().copied().collect();
        tab_ids.sort();

        for id in tab_ids {
            let tab = &state.tabs[&id];
            let active = if id == state.active_tab {
                " [active]"
            } else {
                ""
            };
            lines.push(format!("Tab {id}{active}: [{}]", tab.current_url));
        }

        Ok(lines.join("\n"))
    }

    async fn do_switch_tab(&self, tab_id: u32) -> anyhow::Result<String> {
        info!(tab_id = tab_id, "switch_tab");
        let mut state = self.state.write().await;
        if !state.tabs.contains_key(&tab_id) {
            warn!(tab_id = tab_id, "tab not found");
            anyhow::bail!("No tab with ID {tab_id}");
        }
        state.active_tab = tab_id;
        drop(state);

        self.do_snapshot().await
    }

    async fn do_close_tab(&self, tab_id: u32) -> anyhow::Result<String> {
        info!(tab_id = tab_id, "close_tab");
        let mut state = self.state.write().await;
        let tab = state
            .tabs
            .remove(&tab_id)
            .with_context(|| format!("No tab with ID {tab_id}"))?;

        tab.page.close().await.ok();

        if state.active_tab == tab_id {
            state.active_tab = state.tabs.keys().copied().min().unwrap_or(0);
        }

        if state.tabs.is_empty() {
            Ok(format!("Tab {tab_id} closed. No tabs remaining."))
        } else {
            Ok(format!(
                "Tab {tab_id} closed. Active tab: {}",
                state.active_tab
            ))
        }
    }

    async fn do_scroll(&self, scroll_js: &str) -> anyhow::Result<String> {
        {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            tab.page.evaluate(scroll_js).await.ok();
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        {
            let mut state = self.state.write().await;
            let tab = state.active_tab_mut()?;
            tab.cached_snapshot = None;
        }
        self.do_snapshot().await
    }

    async fn do_page_diff(&self) -> anyhow::Result<String> {
        let old_snapshot = {
            let state = self.state.read().await;
            state.active_tab()?.previous_snapshot.clone()
        };

        match old_snapshot {
            Some(old) => {
                self.do_snapshot().await?;
                let state = self.state.read().await;
                let tab = state.active_tab()?;
                match &tab.previous_snapshot {
                    Some(current) => {
                        let diff_result = diff::diff_snapshots(&old, current);
                        Ok(diff::format_diff(&diff_result))
                    }
                    None => Ok("no snapshot to compare".into()),
                }
            }
            None => Ok("no previous snapshot to compare - take a snapshot first".into()),
        }
    }

    async fn do_extract(&self, params: ExtractParams) -> anyhow::Result<String> {
        debug!("extract requested");
        let state = self.state.read().await;
        let tab = state.active_tab()?;

        let html = tab
            .page
            .content()
            .await
            .context("Failed to get page content")?;
        let url = tab
            .page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| tab.current_url.clone());

        let snapshot = pipeline::process(&html, &url);

        let result =
            extract::extract_with_schema(&snapshot, &params.schema, params.selector.as_deref());

        Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| "null".into()))
    }

    async fn do_start_recording(&self, params: StartRecordingParams) -> anyhow::Result<String> {
        let mut state = self.state.write().await;
        if state.active_recording.is_some() {
            anyhow::bail!("A recording is already in progress. Stop it first.");
        }

        let (domain, start_url) = if let Ok(tab) = state.active_tab() {
            (
                recording::extract_domain(&tab.current_url),
                tab.current_url.clone(),
            )
        } else {
            ("unknown".into(), String::new())
        };

        state.active_recording = Some(recording::Recording {
            name: params.name.clone(),
            domain: domain.clone(),
            start_url,
            created_at: recording::now_timestamp(),
            description: params.description.clone(),
            actions: Vec::new(),
        });

        info!(name = %params.name, domain = %domain, "recording started");
        Ok(format!(
            "Recording '{}' started for domain '{}'. Actions will be captured until stop_recording is called.",
            params.name, domain
        ))
    }

    async fn do_stop_recording(&self) -> anyhow::Result<String> {
        let rec = {
            let mut state = self.state.write().await;
            state
                .active_recording
                .take()
                .ok_or_else(|| anyhow::anyhow!("No active recording to stop"))?
        };

        let action_count = rec.actions.len();
        let path = self.store.save(&rec)?;

        info!(name = %rec.name, actions = action_count, path = %path.display(), "recording saved");
        Ok(format!(
            "Recording '{}' saved with {} action(s) to {}",
            rec.name,
            action_count,
            path.display()
        ))
    }

    async fn do_replay_recording(&self, params: ReplayRecordingParams) -> anyhow::Result<String> {
        let rec = self.store.load(&params.name, params.domain.as_deref())?;
        info!(name = %rec.name, actions = rec.actions.len(), "replaying recording");

        {
            let mut state = self.state.write().await;
            state.replaying = true;
        }

        let replay_result = self.do_replay_actions(&rec).await;

        {
            let mut state = self.state.write().await;
            state.replaying = false;
        }

        replay_result
    }

    async fn do_replay_actions(&self, rec: &recording::Recording) -> anyhow::Result<String> {
        let mut step_results = Vec::new();

        for (i, action) in rec.actions.iter().enumerate() {
            let step_num = i + 1;
            match action {
                recording::RecordedAction::Navigate { url } => {
                    self.do_navigate(url).await?;
                    step_results.push(format!("Step {}: navigate → {}", step_num, url));
                }
                recording::RecordedAction::Click { locator, .. } => {
                    let js = locator.click_js();
                    self.execute_replay_step(&js, step_num).await?;
                    step_results.push(format!("Step {}: click", step_num));
                }
                recording::RecordedAction::TypeText { locator, text, .. } => {
                    let js = locator.type_js(text);
                    self.execute_replay_step(&js, step_num).await?;
                    step_results.push(format!("Step {}: type_text", step_num));
                }
                recording::RecordedAction::SelectOption { locator, value, .. } => {
                    let js = locator.select_js(value);
                    self.execute_replay_step(&js, step_num).await?;
                    step_results.push(format!("Step {}: select_option", step_num));
                }
            }
        }

        let final_snapshot = self.do_snapshot().await?;
        let summary = format!(
            "Replay '{}' completed ({} steps):\n{}\n\n{}",
            rec.name,
            rec.actions.len(),
            step_results.join("\n"),
            final_snapshot,
        );
        Ok(summary)
    }

    /// Execute a JS step during replay, checking for NOT_FOUND.
    async fn execute_replay_step(&self, js: &str, step_num: usize) -> anyhow::Result<()> {
        let result_value = {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            let eval = tab
                .page
                .evaluate(js)
                .await
                .context("Failed to execute replay action")?;
            eval.into_value::<String>().unwrap_or_default()
        };

        if result_value == "NOT_FOUND" {
            anyhow::bail!(
                "Replay step {}: element not found in the live DOM",
                step_num
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        Ok(())
    }

    async fn do_list_recordings(&self, params: ListRecordingsParams) -> anyhow::Result<String> {
        let summaries = self.store.list(params.domain.as_deref())?;
        if summaries.is_empty() {
            return Ok("No recordings found.".into());
        }

        let lines: Vec<String> = summaries
            .iter()
            .map(|s| {
                let desc = s
                    .description
                    .as_deref()
                    .map(|d| format!(" - {d}"))
                    .unwrap_or_default();
                format!(
                    "  {} [{}] ({} actions){}",
                    s.name, s.domain, s.action_count, desc
                )
            })
            .collect();

        Ok(format!("Recordings:\n{}", lines.join("\n")))
    }

    async fn do_delete_recording(&self, params: DeleteRecordingParams) -> anyhow::Result<String> {
        self.store.delete(&params.name, params.domain.as_deref())?;
        Ok(format!("Recording '{}' deleted.", params.name))
    }

    async fn do_scroll_to_ref(&self, ref_id: u32) -> anyhow::Result<String> {
        debug!(ref_id = ref_id, "scroll_to_ref");
        let js = {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            let locator = tab
                .ref_index
                .get(&ref_id)
                .with_context(|| format!("Unknown ref @e{ref_id}"))?;
            format!(
                "(function() {{ var el = {}; if (!el) return 'NOT_FOUND'; el.scrollIntoView({{behavior: 'instant', block: 'center'}}); return 'OK'; }})()",
                locator.to_js_expression()
            )
        };

        let result_value = {
            let state = self.state.read().await;
            let tab = state.active_tab()?;
            let eval = tab
                .page
                .evaluate(js.as_str())
                .await
                .context("Failed to scroll")?;
            eval.into_value::<String>().unwrap_or_default()
        };

        if result_value == "NOT_FOUND" {
            anyhow::bail!("Element @e{ref_id} not found in the live DOM");
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        {
            let mut state = self.state.write().await;
            let tab = state.active_tab_mut()?;
            tab.cached_snapshot = None;
        }
        self.do_snapshot().await
    }

    async fn do_screenshot(&self, params: ScreenshotParams) -> anyhow::Result<CallToolResult> {
        let full_page = params.full_page.unwrap_or(false);
        let annotate = params.annotate.unwrap_or(false);
        info!(full_page = full_page, annotate = annotate, "screenshot");

        self.ensure_browser().await?;

        let state = self.state.read().await;
        let tab = state.active_tab()?;

        if annotate {
            let annotation_js = build_annotation_js(&tab.ref_index, full_page);
            tab.page.evaluate(annotation_js).await.ok();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let screenshot_params = chromiumoxide::page::ScreenshotParams::builder()
            .full_page(full_page)
            .build();

        let bytes = tab
            .page
            .screenshot(screenshot_params)
            .await
            .context("Failed to capture screenshot")?;

        if annotate {
            tab.page.evaluate(REMOVE_ANNOTATIONS_JS).await.ok();
        }

        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let size_kb = bytes.len() / 1024;
        let meta = format!(
            "Screenshot captured ({size_kb} KB, {})",
            if full_page { "full page" } else { "viewport" }
        );

        Ok(CallToolResult::success(vec![
            Content::text(meta),
            Content::image(encoded, "image/png"),
        ]))
    }

}

pub async fn run_mcp_server(launch: bool, port: u16) -> anyhow::Result<()> {
    info!(
        launch = launch,
        port = port,
        "starting MCP server over stdio"
    );
    let server = CortexBrowserServer::new(launch, port);

    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .context("Failed to start MCP server")?;

    info!("MCP server running, waiting for requests");
    service.waiting().await?;
    info!("MCP server shut down");
    Ok(())
}

pub async fn run_mcp_http_server(
    launch: bool,
    port: u16,
    host: &str,
    http_port: u16,
) -> anyhow::Result<()> {
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };
    use tokio_util::sync::CancellationToken;

    let ct = CancellationToken::new();

    let service: StreamableHttpService<CortexBrowserServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(CortexBrowserServer::new(launch, port)),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig {
                cancellation_token: ct.child_token(),
                ..Default::default()
            },
        );

    let router = axum::Router::new().nest_service("/mcp", service);

    let bind_addr = format!("{host}:{http_port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind to {bind_addr}"))?;

    info!(addr = %bind_addr, "MCP HTTP server listening on http://{bind_addr}/mcp");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move { ct.cancelled().await })
        .await
        .context("HTTP server error")?;

    info!("MCP HTTP server shut down");
    Ok(())
}
