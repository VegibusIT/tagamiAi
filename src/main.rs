//! AI田上 PoC — drive Slack and Copilot for Windows via UI Automation.
//!
//! Safe, read-oriented commands first:
//!   tagami slack-read           # dump latest Slack message texts
//!   tagami copilot-read         # wake Copilot, show conversation + locate input box
//!   tagami copilot-type "text"  # type text into Copilot's input box (does NOT submit)

mod config;
mod mem;
mod slack_api;
mod uia;
mod updater;
mod win32;

use anyhow::{anyhow, bail, Result};
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::IUIAutomationElement;

use uia::{
    control_type, control_type_name, current_value, has_value_pattern, name, set_focus, Uia,
    CT_BUTTON, CT_EDIT, CT_TEXT,
};
use win32::*;

fn truncate(s: &str, max_chars: usize) -> String {
    let cleaned: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() > max_chars {
        cleaned.chars().take(max_chars).collect()
    } else {
        cleaned
    }
}

/// Return the richest UIA subtree (main window or a render-host child) WITHOUT re-waking.
fn richest_subtree(uia: &Uia, hwnd: HWND) -> Result<Vec<IUIAutomationElement>> {
    let mut targets = vec![hwnd];
    targets.extend(render_host_children(hwnd));

    let mut best: Vec<IUIAutomationElement> = Vec::new();
    for t in targets {
        if let Ok(root) = uia.element_from_hwnd(t) {
            if let Ok(v) = uia.subtree(&root) {
                if v.len() > best.len() {
                    best = v;
                }
            }
        }
    }
    Ok(best)
}

/// Wake the app's accessibility tree, then return its richest UIA subtree.
fn read_app(uia: &Uia, hwnd: HWND) -> Result<Vec<IUIAutomationElement>> {
    wake_accessibility(hwnd);
    sleep(Duration::from_millis(2200));
    richest_subtree(uia, hwnd)
}

/// Extract Copilot's latest reply: the Text after the last "Copilot の発言" marker,
/// excluding labels, the input placeholder and the echoed prompt; de-duplicated.
fn extract_response(els: &[IUIAutomationElement], prompt: &str) -> String {
    // Start just after the most recent "Copilot の発言" (Copilot's turn) marker.
    let mut begin = 0usize;
    for (i, el) in els.iter().enumerate() {
        if name(el).trim() == "Copilot の発言" {
            begin = i + 1;
        }
    }
    let mut parts: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for el in &els[begin..] {
        let n = name(el);
        let t = n.trim();
        if t.is_empty() || control_type(el) != CT_TEXT {
            continue;
        }
        if t == "Copilot へメッセージを送る"
            || t.contains("Copilot のエクスペリエンス")
            || t.contains("フィードバックがあれば")
            || t.contains("Copilot をご利用")
        {
            break; // reached Copilot's trailing UI/footer — stop
        }
        if t == "あなたの発言" || t == "Copilot の発言" || t == prompt {
            continue;
        }
        if seen.insert(t.to_string()) {
            parts.push(t.to_string());
        }
    }
    parts.join("")
}

fn find_input_box<'a>(els: &'a [IUIAutomationElement]) -> Option<&'a IUIAutomationElement> {
    els.iter()
        .find(|el| control_type(el) == CT_EDIT && has_value_pattern(el))
}

/// Find Copilot's input box, re-waking and retrying (the WebView2 a11y tree can
/// take a couple of tries to expose after the window is brought forward).
fn find_copilot_input(uia: &Uia, hwnd: HWND) -> Option<IUIAutomationElement> {
    for _ in 0..4 {
        if let Ok(els) = read_app(uia, hwnd) {
            if let Some(el) = find_input_box(&els) {
                return Some(el.clone());
            }
        }
        sleep(Duration::from_millis(1200));
    }
    None
}

fn slack_read(uia: &Uia) -> Result<()> {
    let hwnd = find_visible_window_by_title("Slack").ok_or_else(|| anyhow!("Slack window not found (is the desktop app open?)"))?;
    println!("Slack: '{}'", window_text(hwnd));
    let els = read_app(uia, hwnd)?;
    println!("subtree: {} elements", els.len());
    println!("--- message-like text (name length > 15) ---");
    let mut shown = 0;
    for el in &els {
        let n = name(el);
        if n.trim().chars().count() > 15 {
            println!("[{}] {}", control_type_name(control_type(el)), truncate(&n, 90));
            shown += 1;
            if shown >= 30 {
                break;
            }
        }
    }
    Ok(())
}

fn copilot_read(uia: &Uia) -> Result<()> {
    let hwnd = find_visible_window_by_title("Copilot").ok_or_else(|| anyhow!("Copilot window not found (open the Copilot app)"))?;
    restore_and_foreground(hwnd);
    let els = read_app(uia, hwnd)?;
    println!("Copilot subtree: {} elements", els.len());

    match find_input_box(&els) {
        Some(el) => println!("input box: FOUND -> [Edit] '{}'", name(el)),
        None => println!("input box: not found"),
    }

    println!("--- text (name length > 8) ---");
    let mut shown = 0;
    for el in &els {
        let n = name(el);
        if n.trim().chars().count() > 8 {
            println!("[{}] {}", control_type_name(control_type(el)), truncate(&n, 75));
            shown += 1;
            if shown >= 25 {
                break;
            }
        }
    }
    Ok(())
}

fn copilot_type(uia: &Uia, text: &str) -> Result<()> {
    let hwnd = find_visible_window_by_title("Copilot").ok_or_else(|| anyhow!("Copilot window not found"))?;
    restore_and_foreground(hwnd);
    let els = read_app(uia, hwnd)?;
    let input = find_input_box(&els).ok_or_else(|| anyhow!("Copilot input box not found"))?;
    set_focus(input);
    sleep(Duration::from_millis(300));
    select_all();
    sleep(Duration::from_millis(150));
    type_unicode(text);
    sleep(Duration::from_millis(500));
    let after = read_app(uia, hwnd)?;
    let visible = after.iter().any(|el| name(el).contains(text));
    println!("typed (no submit): {:?}", text);
    println!("appears in UI tree: {}  (ValuePattern read-back: {:?})", visible, current_value(input));
    Ok(())
}

/// Send a prompt to Copilot and return its reply text.
const COPILOT_APP_ID: &str = "Microsoft.Copilot_8wekyb3d8bbwe!App";

/// Find Copilot's window; if it's only resident in the tray (no window), launch it and wait.
fn ensure_copilot_window() -> Option<HWND> {
    if let Some(h) = find_visible_window_by_title("Copilot") {
        return Some(h);
    }
    let _ = std::process::Command::new("explorer")
        .arg(format!("shell:AppsFolder\\{COPILOT_APP_ID}"))
        .spawn();
    for _ in 0..24 {
        sleep(Duration::from_millis(500));
        if let Some(h) = find_visible_window_by_title("Copilot") {
            return Some(h);
        }
    }
    None
}

