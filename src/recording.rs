use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::dom::ElementLocator;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RecordedAction {
    Navigate {
        url: String,
    },
    Click {
        locator: ElementLocator,
        ref_id: u32,
    },
    TypeText {
        locator: ElementLocator,
        text: String,
        ref_id: u32,
    },
    SelectOption {
        locator: ElementLocator,
        value: String,
        ref_id: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub name: String,
    pub domain: String,
    pub start_url: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub actions: Vec<RecordedAction>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordingSummary {
    pub name: String,
    pub domain: String,
    pub description: Option<String>,
    pub action_count: usize,
    pub created_at: String,
}

impl From<&Recording> for RecordingSummary {
    fn from(rec: &Recording) -> Self {
        Self {
            name: rec.name.clone(),
            domain: rec.domain.clone(),
            description: rec.description.clone(),
            action_count: rec.actions.len(),
            created_at: rec.created_at.clone(),
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

/// Extract the domain from a URL, replacing dots/colons with hyphens.
/// e.g. "https://github.com/foo" → "github-com"
pub fn extract_domain(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host = host.split(':').next().unwrap_or(host);
    host.replace('.', "-")
}

/// Sanitize a name for use as a filename (letters, digits, hyphens, underscores).
pub fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "recording".into()
    } else {
        trimmed.to_string()
    }
}

/// Base directory for all recordings: `~/.cortex-browser/recordings`
pub fn recordings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cortex-browser")
        .join("recordings")
}

// ── RecordingStore ──────────────────────────────────────────────────────────

pub struct RecordingStore {
    base: PathBuf,
}

impl Default for RecordingStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingStore {
    pub fn new() -> Self {
        Self {
            base: recordings_dir(),
        }
    }

    pub fn with_base(base: PathBuf) -> Self {
        Self { base }
    }

    /// Save a recording to disk. Returns the path written.
    pub fn save(&self, rec: &Recording) -> anyhow::Result<PathBuf> {
        let dir = self.base.join(&rec.domain);
        fs::create_dir_all(&dir)?;
        let filename = format!("{}.json", sanitize_filename(&rec.name));
        let path = dir.join(&filename);
        let json = serde_json::to_string_pretty(rec)?;
        fs::write(&path, json)?;
        Ok(path)
    }

    /// Load a recording by name. If `domain` is None, search all domain dirs.
    pub fn load(&self, name: &str, domain: Option<&str>) -> anyhow::Result<Recording> {
        let filename = format!("{}.json", sanitize_filename(name));

        if let Some(d) = domain {
            let path = self.base.join(d).join(&filename);
            let json = fs::read_to_string(&path)
                .map_err(|_| anyhow::anyhow!("Recording '{}' not found in domain '{}'", name, d))?;
            let rec: Recording = serde_json::from_str(&json)?;
            return Ok(rec);
        }

        // Search all domain directories
        if self.base.exists() {
            for entry in fs::read_dir(&self.base)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let path = entry.path().join(&filename);
                    if path.exists() {
                        let json = fs::read_to_string(&path)?;
                        let rec: Recording = serde_json::from_str(&json)?;
                        return Ok(rec);
                    }
                }
            }
        }

        anyhow::bail!("Recording '{}' not found", name)
    }

    /// List all recordings, optionally filtered by domain.
    pub fn list(&self, domain: Option<&str>) -> anyhow::Result<Vec<RecordingSummary>> {
        let mut summaries = Vec::new();

        if !self.base.exists() {
            return Ok(summaries);
        }

        let dirs: Vec<PathBuf> = if let Some(d) = domain {
            let p = self.base.join(d);
            if p.exists() {
                vec![p]
            } else {
                vec![]
            }
        } else {
            fs::read_dir(&self.base)?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect()
        };

        for dir in dirs {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Ok(json) = fs::read_to_string(&path) {
                        if let Ok(rec) = serde_json::from_str::<Recording>(&json) {
                            summaries.push(RecordingSummary::from(&rec));
                        }
                    }
                }
            }
        }

        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(summaries)
    }

    /// Delete a recording by name. If `domain` is None, search all domain dirs.
    pub fn delete(&self, name: &str, domain: Option<&str>) -> anyhow::Result<()> {
        let filename = format!("{}.json", sanitize_filename(name));

        if let Some(d) = domain {
            let path = self.base.join(d).join(&filename);
            if path.exists() {
                fs::remove_file(&path)?;
                return Ok(());
            }
            anyhow::bail!("Recording '{}' not found in domain '{}'", name, d);
        }

        // Search all domain directories
        if self.base.exists() {
            for entry in fs::read_dir(&self.base)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let path = entry.path().join(&filename);
                    if path.exists() {
                        fs::remove_file(&path)?;
                        return Ok(());
                    }
                }
            }
        }

        anyhow::bail!("Recording '{}' not found", name)
    }
}
