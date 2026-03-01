use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::recording::{extract_domain, now_timestamp, sanitize_filename};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<f64>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfile {
    pub profile: String,
    pub domain: String,
    pub url: String,
    pub saved_at: String,
    pub cookies: Vec<StoredCookie>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthSummary {
    pub profile: String,
    pub domain: String,
    pub cookie_count: usize,
    pub saved_at: String,
}

impl From<&AuthProfile> for AuthSummary {
    fn from(p: &AuthProfile) -> Self {
        Self {
            profile: p.profile.clone(),
            domain: p.domain.clone(),
            cookie_count: p.cookies.len(),
            saved_at: p.saved_at.clone(),
        }
    }
}

fn auth_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cortex-browser")
        .join("auth")
}

pub struct AuthStore {
    base: PathBuf,
}

impl Default for AuthStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthStore {
    pub fn new() -> Self {
        Self { base: auth_dir() }
    }

    pub fn with_base(base: PathBuf) -> Self {
        Self { base }
    }

    pub fn save(
        &self,
        url: &str,
        profile_name: &str,
        cookies: Vec<StoredCookie>,
    ) -> anyhow::Result<PathBuf> {
        let domain = extract_domain(url);
        let dir = self.base.join(&domain);
        fs::create_dir_all(&dir)?;

        let profile = AuthProfile {
            profile: profile_name.to_string(),
            domain: domain.clone(),
            url: url.to_string(),
            saved_at: now_timestamp(),
            cookies,
        };

        let filename = format!("{}.json", sanitize_filename(profile_name));
        let path = dir.join(&filename);
        let json = serde_json::to_string_pretty(&profile)?;
        fs::write(&path, json)?;
        Ok(path)
    }

    pub fn load(&self, profile_name: &str, domain: Option<&str>) -> anyhow::Result<AuthProfile> {
        let filename = format!("{}.json", sanitize_filename(profile_name));

        if let Some(d) = domain {
            let path = self.base.join(d).join(&filename);
            let json = fs::read_to_string(&path).map_err(|_| {
                anyhow::anyhow!(
                    "Auth profile '{}' not found in domain '{}'",
                    profile_name,
                    d
                )
            })?;
            let profile: AuthProfile = serde_json::from_str(&json)?;
            return Ok(profile);
        }

        if self.base.exists() {
            for entry in fs::read_dir(&self.base)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let path = entry.path().join(&filename);
                    if path.exists() {
                        let json = fs::read_to_string(&path)?;
                        let profile: AuthProfile = serde_json::from_str(&json)?;
                        return Ok(profile);
                    }
                }
            }
        }

        anyhow::bail!("Auth profile '{}' not found", profile_name)
    }

    pub fn list(&self, domain: Option<&str>) -> anyhow::Result<Vec<AuthSummary>> {
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
                        if let Ok(profile) = serde_json::from_str::<AuthProfile>(&json) {
                            summaries.push(AuthSummary::from(&profile));
                        }
                    }
                }
            }
        }

        summaries.sort_by(|a, b| a.profile.cmp(&b.profile));
        Ok(summaries)
    }

    pub fn delete(&self, profile_name: &str, domain: Option<&str>) -> anyhow::Result<()> {
        let filename = format!("{}.json", sanitize_filename(profile_name));

        if let Some(d) = domain {
            let path = self.base.join(d).join(&filename);
            if path.exists() {
                fs::remove_file(&path)?;
                return Ok(());
            }
            anyhow::bail!(
                "Auth profile '{}' not found in domain '{}'",
                profile_name,
                d
            );
        }

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

        anyhow::bail!("Auth profile '{}' not found", profile_name)
    }
}