fn copilot_send_and_read(uia: &Uia, prompt: &str) -> Result<String> {
    let hwnd = ensure_copilot_window().ok_or_else(|| {
        anyhow!("Copilotを開けませんでした。Copilot for Windows を起動してから再試行してください。")
    })?;
    // Remember the window the user is working in so we can hand focus back the instant the
    // prompt is submitted.
    let user_fg = get_foreground_window();
    // Copilot is a Chromium/WebView2 app: it only accepts input and *streams its reply* while
    // genuinely visible on screen (off-screen or covered, generation pauses). It does NOT,
    // however, need to remain the FOREGROUND window. So we dock it into a corner, bring it
    // forward just long enough to type, then hand focus back. For typing we keep it wide enough
    // (>=840px) that the "新しいチャット" button stays on screen; afterwards we shrink it.
    let (ax, ay, aw, ah) = work_area();
    let in_w = 920.min(aw);
    let in_h = 800.min(ah);
    place_window(hwnd, ax + aw - in_w, ay + ah - in_h, in_w, in_h);
    set_foreground(hwnd);
    sleep(Duration::from_millis(350));

    // Start a fresh chat so the previous turn's answer can't be mistaken for ours.
    if let Ok(els) = read_app(uia, hwnd) {
        if let Some(btn) = els
            .iter()
            .find(|e| control_type(e) == CT_BUTTON && name(e).contains("新しいチャット"))
        {
            let _ = uia::invoke(btn);
            sleep(Duration::from_millis(1200));
        }
    }

    let input = find_copilot_input(uia, hwnd)
        .ok_or_else(|| anyhow!("Copilot input box not found (open the Copilot chat window)"))?;
    // Collapse newlines/tabs to spaces — a typed Enter would submit the prompt early.
    let clean: String = prompt
        .chars()
        .map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c })
        .collect();
    // Click then SetFocus so the WebView2 input reliably has the editing caret for the paste.
    if let Some((l, t, r, b)) = uia::bounding_rect(&input) {
        click((l + r) / 2, (t + b) / 2);
        sleep(Duration::from_millis(250));
    }
    set_focus(&input);
    sleep(Duration::from_millis(350));
    select_all();
    sleep(Duration::from_millis(150));
    // Paste long prompts via the clipboard — per-keystroke typing drops characters on long
    // input and the message never submits.
    if set_clipboard_text(&clean) {
        paste();
    } else {
        type_unicode(&clean);
    }
    sleep(Duration::from_millis(700));
    // Submit by invoking Copilot's Send button — more reliable than a synthesized Enter on this
    // WebView2 input. The button is labelled "メッセージの送信" only once the composer holds
    // text; fall back to Enter if it isn't found.
    let submitted = richest_subtree(uia, hwnd)
        .ok()
        .and_then(|all| {
            all.iter()
                .find(|e| control_type(e) == CT_BUTTON && name(e).contains("メッセージの送信"))
                .map(|b| uia::invoke(b).is_ok())
        })
        .unwrap_or(false);
    if !submitted {
        press_enter();
    }

    // The prompt is in. Wait until Copilot accepts it (the composer clears), then shrink it to a
    // small bottom-right corner — still visible, so it keeps streaming the reply (read via UIA),
    // but out of the way — and hand focus straight back to the user. They get their active
    // window back in ~1s with only a small corner window left, instead of a full-screen Copilot
    // stealing focus for the whole reply.
    for _ in 0..24 {
        sleep(Duration::from_millis(150));
        if current_value(&input).trim().is_empty() {
            break;
        }
    }
    let sm_w = 600.min(aw);
    let sm_h = 700.min(ah);
    place_window(hwnd, ax + aw - sm_w, ay + ah - sm_h, sm_w, sm_h);
    if !user_fg.is_invalid() && user_fg != hwnd {
        set_foreground(user_fg);
    }

    // Copilot streams its answer; poll until it stabilises (first token can take seconds).
    let mut last = String::new();
    let mut stable = 0;
    for _ in 0..16 {
        sleep(Duration::from_millis(2000));
        let after = richest_subtree(uia, hwnd)?;
        let resp = extract_response(&after, &clean);
        if !resp.is_empty() && resp == last {
            stable += 1;
            if stable >= 2 {
                break;
            }
        } else {
            stable = 0;
        }
        last = resp;
        eprintln!("  ...copilot {} chars", last.chars().count());
    }
    Ok(last)
}

fn copilot_ask(uia: &Uia, prompt: &str) -> Result<()> {
    let resp = copilot_send_and_read(uia, prompt)?;
    println!("=== Copilot response ===");
    println!("{}", if resp.is_empty() { "(no response captured)" } else { &resp });
    Ok(())
}

/// Latest message bubble in the currently open Slack conversation.
fn latest_slack_message(els: &[IUIAutomationElement]) -> Option<String> {
    els.iter()
        .rev()
        .find(|el| {
            control_type(el) == uia::CT_LISTITEM && name(el).trim().chars().count() > 6
        })
        .map(|el| name(el).trim().to_string())
}

/// Slack's message composer input (an Edit with ValuePattern, not the search box).
fn slack_input<'a>(els: &'a [IUIAutomationElement]) -> Option<&'a IUIAutomationElement> {
    els.iter().find(|el| {
        control_type(el) == CT_EDIT
            && has_value_pattern(el)
            && name(el).contains("メッセージ")
            && !name(el).contains("検索")
    })
}

/// Read latest Slack message -> draft a reply via Copilot (as 田上) -> type it into
/// Slack's composer. Sends only when `send` is true; otherwise leaves it as a draft.
struct Prepared {
    sc: slack_api::SlackClient,
    channel: String,
    thread_ts: String,
    incoming: String,
    judgment: String,
    draft: String, // empty when no reply is needed
}

/// A message worth replying to: a DM, or a channel message that @mentions us.
struct IncomingTarget {
    channel: String,
    text: String,
    thread_ts: String,
    ts: f64,
    mentioned: bool,
    is_dm: bool,
}

/// Scan the workspace for the most recent message that is actually directed at us — a DM/group
/// DM, or a channel message containing our `<@USERID>` mention. Plain channel chatter is ignored
/// (we don't want to reply to everything), which is what made real mentions get buried before.
fn find_latest_target(sc: &slack_api::SlackClient, me: &str) -> Result<Option<IncomingTarget>> {
    let conv = sc.call(
        "users.conversations",
        &[
            ("types", "public_channel,private_channel,im,mpim"),
            ("limit", "200"),
            ("exclude_archived", "true"),
        ],
    )?;
    let empty = Vec::new();
    let channels = conv["channels"].as_array().unwrap_or(&empty);
    let mention_tag = format!("<@{me}"); // matches both <@U123> and <@U123|name>
    let mut best: Option<IncomingTarget> = None;
    for ch in channels.iter().take(50) {
        let id = match ch["id"].as_str() {
            Some(s) => s,
            None => continue,
        };
        let is_dm =
            ch["is_im"].as_bool().unwrap_or(false) || ch["is_mpim"].as_bool().unwrap_or(false);
        let h = match sc.conversations_history(id, 6) {
            Ok(h) => h,
            Err(_) => continue,
        };
        if let Some(msgs) = h["messages"].as_array() {
            for m in msgs {
                let u = m["user"].as_str().unwrap_or("");
                let text = m["text"].as_str().unwrap_or("");
                let ts_str = m["ts"].as_str().unwrap_or("");
                let ts: f64 = ts_str.parse().unwrap_or(0.0);
                if u == me || u.is_empty() || text.trim().is_empty() {
                    continue;
                }
                let mentioned = text.contains(&mention_tag);
                // Only DMs, or channel messages that mention us, count as something to reply to.
                if !is_dm && !mentioned {
                    continue;
                }
                if best.as_ref().map_or(true, |b| ts > b.ts) {
                    let thread_ts = m["thread_ts"].as_str().unwrap_or(ts_str).to_string();
                    best = Some(IncomingTarget {
                        channel: id.to_string(),
                        text: text.to_string(),
                        thread_ts,
                        ts,
                        mentioned,
                        is_dm,
                    });
                }
            }
        }
    }
    Ok(best)
}

