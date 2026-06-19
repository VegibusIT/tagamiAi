//! Minimal settings, no external deps. Resolution order for each value:
//!   1. environment variable (e.g. TAGAMI_PERSONA)
//!   2. `tagami.toml` next to the working dir  (key = "value")
//!   3. built-in default

pub struct Config {
    /// Whose voice the AI replies in (e.g. "安河内", "田上").
    pub persona: String,
    /// Slack workspace subdomain for the API host, e.g. "vegibushq" -> vegibushq.slack.com.
    pub slack_subdomain: String,
    /// Knowledge base file (facts/schedule/contacts/style) injected into the prompt.
    /// Lives on Google Drive so it can be edited from anywhere.
    pub knowledge_path: String,
}

impl Config {
    pub fn load() -> Config {
        Config {
            persona: resolve("TAGAMI_PERSONA", "persona", "安河内"),
            slack_subdomain: resolve("TAGAMI_SLACK_SUBDOMAIN", "slack_subdomain", "vegibushq"),
            knowledge_path: resolve(
                "TAGAMI_KNOWLEDGE",
                "knowledge_path",
                "G:\\マイドライブ\\tagamiAi\\knowledge.md",
            ),
        }
    }
}

fn resolve(env_key: &str, file_key: &str, default: &str) -> String {
    if let Ok(v) = std::env::var(env_key) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    for path in ["tagami.toml", "config.toml"] {
        if let Ok(txt) = std::fs::read_to_string(path) {
            if let Some(v) = parse_value(&txt, file_key) {
                if !v.is_empty() {
                    return v;
                }
            }
        }
    }
    default.to_string()
}

/// Parse a very small subset of TOML: top-level `key = "value"` (or `key = value`).
fn parse_value(txt: &str, key: &str) -> Option<String> {
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim().trim_matches('"').trim().to_string());
            }
        }
    }
    None
}
