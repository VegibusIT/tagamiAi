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

use anyhow::{anyhow, Result};
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
        if t == "Copilot へメッセージを送る" {
            break;
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
fn copilot_send_and_read(uia: &Uia, prompt: &str) -> Result<String> {
    let hwnd = find_visible_window_by_title("Copilot").ok_or_else(|| anyhow!("Copilot window not found"))?;
    restore_and_foreground(hwnd);

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
    // Click the input box to guarantee keyboard focus (UIA SetFocus alone is unreliable here).
    if let Some((l, t, r, b)) = uia::bounding_rect(&input) {
        click((l + r) / 2, (t + b) / 2);
        sleep(Duration::from_millis(250));
    }
    set_focus(&input);
    sleep(Duration::from_millis(300));
    select_all();
    sleep(Duration::from_millis(150));
    // Paste long prompts via the clipboard — per-keystroke typing drops characters
    // on long input and the message never submits.
    if set_clipboard_text(&clean) {
        paste();
    } else {
        type_unicode(&clean);
    }
    sleep(Duration::from_millis(700));
    press_enter();

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
fn prepare_reply(uia: &Uia, persona: &str, subdomain: &str) -> Result<Prepared> {
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

    // Single combined call (judgment + draft) to halve Copilot usage (free quota).
    let prompt = format!(
        "あなたは『{persona}』本人です。次のSlackメッセージへの対応を決めてください。\
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
fn reply(uia: &Uia, persona: &str, subdomain: &str, send: bool) -> Result<()> {
    let p = prepare_reply(uia, persona, subdomain)?;
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
fn reply_gui(uia: &Uia, persona: &str, subdomain: &str) -> Result<()> {
    let p = prepare_reply(uia, persona, subdomain)?;
    if p.draft.trim().is_empty() {
        println!("=> 返信不要と判断。GUIは開きません。");
        return Ok(());
    }
    serve_approval(p, persona)
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

struct ApprovalApp {
    persona: String,
    incoming: String,
    judgment: String,
    draft: String,
    sc: slack_api::SlackClient,
    channel: String,
    thread_ts: String,
    status: String,
    done: bool,
}

impl eframe::App for ApprovalApp {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        use eframe::egui;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("AI田上 — 返信の確認（{} として）", self.persona));
            ui.add_space(6.0);
            ui.label("受信メッセージ:");
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(&self.incoming);
            });
            ui.add_space(4.0);
            ui.label(format!("AIの判断: {}", self.judgment));
            ui.add_space(8.0);
            ui.label("返信案（編集できます）:");
            ui.add(
                egui::TextEdit::multiline(&mut self.draft)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.done, egui::Button::new("スレッドに送信"))
                    .clicked()
                {
                    match self
                        .sc
                        .post_message(&self.channel, &self.draft, Some(&self.thread_ts))
                    {
                        Ok(j) => self.status = format!("送信しました（ok={}）", j["ok"]),
                        Err(e) => self.status = format!("送信エラー: {e}"),
                    }
                    self.done = true;
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
        });
    }
}

/// Open a native desktop window for the human to review/edit/approve, then send.
fn serve_approval(p: Prepared, persona: &str) -> Result<()> {
    let app = ApprovalApp {
        persona: persona.to_owned(),
        incoming: p.incoming,
        judgment: p.judgment,
        draft: p.draft,
        sc: p.sc,
        channel: p.channel,
        thread_ts: p.thread_ts,
        status: String::new(),
        done: false,
    };
    let opts = eframe::NativeOptions {
        // Disable drag&drop so winit doesn't call OleInitialize (STA) — our UIA code
        // already put this thread in COM MTA, which would otherwise clash.
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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");
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
            reply(&uia, &cfg.persona, &cfg.slack_subdomain, send)?;
        }
        "reply-gui" => {
            reply_gui(&uia, &cfg.persona, &cfg.slack_subdomain)?;
        }
        "update" => {
            updater::check_and_update()?;
        }
        "version" => {
            println!("tagami {}", updater::current_version());
        }
        _ => {
            println!("AI田上 PoC");
            println!("usage:");
            println!("  tagami slack-read");
            println!("  tagami copilot-read");
            println!("  tagami copilot-type \"text\"");
            println!("  tagami copilot-ask \"prompt\"");
            println!("  tagami reply [--send]   # draft (or send) a reply to the latest Slack message");
            println!("  tagami reply-gui        # review/edit/approve a reply in a desktop window, then send");
            println!("  tagami update           # self-update from the latest GitHub release");
            println!("  tagami version          # show current version");
        }
    }
    Ok(())
}