/// Connect, find the most recent incoming message, triage it, and (if needed)
/// draft a reply as `persona`. Heavy work (Copilot) happens here, before any UI.
fn prepare_reply(uia: &Uia, persona: &str, subdomain: &str, knowledge_path: &str) -> Result<Prepared> {
    let sc = slack_api::SlackClient::connect(subdomain)?;
    let auth = sc.auth_test()?;
    let me = auth["user_id"].as_str().unwrap_or_default().to_string();
    println!(
        "[Slack] {} / {}",
        auth["team"].as_str().unwrap_or("?"),
        auth["user"].as_str().unwrap_or("?")
    );

    let target = find_latest_target(&sc, &me)?
        .ok_or_else(|| anyhow!("返信対象（DM・または自分宛メンション）が見つかりませんでした"))?;
    let channel = target.channel.clone();
    let thread_ts = target.thread_ts.clone();
    let kind = if target.mentioned {
        "メンション"
    } else if target.is_dm {
        "DM"
    } else {
        "メッセージ"
    };
    println!(
        "[最新の受信] ch={} 種別={} : {}",
        channel,
        kind,
        truncate(&target.text, 140)
    );
    let msg = truncate(&target.text, 500);
    // Tell Copilot when the message is explicitly aimed at the user, so triage doesn't dismiss
    // a direct ping as "どうでもいい内容".
    let direct_note = if target.mentioned {
        "（これはあなた（本人）宛の@メンションです。基本的に何らかの返信をしてください）"
    } else if target.is_dm {
        "（これはあなた宛のダイレクトメッセージです）"
    } else {
        ""
    };

    // Knowledge base (Drive) — facts the AI may rely on; stops it inventing schedules.
    let knowledge = std::fs::read_to_string(knowledge_path).unwrap_or_default();
    let knowledge_block = if knowledge.trim().is_empty() {
        String::new()
    } else {
        format!(
            "【{persona}に関する前提情報（これに反する推測はしない。ここに無い事実は断定しない）】\n{}\n\n",
            knowledge.trim()
        )
    };

    // Single combined call (judgment + draft) to halve Copilot usage (free quota).
    let prompt = format!(
        "{knowledge_block}あなたは『{persona}』本人です。次のSlackメッセージへの対応を決めてください。{direct_note}\
         返信すべきでない（挨拶のみ・雑談・自動通知・どうでもいい内容）なら、出力は『返信不要』だけにしてください。\
         返信すべきなら、1行目に『返信必要』と書き、2行目以降に返信本文だけを書いてください\
         （前置き・解説・引用符なし。予定・可否・事実など自分が確実に知らないことは推測で断定せず、『確認して折り返します』等の確認形に）。\n\nメッセージ:\n{msg}"
    );
    let resp = copilot_send_and_read(uia, &prompt)?;

    let no_reply =
        resp.trim_start().starts_with("返信不要") || (resp.contains("返信不要") && !resp.contains("返信必要"));
    let (judgment, draft) = if no_reply {
        ("返信不要".to_string(), String::new())
    } else {
        let body = resp.splitn(2, "返信必要").nth(1).unwrap_or(resp.as_str());
        let body = body
            .trim_start_matches(|c: char| {
                matches!(c, '：' | ':' | '。' | '、' | ' ' | '　' | '\n' | '\r' | '\t' | '-')
            })
            .trim()
            .to_string();
        ("返信必要".to_string(), body)
    };
    println!("[判断] {} / [下書き] {}", judgment, truncate(&draft, 80));

    Ok(Prepared {
        sc,
        channel,
        thread_ts,
        incoming: msg,
        judgment,
        draft,
    })
}

/// CLI path: print the draft; post it (threaded) only with `send`.
fn reply(uia: &Uia, persona: &str, subdomain: &str, knowledge_path: &str, send: bool) -> Result<()> {
    let p = prepare_reply(uia, persona, subdomain, knowledge_path)?;
    if p.draft.trim().is_empty() {
        println!("=> 返信不要と判断。スキップします。");
        return Ok(());
    }
    println!("[下書き] {}", p.draft);
    if send {
        let r = p.sc.post_message(&p.channel, &p.draft, Some(&p.thread_ts))?;
        println!("=> スレッド返信 ok={}", r["ok"]);
    } else {
        println!("=> 下書きのみ（未送信）。実送信は `reply --send`。");
    }
    Ok(())
}

/// GUI path: open a local browser page so the human can review/edit/approve before sending.
/// Cheap Slack-only poll: ts of the most recent message actually aimed at us (DM or @mention),
/// so the watcher wakes on the same thing `prepare_reply` will answer — not on channel chatter.
fn latest_incoming_ts(subdomain: &str) -> Result<f64> {
    let sc = slack_api::SlackClient::connect(subdomain)?;
    let me = sc
        .auth_test()?["user_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok(find_latest_target(&sc, &me)?.map(|t| t.ts).unwrap_or(0.0))
}

/// Where the daily activity logs live: next to the knowledge base on Drive.
fn activity_log_dir(knowledge_path: &str) -> std::path::PathBuf {
    std::path::Path::new(knowledge_path)
        .parent()
        .map(|p| p.join("activity"))
        .unwrap_or_else(|| std::path::PathBuf::from("activity"))
}

/// Shareable reports folder on Drive — kept separate from the raw `activity` logs so the user
/// can share just this folder with 田上 once and have every report flow to them automatically.
fn reports_dir(knowledge_path: &str) -> std::path::PathBuf {
    std::path::Path::new(knowledge_path)
        .parent()
        .map(|p| p.join("レポート"))
        .unwrap_or_else(|| std::path::PathBuf::from("レポート"))
}

/// Write a combined, human-readable report (work breakdown + optional automation candidates)
/// into the shared reports folder on Drive; returns the file path.
fn write_shared_report(
    knowledge_path: &str,
    date: &str,
    breakdown: &str,
    automation: Option<&str>,
) -> std::path::PathBuf {
    let dir = reports_dir(knowledge_path);
    let _ = std::fs::create_dir_all(&dir);
    let mut body = String::from(breakdown);
    if let Some(a) = automation {
        body.push_str("\n\n== 自動化できそうな作業（Copilot提案） ==\n");
        body.push_str(a);
    }
    let path = dir.join(format!("{date} 作業レポート.txt"));
    let _ = std::fs::write(&path, body);
    path
}

/// Drive folder for diagnostic logs (so a hidden resident's failures are still visible).
fn logs_dir(knowledge_path: &str) -> std::path::PathBuf {
    std::path::Path::new(knowledge_path)
        .parent()
        .map(|p| p.join("logs"))
        .unwrap_or_else(|| std::path::PathBuf::from("logs"))
}

/// Append a timestamped line to today's log (logs/YYYY-MM-DD.log) on Drive. Best-effort.
fn log_line(knowledge_path: &str, level: &str, msg: &str) {
    use std::io::Write;
    let dir = logs_dir(knowledge_path);
    let _ = std::fs::create_dir_all(&dir);
    let (date, time, _) = win32::local_now();
    let line = format!("{date} {time} [{level}] {}\n", msg.replace(['\r', '\n'], " "));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{date}.log")))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Read the most recent log lines (newest last) for the GUI log view.
fn read_recent_logs(knowledge_path: &str, max_lines: usize) -> String {
    let dir = logs_dir(knowledge_path);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("log"))
        .collect();
    files.sort();
    // newest 2 files, read in chronological order
    let mut recent: Vec<_> = files.iter().rev().take(2).cloned().collect();
    recent.sort();
    let mut lines: Vec<String> = Vec::new();
    for f in &recent {
        if let Ok(txt) = std::fs::read_to_string(f) {
            for l in txt.lines() {
                lines.push(l.to_string());
            }
        }
    }
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    if lines.is_empty() {
        "（まだログはありません）".to_string()
    } else {
        lines.join("\n")
    }
}

/// Path of the logon-autostart launcher in the user's Startup folder.
fn autostart_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    std::path::PathBuf::from(appdata)
        .join("Microsoft\\Windows\\Start Menu\\Programs\\Startup")
        .join("AItagami.vbs")
}

