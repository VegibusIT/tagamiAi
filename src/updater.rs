//! Self-update from GitHub Releases — same scheme as cart-converter (public repo,
//! no token). Release flow: bump `version` in Cargo.toml, push a `vX.Y.Z` tag → CI
//! (.github/workflows/build-windows.yml) builds `tagami.exe` and attaches it to the
//! GitHub release. `tagami update` fetches the latest release and, if newer,
//! downloads the exe, swaps it in, and relaunches.

use anyhow::{anyhow, bail, Result};

/// owner/repo on GitHub (public). Change here when the repo differs.
const GITHUB_REPO: &str = "VegibusIT/tagamiAi";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const ASSET: &str = "tagami.exe";

pub fn current_version() -> &'static str {
    CURRENT_VERSION
}

/// CLI `tagami update`: check, update if newer, relaunch the GUI.
pub fn check_and_update() -> Result<()> {
    if !update_if_newer(&[], true)? {
        println!("最新版です（v{CURRENT_VERSION}）。");
    }
    Ok(())
}

/// Check the latest GitHub release; if newer, download + swap this exe + relaunch it with
/// `relaunch_args` and exit (so this call does not return on success). Returns Ok(false) when
/// already up to date. `verbose` controls progress printing (off for the silent auto-check).
pub fn update_if_newer(relaunch_args: &[&str], verbose: bool) -> Result<bool> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let rel: serde_json::Value = client
        .get(&url)
        .header("User-Agent", "tagami-updater")
        .header("Accept", "application/vnd.github+json")
        .send()?
        .json()?;

    let tag = rel["tag_name"].as_str().unwrap_or_default();
    if tag.is_empty() {
        bail!("GitHubにリリースが見つかりません（{GITHUB_REPO}）。先に v タグでリリースしてください。");
    }
    let current = format!("v{CURRENT_VERSION}");
    if tag == current {
        return Ok(false);
    }

    let download_url = rel["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a["name"].as_str() == Some(ASSET))
                .and_then(|a| a["browser_download_url"].as_str())
        })
        .ok_or_else(|| anyhow!("リリース {tag} に {ASSET} が見つかりません"))?
        .to_string();

    if verbose {
        println!("新バージョン {tag} を取得します（現在 {current}）…");
    }
    let bytes = client
        .get(&download_url)
        .header("User-Agent", "tagami-updater")
        .send()?
        .bytes()?;

    // Windows lets us rename a running exe: write new -> rename current to .old -> put new in place.
    let current_exe = std::env::current_exe()?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow!("exe の親ディレクトリが見つかりません"))?;
    let tmp = parent.join("tagami.new.exe");
    let old = parent.join("tagami.old.exe");
    std::fs::write(&tmp, &bytes)?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&current_exe, &old)?;
    std::fs::rename(&tmp, &current_exe)?;

    if verbose {
        println!("更新完了：{tag}。新バージョンで再起動します。");
    }
    let _ = std::process::Command::new(&current_exe)
        .args(relaunch_args)
        .spawn();
    std::process::exit(0);
}
