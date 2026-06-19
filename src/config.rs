//! Settings, stored at %APPDATA%\tagami\config.toml so they persist wherever the
//! exe runs. Editable from the GUI (Config::save). Each value can also be overridden
//! by an environment variable.

use std::path::PathBuf;

pub struct Config {
    /// Whose voice the AI replies in (e.g. "安河内", "田上").
    pub persona: String,
    /// Slack workspace subdomain for the API host, e.g. "vegibushq" -> vegibushq.slack.com.
    pub slack_subdomain: String,
    /// Knowledge base file (facts/schedule/contacts/style) injected into the prompt.
    pub knowledge_path: String,
}

pub fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("tagami").join("config.toml")
}

impl Config {
    pub fn load() -> Config {
        let txt = std::fs::read_to_string(config_path()).unwrap_or_default();
        Config {
            persona: resolve(&txt, "TAGAMI_PERSONA", "persona", "安河内"),
            slack_subdomain: resolve(&txt, "TAGAMI_SLACK_SUBDOMAIN", "slack_subdomain", "vegibushq"),
            knowledge_path: resolve(
                &txt,
                "TAGAMI_KNOWLEDGE",
                "knowledge_path",
                "G:\\マイドライブ\\tagamiAi\\knowledge.md",
            ),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        // Paths are written raw (single backslashes) and read back raw — our parser
        // does no unescaping, so this round-trips on Windows.
        let body = format!(
            "# AI田上 設定（GUIから編集可）\npersona = \"{}\"\nslack_subdomain = \"{}\"\nknowledge_path = \"{}\"\n",
            self.persona, self.slack_subdomain, self.knowledge_path
        );
        std::fs::write(path, body)
    }
}

fn resolve(file_txt: &str, env_key: &str, file_key: &str, default: &str) -> String {
    if let Ok(v) = std::env::var(env_key) {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    if let Some(v) = parse_value(file_txt, file_key) {
        if !v.is_empty() {
            return v;
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