/// Whether AI田上 is set to start the resident watcher at logon.
fn autostart_enabled() -> bool {
    autostart_path().exists()
}

/// Enable/disable logon autostart by writing (or removing) a tiny .vbs that launches
/// `tagami watch` hidden. The .vbs is UTF-16LE+BOM so a Japanese exe path survives.
fn set_autostart(enable: bool) -> Result<()> {
    let path = autostart_path();
    if enable {
        let exe = std::env::current_exe()?;
        let script = format!(
            "Set s = CreateObject(\"WScript.Shell\")\r\ns.Run \"\"\"{}\"\" watch\", 0, False\r\n",
            exe.display()
        );
        let mut bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16LE BOM
        for u in script.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        std::fs::write(&path, bytes)?;
    } else {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

/// Sample the foreground app every ~10s and append (only when it changes) one TSV line to
/// today's log: `epoch \t YYYY-MM-DD HH:MM:SS \t app \t title`. Idle stretches (no input for
/// >150s) are logged as "(idle)" so time away from the desk isn't counted as work.
fn activity_loop(dir: std::path::PathBuf) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(&dir);
    let mut last_key = String::new();
    let mut last_write = 0u64;
    loop {
        let idle = win32::idle_seconds();
        let (date, time, epoch) = win32::local_now();
        let (app, title) = if idle >= 150 {
            ("(idle)".to_string(), String::new())
        } else {
            let (a, t) = win32::foreground_app_title();
            let clean = |s: String| s.replace(['\t', '\r', '\n'], " ");
            (clean(a), truncate(&clean(t), 120))
        };
        let key = format!("{app}\u{1}{title}");
        // Log on every change, plus a heartbeat every 5 min so a long single-window session
        // still accumulates duration (and report can cap each gap at 5 min).
        if !app.is_empty() && (key != last_key || epoch.saturating_sub(last_write) >= 300) {
            last_key = key;
            last_write = epoch;
            let line = format!("{epoch}\t{date} {time}\t{app}\t{title}\n");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(format!("{date}.tsv")))
            {
                let _ = f.write_all(line.as_bytes());
            }
        }
        sleep(Duration::from_secs(10));
    }
}

/// Standalone resident activity logger (the watcher also runs this in a thread).
fn activity(knowledge_path: &str) -> Result<()> {
    win32::hide_console_if_owned();
    let dir = activity_log_dir(knowledge_path);
    println!("活動ログを記録します（停止は Ctrl+C / プロセス終了） → {}", dir.display());
    activity_loop(dir);
    Ok(())
}

/// Aggregate a day's activity log into a human-readable report (time per app, top windows).
/// Returned as a string so both the CLI and the GUI can show/save it.
fn aggregate_activity(knowledge_path: &str, persona: &str, date: &str) -> Result<String> {
    let dir = activity_log_dir(knowledge_path);
    let path = dir.join(format!("{date}.tsv"));
    let txt = std::fs::read_to_string(&path).map_err(|_| {
        anyhow!(
            "活動ログがありません: {}（まず記録を溜めてください）",
            path.display()
        )
    })?;
    let mut entries: Vec<(u64, String, String)> = Vec::new();
    for line in txt.lines() {
        let mut it = line.splitn(4, '\t');
        let epoch: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let _ts = it.next();
        let app = it.next().unwrap_or("").to_string();
        let title = it.next().unwrap_or("").to_string();
        if epoch > 0 {
            entries.push((epoch, app, title));
        }
    }
    if entries.len() < 2 {
        return Ok(format!(
            "{date} の記録が少なすぎます（{}件）。しばらく記録してから見てください。",
            entries.len()
        ));
    }
    let mut per_app: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut per_title: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut active = 0u64;
    for w in entries.windows(2) {
        // A gap is the time spent in w[0]'s state; cap it (lunch, meetings) so one stale entry
        // doesn't dominate.
        let dur = w[1].0.saturating_sub(w[0].0).min(300);
        let (app, title) = (&w[0].1, &w[0].2);
        if app == "(idle)" || app.is_empty() {
            continue;
        }
        active += dur;
        *per_app.entry(app.clone()).or_default() += dur;
        *per_title
            .entry(format!("{app}  |  {title}"))
            .or_default() += dur;
    }
    let fmt = |s: u64| format!("{}h{:02}m", s / 3600, (s % 3600) / 60);
    let mut apps: Vec<_> = per_app.iter().collect();
    apps.sort_by(|a, b| b.1.cmp(a.1));
    let mut titles: Vec<_> = per_title.iter().collect();
    titles.sort_by(|a, b| b.1.cmp(a.1));

    let mut out = String::new();
    out.push_str(&format!("== {date} 作業レポート（{persona}） ==\n"));
    out.push_str(&format!("アクティブ合計: {}\n\n[アプリ別]\n", fmt(active)));
    for (a, s) in apps.iter().take(15) {
        out.push_str(&format!("  {:<7}  {}\n", fmt(**s), a));
    }
    out.push_str("\n[上位の作業 アプリ｜ウィンドウ]\n");
    for (t, s) in titles.iter().take(25) {
        out.push_str(&format!("  {:<7}  {}\n", fmt(**s), truncate(t, 80)));
    }
    Ok(out)
}

/// The Copilot prompt that turns an activity report into automation candidates.
fn automation_prompt(persona: &str, report_text: &str) -> String {
    format!(
        "以下は『{persona}』のPC作業ログの集計です。ここから、繰り返し・定型で\u{201c}自動化できそうな作業\u{201d}を重要な順に箇条書きで挙げてください。\
         各項目は『作業内容 → 自動化の方針』の形で1〜2行にし、推測しすぎず、どのアプリ/ウィンドウから読み取れるかの根拠も短く添えてください。\n\n{report_text}"
    )
}

