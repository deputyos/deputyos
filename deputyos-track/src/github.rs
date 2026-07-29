//! GitHub Releases API client.
//!
//! Sync HTTP via `ureq`. No tokio. We deliberately keep this thin — the
//! tracker only needs `tag_name`, `published_at`, `prerelease`, `name`,
//! `body`, `html_url` from each release.

use anyhow::{Context, Result};
use serde::Deserialize;

/// Subset of the GitHub Releases API response we care about.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

/// Channel selector matching `[profile].release_channel`.
#[derive(Debug, Clone, Copy)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    /// Parse `[profile].release_channel` into the API selector.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "stable" => Ok(Channel::Stable),
            "beta" => Ok(Channel::Beta),
            other => anyhow::bail!("unknown release channel {other:?}"),
        }
    }
}

/// HTTP client wrapper. The `base_url` knob exists so tests can point at
/// a local stub server.
pub struct Client {
    pub base_url: String,
    pub token: Option<String>,
    pub agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            base_url: "https://api.github.com".into(),
            token: std::env::var("GITHUB_TOKEN").ok().filter(|s| !s.is_empty()),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(20))
                .build(),
        }
    }
}

impl Client {
    /// Construct a client pointed at a custom base URL (for tests).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            token: None,
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(5))
                .build(),
        }
    }

    /// Fetch the latest release matching `channel`. For stable, this is
    /// `releases/latest`. For beta, we fetch the recent releases list and
    /// pick the first non-draft entry (which is the most recent including
    /// pre-releases, per the API ordering).
    pub fn latest(&self, repo: &str, channel: Channel) -> Result<Option<Release>> {
        match channel {
            Channel::Stable => self.latest_stable(repo),
            Channel::Beta => self.latest_beta(repo),
        }
    }

    fn latest_stable(&self, repo: &str) -> Result<Option<Release>> {
        let url = format!("{}/repos/{}/releases/latest", self.base_url, repo);
        match self.get(&url) {
            Ok(text) => {
                let r: Release = serde_json::from_str(&text)
                    .with_context(|| format!("parse releases/latest for {repo}"))?;
                if r.draft {
                    Ok(None)
                } else {
                    Ok(Some(r))
                }
            }
            Err(e) => {
                if let Some(404) = status_of(&e) {
                    // Repo exists but has no published releases yet, or
                    // only pre-releases. Both are "no upgrade available"
                    // for the stable channel.
                    return Ok(None);
                }
                Err(e)
            }
        }
    }

    fn latest_beta(&self, repo: &str) -> Result<Option<Release>> {
        let url = format!("{}/repos/{}/releases?per_page=20", self.base_url, repo);
        let text = self.get(&url)?;
        let list: Vec<Release> =
            serde_json::from_str(&text).with_context(|| format!("parse releases for {repo}"))?;
        Ok(list.into_iter().find(|r| !r.draft))
    }

    fn get(&self, url: &str) -> Result<String> {
        let mut req = self
            .agent
            .get(url)
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("User-Agent", "deputyos-track/0.1");
        if let Some(tok) = &self.token {
            req = req.set("Authorization", &format!("Bearer {tok}"));
        }
        let resp = req.call().map_err(anyhow::Error::from)?;
        resp.into_string()
            .map_err(|e| anyhow::anyhow!("read body: {e}"))
    }
}

/// Extract a status code from an `anyhow::Error` wrapping a `ureq::Error`.
fn status_of(err: &anyhow::Error) -> Option<u16> {
    err.downcast_ref::<ureq::Error>().and_then(|e| match e {
        ureq::Error::Status(code, _) => Some(*code),
        ureq::Error::Transport(_) => None,
    })
}
