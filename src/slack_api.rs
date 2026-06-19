//! Slack Web API via the local desktop session — no app / admin approval.
//!
//! Confirmed recipe (captured from the real Slack desktop traffic):
//!   POST https://<workspace>.slack.com/api/<method>
//!   Cookie: d=<workspace `d` cookie value>      (d alone is enough)
//!   token=<xoxc-... workspace token>             (form field)
//!
//! Both the `xoxc` token and the `xoxd` `d` cookie are read live from Slack's
//! process memory (the cookie file is exclusively locked while Slack runs).

use crate::mem;
use anyhow::{bail, Result};
use std::path::PathBuf;

pub struct SlackClient {
    pub host: String, // e.g. https://vegibushq.slack.com
    token: String,
    cookie: String, // `d` cookie value
    http: reqwest::blocking::Client,
}

impl SlackClient {
    fn new(host: String, token: String, cookie: String) -> Self {
        Self {
            host,
            token,
            cookie,
            http: reqwest::blocking::Client::new(),
        }
    }

    pub fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<serde_json::Value> {
        let mut form: Vec<(&str, &str)> = vec![("token", self.token.as_str())];
        form.extend_from_slice(params);
        let resp = self
            .http
            .post(format!("{}/api/{}", self.host, method))
            .header("Cookie", format!("d={}", self.cookie))
            .header("Origin", "https://app.slack.com")
            .header("User-Agent", "Mozilla/5.0")
            .form(&form)
            .send()?
            .json::<serde_json::Value>()?;
        Ok(resp)
    }

    pub fn auth_test(&self) -> Result<serde_json::Value> {
        self.call("auth.test", &[])
    }

    pub fn ok(&self) -> bool {
        self.auth_test()
            .ok()
            .and_then(|r| r["ok"].as_bool())
            .unwrap_or(false)
    }

    pub fn conversations_history(&self, channel: &str, limit: u32) -> Result<serde_json::Value> {
        let l = limit.to_string();
        self.call("conversations.history", &[("channel", channel), ("limit", &l)])
    }

    pub fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut params = vec![("channel", channel), ("text", text)];
        if let Some(t) = thread_ts {
            params.push(("thread_ts", t));
        }
        self.call("chat.postMessage", &params)
    }

    /// Connect to `<subdomain>.slack.com`: use cached creds if still valid for THIS
    /// workspace, otherwise discover the (token, cookie) pair whose auth.test resolves
    /// to this workspace (auth.test identity is decided by the token, not the host).
    pub fn connect(subdomain: &str) -> Result<Self> {
        let host = format!("https://{subdomain}.slack.com");

        if let Some((t, c)) = load_cache(subdomain) {
            let sc = Self::new(host.clone(), t, c);
            if let Ok(j) = sc.auth_test() {
                if team_matches(&j, subdomain) {
                    return Ok(sc);
                }
            }
        }

        let (tokens, cookies) = mem::scan_slack();
        let toks: Vec<&String> = tokens.iter().filter(|t| t.len() >= 80).collect();
        let cks: Vec<&String> = cookies.iter().filter(|c| c.len() >= 120).collect();
        let http = reqwest::blocking::Client::new();
        for t in &toks {
            for c in &cks {
                let j = http
                    .post(format!("{host}/api/auth.test"))
                    .header("Cookie", format!("d={c}"))
                    .header("Origin", "https://app.slack.com")
                    .header("User-Agent", "Mozilla/5.0")
                    .form(&[("token", t.as_str())])
                    .send()
                    .ok()
                    .and_then(|r| r.json::<serde_json::Value>().ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                if team_matches(&j, subdomain) {
                    save_cache(subdomain, t, c);
                    return Ok(Self::new(host, (*t).clone(), (*c).clone()));
                }
            }
        }
        bail!(
            "no working Slack creds for '{subdomain}' (tokens={}, cookies={}). Is Slack running and logged in to that workspace?",
            toks.len(),
            cks.len()
        )
    }
}

/// True if auth.test succeeded AND resolved to `<subdomain>.slack.com`.
fn team_matches(j: &serde_json::Value, subdomain: &str) -> bool {
    j["ok"].as_bool() == Some(true)
        && j["url"]
            .as_str()
            .map_or(false, |u| u.contains(&format!("{subdomain}.slack.com")))
}

fn cache_path() -> PathBuf {
    let dir = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(dir).join("tagami").join("slack_creds.json")
}

fn load_cache(subdomain: &str) -> Option<(String, String)> {
    let txt = std::fs::read_to_string(cache_path()).ok()?;
    let j: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let o = j.get(subdomain)?;
    Some((
        o["token"].as_str()?.to_string(),
        o["cookie"].as_str()?.to_string(),
    ))
}

fn save_cache(subdomain: &str, token: &str, cookie: &str) {
    let path = cache_path();
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let mut j: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    j[subdomain] = serde_json::json!({ "token": token, "cookie": cookie });
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&j).unwrap_or_default());
}