/// CLI: print the day's report; with `--ai` also ask Copilot for automation candidates.
fn report(
    uia: &Uia,
    knowledge_path: &str,
    persona: &str,
    date: Option<&str>,
    ai: bool,
) -> Result<()> {
    let day = date
        .map(String::from)
        .unwrap_or_else(|| win32::local_now().0);
    let out = aggregate_activity(knowledge_path, persona, &day)?;
    println!("{out}");
    let mut automation: Option<String> = None;
    if ai {
        println!("\nCopilotに自動化候補を抽出させています…");
        let resp = copilot_send_and_read(uia, &automation_prompt(persona, &out))?;
        println!("\n== 自動化候補（Copilot）==\n{resp}");
        automation = Some(resp);
    }
    let path = write_shared_report(knowledge_path, &day, &out, automation.as_deref());
    println!("→ Driveの共有フォルダに保存: {}", path.display());
    Ok(())
}

/// Resident watcher: poll Slack (API only); when a message newer than startup arrives,
/// open a fresh approval window in a separate process (winit runs once per process).
/// Copilot is invoked only when a new message is detected — no idle quota use.
/// Also runs the activity logger in a background thread so the resident covers both.
fn watch(subdomain: &str, interval_secs: u64, knowledge_path: &str) -> Result<()> {
    win32::hide_console_if_owned();
    log_line(
        knowledge_path,
        "INFO",
        &format!("常駐を開始 v{}（{}）", updater::current_version(), subdomain),
    );
    // Self-update on startup, then every ~6h, relaunching as `watch` so the resident stays
    // current automatically (errors — e.g. offline — are logged but non-fatal).
    if let Err(e) = updater::update_if_newer(&["watch"], false) {
        log_line(knowledge_path, "WARN", &format!("自動更新の確認に失敗: {e}"));
    }
    let mut last_update = std::time::Instant::now();
    let dir = activity_log_dir(knowledge_path);
    std::thread::spawn(move || activity_loop(dir));
    let exe = std::env::current_exe()?;
    let mut last_seen = 0f64;
    let mut initialized = false;
    let mut last_err = String::new();
    loop {
        std::thread::sleep(Duration::from_secs(interval_secs.max(30)));
        if last_update.elapsed() >= Duration::from_secs(6 * 3600) {
            last_update = std::time::Instant::now();
            if let Err(e) = updater::update_if_newer(&["watch"], false) {
                log_line(knowledge_path, "WARN", &format!("自動更新の確認に失敗: {e}"));
            }
        }
        let latest = match latest_incoming_ts(subdomain) {
            Ok(t) => t,
            Err(e) => {
                // Log Slack-poll failures once per distinct message (avoid spamming the log).
                let msg = e.to_string();
                if msg != last_err {
                    log_line(knowledge_path, "WARN", &format!("Slack確認に失敗: {msg}"));
                    last_err = msg;
                }
                continue;
            }
        };
        if !last_err.is_empty() {
            log_line(knowledge_path, "INFO", "Slack確認が復帰しました");
            last_err.clear();
        }
        if !initialized {
            last_seen = latest; // ignore the existing backlog at startup
            initialized = true;
            continue;
        }
        if latest > last_seen + 0.000_5 {
            last_seen = latest;
            log_line(knowledge_path, "INFO", "新着メッセージを検知 → 承認ウィンドウを起動");
            let _ = std::process::Command::new(&exe).arg("reply-gui").status();
        }
    }
}

/// 事前学習: collect the user's own past Slack messages, ask Copilot to distil a
/// style profile, and write it into the knowledge base so replies match their voice.
fn learn(uia: &Uia, persona: &str, subdomain: &str, knowledge_path: &str) -> Result<()> {
    let sc = slack_api::SlackClient::connect(subdomain)?;
    let me = sc
        .auth_test()?["user_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    println!("過去の自分の発言を収集中…");
    let conv = sc.call(
        "users.conversations",
        &[
            ("types", "public_channel,private_channel,im,mpim"),
            ("limit", "200"),
            ("exclude_archived", "true"),
        ],
    )?;
    let empty = Vec::new();
    let channels = conv["channels"].as_array().unwrap_or(&empty);
    let mut samples: Vec<String> = Vec::new();
    'outer: for ch in channels.iter().take(40) {
        let id = match ch["id"].as_str() {
            Some(s) => s,
            None => continue,
        };
        let h = match sc.conversations_history(id, 30) {
            Ok(h) => h,
            Err(_) => continue,
        };
        if let Some(msgs) = h["messages"].as_array() {
            for m in msgs {
                if m["user"].as_str() == Some(me.as_str()) {
                    let t = m["text"].as_str().unwrap_or("").trim();
                    if t.chars().count() >= 8 && !t.starts_with('<') {
                        samples.push(truncate(t, 200));
                        if samples.len() >= 40 {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }
    let drive_summary = collect_drive_summary("G:\\マイドライブ", 80);
    if samples.is_empty() && drive_summary.is_empty() {
        bail!("Slackの発言もGoogleドライブの項目も取得できませんでした");
    }
    println!(
        "Slack {}件 + Driveの項目 {}件 で分析します…",
        samples.len(),
        drive_summary.lines().filter(|l| !l.trim().is_empty()).count()
    );
    let joined = samples.join("\n---\n");
    let prompt = format!(
        "以下は『{persona}』本人のSlack発言例と、本人のGoogleドライブにあるフォルダ/ファイル名です。\
         ここから次の2つを箇条書きでまとめてください。\
         【人物・業務】= どんな役割・業務・専門・関心を持つ人か（断定せず『〜と思われる』程度に）。\
         【文体】= 本人になりきって返信するための書き方の特徴（書き出し・語尾・丁寧さ・絵文字・文の長さ）。\
         見出し『【人物・業務】』『【文体】』を付け、箇条書き本文だけを出力してください。\n\n\
         ■Slack発言例:\n{joined}\n\n■Googleドライブの項目:\n{drive_summary}"
    );
    let profile = copilot_send_and_read(uia, &prompt)?;
    if profile.trim().is_empty() {
        bail!("プロファイルの生成に失敗しました");
    }
    update_knowledge_section(
        knowledge_path,
        "## 自動生成プロファイル（人物・業務・文体）",
        profile.trim(),
    )?;
    println!(
        "プロファイルを保存しました → {knowledge_path}\n---\n{}",
        truncate(&profile, 400)
    );
    Ok(())
}

/// Top-level items in the user's Google Drive (folder/file names) — signals what they
/// create and work on. Names only (no content download); capped.
fn collect_drive_summary(root: &str, max: usize) -> String {
    let mut items: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('~') || name == "desktop.ini" {
                continue;
            }
            let kind = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "[フォルダ]"
            } else {
                "[ファイル]"
            };
            items.push(format!("{kind} {name}"));
            if items.len() >= max {
                break;
            }
        }
    }
    items.join("\n")
}

/// Replace (or append) a named section in the knowledge-base markdown file.
fn update_knowledge_section(path: &str, header: &str, body: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let cleaned = if let Some(idx) = existing.find(header) {
        let before = existing[..idx].trim_end().to_string();
        let after = &existing[idx + header.len()..];
        if let Some(rel) = after.find("\n## ") {
            format!("{before}\n\n{}", after[rel + 1..].trim_start())
        } else {
            before
        }
    } else {
        existing.trim_end().to_string()
    };
    let new = if cleaned.is_empty() {
        format!("{header}\n{body}\n")
    } else {
        format!("{cleaned}\n\n{header}\n{body}\n")
    };
    std::fs::write(path, new)?;
    Ok(())
}

/// Launch the desktop window immediately (showing "準備中…") and do the slow
/// Slack+Copilot work on a background thread so the user always sees a GUI.
/// The desktop app. `auto` = launched by the watcher for a fresh message → go straight to the
/// reply view and start loading; otherwise (double-click) show the Home menu.
fn reply_gui(persona: &str, subdomain: &str, knowledge_path: &str, auto: bool) -> Result<()> {
    win32::hide_console_if_owned();
    let mut app = GuiApp {
        persona: persona.to_owned(),
        subdomain: subdomain.to_owned(),
        knowledge_path: knowledge_path.to_owned(),
        view: if auto { View::Reply } else { View::Home },
        rx: None,
        stage: Stage::Loading,
        incoming: String::new(),
        judgment: String::new(),
        draft: String::new(),
        sc: None,
        channel: String::new(),
        thread_ts: String::new(),
        status: String::new(),
        error: String::new(),
        done: false,
        r_date: win32::local_now().0,
        r_text: String::new(),
        r_ai: String::new(),
        r_loading: false,
        r_rx: None,
        r_msg: String::new(),
        log_text: String::new(),
        s_persona: persona.to_owned(),
        s_subdomain: subdomain.to_owned(),
        s_knowledge: knowledge_path.to_owned(),
        s_interval: config::Config::load().watch_interval_secs.to_string(),
        settings_msg: String::new(),
    };
    if auto {
        app.start_reply_load();
    }
    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([760.0, 680.0])
            .with_drag_and_drop(false),
        ..Default::default()
    };
    eframe::run_native(
        "AI田上",
        opts,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow!("eframe error: {e}"))?;
    Ok(())
}

/// Load a Japanese-capable font from Windows so egui can render CJK text.
fn setup_fonts(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::default();
    let candidates = [
        "C:\\Windows\\Fonts\\meiryo.ttc",
        "C:\\Windows\\Fonts\\YuGothR.ttc",
        "C:\\Windows\\Fonts\\YuGothM.ttc",
        "C:\\Windows\\Fonts\\msgothic.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("jp".to_owned(), FontData::from_owned(bytes));
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "jp".to_owned());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("jp".to_owned());
            break;
        }
    }
    ctx.set_fonts(fonts);
}

