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
    let mut best_ts = 0f64;
    let mut target: Option<(String, String, String)> = None; // (channel, text, thread_ts)
    for ch in channels.iter().take(30) {
        let id = match ch["id"].as_str() {
            Some(s) => s,
            None => continue,
        };
        let h = match sc.conversations_history(id, 3) {
            Ok(h) => h,
            Err(_) => continue,
        };
        if let Some(msgs) = h["messages"].as_array() {
            for m in msgs {
                let u = m["user"].as_str().unwrap_or("");
                let text = m["text"].as_str().unwrap_or("");
                let ts_str = m["ts"].as_str().unwrap_or("");
                let ts: f64 = ts_str.parse().unwrap_or(0.0);
                if u != me && !text.trim().is_empty() && ts > best_ts {
                    best_ts = ts;
                    let thread_ts = m["thread_ts"].as_str().unwrap_or(ts_str).to_string();
                    target = Some((id.to_string(), text.to_string(), thread_ts));
                }
            }
        }
    }
    let (channel, msg, thread_ts) =
        target.ok_or_else(|| anyhow!("no incoming message found in workspace"))?;
    println!("[最新の受信] ch={} : {}", channel, truncate(&msg, 140));
    let msg = truncate(&msg, 500);

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
        "{knowledge_block}あなたは『{persona}』本人です。次のSlackメッセージへの対応を決めてください。\
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
/// Cheap Slack-only poll: ts of the most recent incoming message (no Copilot used).
fn latest_incoming_ts(subdomain: &str) -> Result<f64> {
    let sc = slack_api::SlackClient::connect(subdomain)?;
    let me = sc
        .auth_test()?["user_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
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
    let mut best = 0f64;
    for ch in channels.iter().take(30) {
        let id = match ch["id"].as_str() {
            Some(s) => s,
            None => continue,
        };
        let h = match sc.conversations_history(id, 1) {
            Ok(h) => h,
            Err(_) => continue,
        };
        if let Some(msgs) = h["messages"].as_array() {
            for m in msgs {
                let u = m["user"].as_str().unwrap_or("");
                let ts: f64 = m["ts"].as_str().unwrap_or("0").parse().unwrap_or(0.0);
                if u != me && ts > best {
                    best = ts;
                }
            }
        }
    }
    Ok(best)
}

/// Resident watcher: poll Slack (API only); when a message newer than startup arrives,
/// open a fresh approval window in a separate process (winit runs once per process).
/// Copilot is invoked only when a new message is detected — no idle quota use.
fn watch(subdomain: &str, interval_secs: u64) -> Result<()> {
    win32::hide_console_if_owned();
    let exe = std::env::current_exe()?;
    let mut last_seen = 0f64;
    let mut initialized = false;
    loop {
        std::thread::sleep(Duration::from_secs(interval_secs.max(30)));
        let latest = match latest_incoming_ts(subdomain) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !initialized {
            last_seen = latest; // ignore the existing backlog at startup
            initialized = true;
            continue;
        }
        if latest > last_seen + 0.000_5 {
            last_seen = latest;
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
fn reply_gui(_uia: &Uia, persona: &str, subdomain: &str, knowledge_path: &str) -> Result<()> {
    win32::hide_console_if_owned();

    let (tx, rx) = std::sync::mpsc::channel::<PrepMsg>();
    {
        let persona = persona.to_owned();
        let subdomain = subdomain.to_owned();
        let knowledge_path = knowledge_path.to_owned();
        std::thread::spawn(move || {
            let result = (|| -> Result<PrepMsg> {
                let uia = Uia::new()?; // own COM/UIA on this thread
                let p = prepare_reply(&uia, &persona, &subdomain, &knowledge_path)?;
                if p.draft.trim().is_empty() {
                    Ok(PrepMsg::NoReply(p.judgment))
                } else {
                    Ok(PrepMsg::Ready(Box::new(p)))
                }
            })();
            let _ = tx.send(result.unwrap_or_else(|e| PrepMsg::Error(e.to_string())));
        });
    }

    let app = GuiApp {
        persona: persona.to_owned(),
        rx,
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
        settings_open: false,
        s_persona: persona.to_owned(),
        s_subdomain: subdomain.to_owned(),
        s_knowledge: knowledge_path.to_owned(),
        s_interval: config::Config::load().watch_interval_secs.to_string(),
        settings_msg: String::new(),
    };
    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([720.0, 640.0])
            .with_drag_and_drop(false),
        ..Default::default()
    };
    eframe::run_native(
        "AI田上 返信確認",
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

struct GuiApp {
    persona: String,
    rx: std::sync::mpsc::Receiver<PrepMsg>,
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
    // settings editor
    settings_open: bool,
    s_persona: String,
    s_subdomain: String,
    s_knowledge: String,
    s_interval: String,
    settings_msg: String,
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        use eframe::egui;
        if self.stage == Stage::Loading {
            match self.rx.try_recv() {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("AI田上（{} として）", self.persona));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙ 設定").clicked() {
                        self.settings_open = !self.settings_open;
                    }
                });
            });
            ui.separator();

            if self.settings_open {
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
                    if ui.button("戻る").clicked() {
                        self.settings_open = false;
                    }
                });
                if !self.settings_msg.is_empty() {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(0x2e, 0xb6, 0x7d), &self.settings_msg);
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
                }
                Stage::NoReply => {
                    ui.label(format!("AIの判断: {}", self.judgment));
                    ui.add_space(6.0);
                    ui.label("→ 返信不要と判断。送信する返信はありません。");
                    if ui.button("閉じる").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
                                    Ok(j) => self.status = format!("送信しました（ok={}）", j["ok"]),
                                    Err(e) => self.status = format!("送信エラー: {e}"),
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
        });
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let uia = Uia::new()?;
    let cfg = config::Config::load();
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
            reply_gui(&uia, &cfg.persona, &cfg.slack_subdomain, &cfg.knowledge_path)?;
        }
        "watch" => {
            watch(&cfg.slack_subdomain, cfg.watch_interval_secs)?;
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
            println!("  tagami reply [--send]   # CLI: draft (or send) a reply to the latest Slack message");
            println!("  tagami copilot-show     # bring the (hidden, off-screen) Copilot window back on screen");
            println!("  tagami update           # self-update from the latest GitHub release");
            println!("  tagami version          # show current version");
            println!("  tagami slack-auth       # check Slack auth");
        }
        _ => {
            // No argument (e.g. double-click) or unknown command: open the desktop GUI.
            reply_gui(&uia, &cfg.persona, &cfg.slack_subdomain, &cfg.knowledge_path)?;
        }
    }
    Ok(())
}