enum PrepMsg {
    Ready(Box<Prepared>),
    NoReply(String),
    Error(String),
}

#[derive(PartialEq)]
enum Stage {
    Loading,
    Ready,
    NoReply,
    Error,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Home,
    Reply,
    Report,
    Log,
    Settings,
}

struct GuiApp {
    persona: String,
    subdomain: String,
    knowledge_path: String,
    view: View,
    // reply flow
    rx: Option<std::sync::mpsc::Receiver<PrepMsg>>,
    stage: Stage,
    incoming: String,
    judgment: String,
    draft: String,
    sc: Option<slack_api::SlackClient>,
    channel: String,
    thread_ts: String,
    status: String,
    error: String,
    done: bool,
    // report view
    r_date: String,
    r_text: String,
    r_ai: String,
    r_loading: bool,
    r_rx: Option<std::sync::mpsc::Receiver<std::result::Result<String, String>>>,
    r_msg: String,
    // log view
    log_text: String,
    // settings editor
    s_persona: String,
    s_subdomain: String,
    s_knowledge: String,
    s_interval: String,
    settings_msg: String,
}

impl GuiApp {
    /// Spawn the (Slack + Copilot) reply preparation on a background thread.
    fn start_reply_load(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel::<PrepMsg>();
        let (persona, subdomain, knowledge) = (
            self.persona.clone(),
            self.subdomain.clone(),
            self.knowledge_path.clone(),
        );
        std::thread::spawn(move || {
            let result = (|| -> Result<PrepMsg> {
                let uia = Uia::new()?; // own COM/UIA on this thread
                let p = prepare_reply(&uia, &persona, &subdomain, &knowledge)?;
                if p.draft.trim().is_empty() {
                    Ok(PrepMsg::NoReply(p.judgment))
                } else {
                    Ok(PrepMsg::Ready(Box::new(p)))
                }
            })();
            let msg = match result {
                Ok(m) => m,
                Err(e) => {
                    log_line(&knowledge, "ERROR", &format!("返信準備に失敗: {e}"));
                    PrepMsg::Error(e.to_string())
                }
            };
            let _ = tx.send(msg);
        });
        self.rx = Some(rx);
        self.stage = Stage::Loading;
        self.error.clear();
        self.status.clear();
        self.done = false;
    }

    /// Spawn (aggregate + Copilot) automation-candidate extraction on a background thread.
    fn start_report_ai(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<String, String>>();
        let (persona, knowledge, date) = (
            self.persona.clone(),
            self.knowledge_path.clone(),
            self.r_date.trim().to_owned(),
        );
        std::thread::spawn(move || {
            let result = (|| -> Result<String> {
                let uia = Uia::new()?;
                let out = aggregate_activity(&knowledge, &persona, &date)?;
                copilot_send_and_read(&uia, &automation_prompt(&persona, &out))
            })();
            if let Err(e) = &result {
                log_line(&knowledge, "ERROR", &format!("自動化候補の抽出に失敗: {e}"));
            }
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
        self.r_rx = Some(rx);
        self.r_loading = true;
        self.r_msg = "Copilotで自動化候補を抽出中…（30〜60秒）".to_owned();
    }

    fn ui_home(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        ui.add_space(12.0);
        ui.label("やりたいことを選んでください。");
        ui.add_space(14.0);
        let big = egui::vec2(320.0, 40.0);
        if ui
            .add(egui::Button::new("✉   Slackの返信を確認する").min_size(big))
            .clicked()
        {
            self.view = View::Reply;
            if self.rx.is_none() {
                self.start_reply_load();
            }
        }
        ui.add_space(8.0);
        if ui
            .add(egui::Button::new("📊   今日の作業レポートを見る").min_size(big))
            .clicked()
        {
            self.view = View::Report;
            self.r_text =
                aggregate_activity(&self.knowledge_path, &self.persona, self.r_date.trim())
                    .unwrap_or_else(|e| e.to_string());
        }
        ui.add_space(8.0);
        if ui
            .add(egui::Button::new("📜   ログ（エラー・動作履歴）").min_size(big))
            .clicked()
        {
            self.view = View::Log;
            self.log_text = read_recent_logs(&self.knowledge_path, 400);
        }
        ui.add_space(8.0);
        if ui.add(egui::Button::new("⚙   設定").min_size(big)).clicked() {
            self.view = View::Settings;
        }
    }

    fn ui_log(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        ui.horizontal(|ui| {
            ui.label("常駐・返信・更新の動作履歴とエラーです（新しいものが下）。");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📁 ログフォルダを開く").clicked() {
                    let dir = logs_dir(&self.knowledge_path);
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                }
                if ui.button("🔄 更新").clicked() {
                    self.log_text = read_recent_logs(&self.knowledge_path, 400);
                }
            });
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_source("logview")
            .max_height(540.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.monospace(&self.log_text);
            });
    }

    fn ui_report(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        ui.horizontal(|ui| {
            ui.label("日付:");
            ui.add(egui::TextEdit::singleline(&mut self.r_date).desired_width(110.0));
            if ui.button("集計").clicked() {
                self.r_text =
                    aggregate_activity(&self.knowledge_path, &self.persona, self.r_date.trim())
                        .unwrap_or_else(|e| e.to_string());
                self.r_ai.clear();
                write_shared_report(&self.knowledge_path, self.r_date.trim(), &self.r_text, None);
            }
            if ui
                .add_enabled(!self.r_loading, egui::Button::new("🤖 自動化候補を出す"))
                .clicked()
            {
                if self.r_text.is_empty() {
                    self.r_text = aggregate_activity(
                        &self.knowledge_path,
                        &self.persona,
                        self.r_date.trim(),
                    )
                    .unwrap_or_else(|e| e.to_string());
                }
                self.start_report_ai();
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let dir = reports_dir(&self.knowledge_path);
            ui.label("共有フォルダ(Drive):");
            ui.monospace(dir.display().to_string());
            if ui.button("📁 開く").clicked() {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer").arg(&dir).spawn();
            }
        });
        if !self.r_msg.is_empty() {
            ui.add_space(4.0);
            if self.r_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&self.r_msg);
                });
            } else {
                ui.colored_label(egui::Color32::from_rgb(0xcc, 0x55, 0x55), &self.r_msg);
            }
        }
        ui.add_space(6.0);
        let report_h = if self.r_ai.is_empty() { 470.0 } else { 210.0 };
        egui::ScrollArea::vertical()
            .id_source("report")
            .max_height(report_h)
            .show(ui, |ui| {
                ui.monospace(if self.r_text.is_empty() {
                    "「集計」を押すと、その日の作業内訳（アプリ別・上位の作業）が出ます。\n「🤖 自動化候補を出す」でCopilotが自動化できそうな作業を提案します。"
                } else {
                    self.r_text.as_str()
                });
            });
        if !self.r_ai.is_empty() {
            ui.add_space(6.0);
            ui.label("🤖 自動化できそうな作業（Copilot）:");
            egui::ScrollArea::vertical()
                .id_source("ai")
                .max_height(250.0)
                .show(ui, |ui| {
                    ui.label(&self.r_ai);
                });
        }
    }

    fn ui_settings(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        ui.label("設定（保存すると次回起動から反映されます）");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("返信の主体（アカウント名）:");
            ui.text_edit_singleline(&mut self.s_persona);
        });
        ui.horizontal(|ui| {
            ui.label("Slackワークスペース（サブドメイン）:");
            ui.text_edit_singleline(&mut self.s_subdomain);
        });
        ui.add_space(4.0);
        ui.label("知識ベースの保存場所（ファイルパス）:");
        ui.add(egui::TextEdit::singleline(&mut self.s_knowledge).desired_width(f32::INFINITY));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Slack確認の間隔（秒・最小30）:");
            ui.text_edit_singleline(&mut self.s_interval);
        });
        ui.add_space(8.0);
        let mut auto = autostart_enabled();
        if ui
            .checkbox(
                &mut auto,
                "PC起動時に自動で常駐する（メンション監視＋活動記録）",
            )
            .changed()
        {
            self.settings_msg = match set_autostart(auto) {
                Ok(_) if auto => "自動起動を設定しました（次回ログインから有効）".to_owned(),
                Ok(_) => "自動起動を解除しました".to_owned(),
                Err(e) => format!("自動起動の設定に失敗: {e}"),
            };
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("保存").clicked() {
                let c = config::Config {
                    persona: self.s_persona.trim().to_owned(),
                    slack_subdomain: self.s_subdomain.trim().to_owned(),
                    knowledge_path: self.s_knowledge.trim().to_owned(),
                    watch_interval_secs: self.s_interval.trim().parse().unwrap_or(180),
                };
                self.settings_msg = match c.save() {
                    Ok(_) => "保存しました（次回起動から反映）".to_owned(),
                    Err(e) => format!("保存失敗: {e}"),
                };
            }
            if ui.button("ホームへ").clicked() {
                self.view = View::Home;
            }
        });
        if !self.settings_msg.is_empty() {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::from_rgb(0x2e, 0xb6, 0x7d), &self.settings_msg);
        }
    }

    fn ui_reply(&mut self, ui: &mut eframe::egui::Ui, ctx: &eframe::egui::Context) {
        use eframe::egui;
        if self.rx.is_none() {
            ui.add_space(16.0);
            ui.label("最新のあなた宛メッセージ（メンション/DM）から返信案を作ります。");
            ui.add_space(8.0);
            if ui.button("返信案を作成（30〜60秒）").clicked() {
                self.start_reply_load();
            }
            return;
        }
        match self.stage {
            Stage::Loading => {
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("準備中… Slack取得・AI判断・返信生成（30〜60秒ほど）");
                });
            }
            Stage::Error => {
                ui.colored_label(egui::Color32::RED, format!("エラー: {}", self.error));
                ui.add_space(6.0);
                if ui.button("再試行").clicked() {
                    self.start_reply_load();
                }
            }
            Stage::NoReply => {
                ui.label(format!("AIの判断: {}", self.judgment));
                ui.add_space(6.0);
                ui.label("→ 返信不要と判断。送信する返信はありません。");
                ui.add_space(6.0);
                if ui.button("もう一度確認").clicked() {
                    self.start_reply_load();
                }
            }
            Stage::Ready => {
                ui.label("受信メッセージ:");
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(&self.incoming);
                });
                ui.add_space(4.0);
                ui.label(format!("AIの判断: {}", self.judgment));
                ui.add_space(6.0);
                ui.label("返信案（編集できます）:");
                ui.add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.done, egui::Button::new("スレッドに送信"))
                        .clicked()
                    {
                        if let Some(sc) = &self.sc {
                            match sc.post_message(&self.channel, &self.draft, Some(&self.thread_ts)) {
                                Ok(j) => {
                                    self.status = format!("送信しました（ok={}）", j["ok"]);
                                    log_line(
                                        &self.knowledge_path,
                                        "INFO",
                                        &format!("返信を送信（ch={}）", self.channel),
                                    );
                                }
                                Err(e) => {
                                    self.status = format!("送信エラー: {e}");
                                    log_line(&self.knowledge_path, "ERROR", &format!("送信失敗: {e}"));
                                }
                            }
                            self.done = true;
                        }
                    }
                    if ui
                        .add_enabled(!self.done, egui::Button::new("送らない"))
                        .clicked()
                    {
                        self.status = "送信しませんでした。".to_owned();
                        self.done = true;
                    }
                    if self.done && ui.button("閉じる").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                if !self.status.is_empty() {
                    ui.add_space(8.0);
                    ui.colored_label(egui::Color32::from_rgb(0x2e, 0xb6, 0x7d), &self.status);
                }
            }
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        use eframe::egui;

        // Reply preparation result (background thread).
        if self.stage == Stage::Loading {
            if let Some(rx) = &self.rx {
                match rx.try_recv() {
                    Ok(PrepMsg::Ready(p)) => {
                        let p = *p;
                        self.incoming = p.incoming;
                        self.judgment = p.judgment;
                        self.draft = p.draft;
                        self.channel = p.channel;
                        self.thread_ts = p.thread_ts;
                        self.sc = Some(p.sc);
                        self.stage = Stage::Ready;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    Ok(PrepMsg::NoReply(j)) => {
                        self.judgment = j;
                        self.stage = Stage::NoReply;
                    }
                    Ok(PrepMsg::Error(e)) => {
                        self.error = e;
                        self.stage = Stage::Error;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.error = "準備処理が異常終了しました".to_owned();
                        self.stage = Stage::Error;
                    }
                }
            }
        }

        // Automation-candidate result (background thread).
        if self.r_loading {
            if let Some(rx) = &self.r_rx {
                match rx.try_recv() {
                    Ok(Ok(resp)) => {
                        self.r_ai = resp;
                        self.r_loading = false;
                        self.r_rx = None;
                        self.r_msg.clear();
                        write_shared_report(
                            &self.knowledge_path,
                            self.r_date.trim(),
                            &self.r_text,
                            Some(&self.r_ai),
                        );
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    Ok(Err(e)) => {
                        self.r_msg = format!("エラー: {e}");
                        self.r_loading = false;
                        self.r_rx = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(std::time::Duration::from_millis(250));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.r_msg = "処理が異常終了しました".to_owned();
                        self.r_loading = false;
                        self.r_rx = None;
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("AI田上");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(self.view == View::Settings, "⚙ 設定")
                        .clicked()
                    {
                        self.view = View::Settings;
                    }
                    if ui
                        .selectable_label(self.view == View::Log, "📜 ログ")
                        .clicked()
                    {
                        self.view = View::Log;
                        self.log_text = read_recent_logs(&self.knowledge_path, 400);
                    }
                    if ui
                        .selectable_label(self.view == View::Report, "📊 レポート")
                        .clicked()
                    {
                        self.view = View::Report;
                    }
                    if ui
                        .selectable_label(self.view == View::Reply, "✉ 返信")
                        .clicked()
                    {
                        self.view = View::Reply;
                    }
                    if ui
                        .selectable_label(self.view == View::Home, "🏠 ホーム")
                        .clicked()
                    {
                        self.view = View::Home;
                    }
                });
            });
            ui.separator();

            match self.view {
                View::Home => self.ui_home(ui),
                View::Reply => self.ui_reply(ui, ctx),
                View::Report => self.ui_report(ui),
                View::Log => self.ui_log(ui),
                View::Settings => self.ui_settings(ui),
            }
        });
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let uia = Uia::new()?;
    let cfg = config::Config::load();
    // Record any panic to the log on Drive — vital for the hidden resident.
    {
        let kp = cfg.knowledge_path.clone();
        std::panic::set_hook(Box::new(move |info| {
            log_line(&kp, "PANIC", &format!("{info}"));
        }));
    }
    let result = (|| -> Result<()> {
        match cmd {
        "slack-read" => slack_read(&uia)?,
        "copilot-read" => copilot_read(&uia)?,
        "copilot-type" => {
            let text = args.get(2).ok_or_else(|| anyhow!("usage: tagami copilot-type \"text\""))?;
            copilot_type(&uia, text)?;
        }
        "copilot-ask" => {
            let text = args.get(2).ok_or_else(|| anyhow!("usage: tagami copilot-ask \"prompt\""))?;
            copilot_ask(&uia, text)?;
        }
        "slack-auth" => {
            let sc = slack_api::SlackClient::connect(&cfg.slack_subdomain)?;
            let r = sc.auth_test()?;
            println!("host={}", sc.host);
            println!("auth.test => {}", serde_json::to_string_pretty(&r)?);
        }
        "reply" => {
            let send = args.iter().any(|a| a == "--send");
            reply(&uia, &cfg.persona, &cfg.slack_subdomain, &cfg.knowledge_path, send)?;
        }
        "reply-gui" => {
            // Launched by the watcher for a fresh message → go straight to the reply view.
            reply_gui(&cfg.persona, &cfg.slack_subdomain, &cfg.knowledge_path, true)?;
        }
        "watch" => {
            watch(&cfg.slack_subdomain, cfg.watch_interval_secs, &cfg.knowledge_path)?;
        }
        "activity" => {
            activity(&cfg.knowledge_path)?;
        }
        "autostart" => {
            let on = args.get(2).map(|s| s != "off").unwrap_or(true);
            set_autostart(on)?;
            println!(
                "ログイン時の自動起動: {}（{}）",
                if on { "ON" } else { "OFF" },
                autostart_path().display()
            );
        }
        "report" => {
            let date = args.get(2).filter(|a| !a.starts_with("--")).map(String::as_str);
            let ai = args.iter().any(|a| a == "--ai");
            report(&uia, &cfg.knowledge_path, &cfg.persona, date, ai)?;
        }
        "learn" => {
            learn(&uia, &cfg.persona, &cfg.slack_subdomain, &cfg.knowledge_path)?;
        }
        "copilot-show" => {
            match find_visible_window_by_title("Copilot") {
                Some(h) => {
                    move_onscreen(h, 60, 60);
                    println!("Copilotを画面に戻しました。");
                }
                None => println!("Copilotウィンドウが見つかりません（起動していますか？）。"),
            }
        }
        "update" => {
            updater::check_and_update()?;
        }
        "version" => {
            println!("tagami {}", updater::current_version());
        }
        "help" | "-h" | "--help" => {
            println!("AI田上 PoC");
            println!("usage (no argument = open the desktop approval window):");
            println!("  tagami                  # = reply-gui (double-click opens this)");
            println!("  tagami reply-gui        # review/edit/approve a reply in a desktop window, then send");
            println!("  tagami watch            # resident: poll Slack, pop the approval window on new messages");
            println!("  tagami learn            # learn your writing style from past Slack posts -> knowledge.md");
            println!("  tagami activity         # resident: log which app/window you use over time -> Drive");
            println!("  tagami report [date] [--ai]  # summarise a day's activity (--ai: Copilot suggests automation)");
            println!("  tagami autostart [on|off]    # start the resident watcher automatically at logon");
            println!("  tagami reply [--send]   # CLI: draft (or send) a reply to the latest Slack message");
            println!("  tagami copilot-show     # bring the (hidden, off-screen) Copilot window back on screen");
            println!("  tagami update           # self-update from the latest GitHub release");
            println!("  tagami version          # show current version");
            println!("  tagami slack-auth       # check Slack auth");
        }
        _ => {
            // No argument (e.g. double-click) or unknown command: open the desktop GUI home.
            reply_gui(&cfg.persona, &cfg.slack_subdomain, &cfg.knowledge_path, false)?;
        }
        }
        Ok(())
    })();
    if let Err(e) = &result {
        log_line(&cfg.knowledge_path, "ERROR", &format!("コマンド'{cmd}'で終了: {e}"));
    }
    result
}
