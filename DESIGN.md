# AI田上（営業特化）— 設計書

> Slack / LINE / メールのデスクトップアプリから受信を集約し、「TODO化 → 田上さんの文体で返信下書き → 承認 → 送信」を行う **Rust 製の営業支援AI**。あわせて、やり取りから**人間関係を徐々に可視化**（軽量CRM＋関係グラフ）する。

- ステータス: ドラフト v0.2
- 言語 / ランタイム: **Rust + tokio (async)**、対象OS: **Windows 11**
- LLM: **Gemini**（差し替え可能な層）
- 最終更新: 2026-06-19

---

## 1. ゴールと非ゴール

### ゴール
- Slack（VegibusHQ）/ メール（Outlook）/ LINE（個人）の **デスクトップアプリから**受信を集約。
- AI が **営業観点**で分類して TODO 化（新規リード / 商談 / フォロー要 / 提案・見積 / 社内 / FYI など）。
- 田上さんの文体で**返信下書き**を生成 → **承認 → 送信**（human-in-the-loop）。
- やり取りから人物・会社・商談を抽出し、**人間関係を徐々に可視化**（関係グラフ＋接触履歴）。
- 営業資料（提案書・見積・カタログ等）を **Google Drive** から参照・添付（連携API、スクレイピング不要）。
- **徐々に学習**：田上さんの修正を貯めて文体・対応方針の精度を上げる。

### 非ゴール（初期）
- 全チャネルの完全自動送信（誤送信・信用リスク。まずは下書き＋承認）。
- LINE 個人アカウントの公式API連携（存在しない）。
- 高度なCRM機能（パイプライン分析、売上予測など）は将来拡張。

---

## 2. 全体アーキテクチャ

```
┌──────── Desktop Automation Layer (Windows) ─────────┐
│  Outlook (COM/Object Model)   受信◎ 送信◎            │
│  Slack デスクトップ (UI Automation)  受信△ 送信△     │
│  LINE デスクトップ (UI Automation)   受信△ 送信×(手動)│
└───────────────┬─────────────────────────────────────┘
                │ normalize → IncomingMessage
                ▼
        ┌────────────────┐   Gemini
        │  Triage Engine  │────────►  営業分類 / 優先度 / 要約
        └───────┬────────┘
                ├──────────────► Entity/Relationship 抽出（人物・会社・商談）
                │                       │
                ▼                       ▼
        ┌────────────────┐      ┌──────────────────┐
        │   Responder     │      │ Relationship Graph│
        │ (文体で下書き)   │      │   軽量CRM / 可視化  │
        └───────┬────────┘      └──────────────────┘
                ▼
        ┌────────────────┐
        │  Approval UI    │  TUI / Web(iPhone対応) — 承認・編集
        └───────┬────────┘
                ▼
   Desktop Adapter.send()  →  相手へ送信
                │
                ▼
   Store (SQLite): messages / todos / drafts / contacts / relationships / interactions / style_samples
```

すべて `daemon`（常駐プロセス）が `tokio` 上でオーケストレーション。デスクトップ取得はポーリング中心。

---

## 3. データフロー

1. **取込:** 各デスクトップアダプタが新着を取得 → `IncomingMessage` に正規化 → Store 保存。
2. **分類:** Gemini が営業観点で分類（カテゴリ・優先度・要約）→ `Todo` 生成。
3. **関係抽出:** 同じメッセージから人物/会社/商談を抽出 → `contacts` / `interactions` / `relationships` を更新（徐々に充実）。
4. **下書き:** 要返信に、文体プロファイル＋類似事例(few-shot)＋関係コンテキストで `ReplyDraft` 生成。
5. **承認:** 田上さんが UI で確認 → 承認 / 編集 / 却下（編集差分は学習に反映）。
6. **送信:** 承認済みをアダプタ経由で送信（LINEは手動コピペ補助）。結果と接触履歴を記録。

---

## 4. 取得方式（デスクトップアプリ・Windows）

> 実機調査の詳細は **[INVESTIGATION.md](INVESTIGATION.md)** を参照。要点を以下に反映。

| チャネル | 受信 | 送信 | 実現性 | 方式 |
|---|---|---|---|---|
| **メール (Outlook)** | **UIA**（新Outlook） | 下書き作成→送信 | ◎ | **新Outlook UIA に決定**（従来版はアカウント未設定でCOM不可。実機で244要素確認） |
| **Slack デスクトップ** | レンダラのUIAツリー | 入力欄へ入力+Enter | ○ | **UIA**（実機で265要素確認・レンダラ起動要） |
| **LINE デスクトップ** | **OCR**（実機実証） | 入力注入(SendInput)/貼付+Enter | △ | **Qt製**。ローカルDBは暗号化で直読み不可 → **OCR受信＋入力注入送信** |

### 共通テクニック（実機検証済み）
Chromium系(新Outlook/Slack)は既定でWeb内部a11yツリー非構築。**(1) UIAクライアントとして常駐**＋**(2) `Chrome_RenderWidgetHostHWND` に `WM_GETOBJECT(OBJID_CLIENT)` 送信** で展開できる。LINEはQt製のため本テクは不可。

### 4.1 メール: Outlook + COM（最も堅牢）
- Rust から COM (`windows` crate / IDispatch 遅延バインド) で `Outlook.Application` を操作。
- 受信: `GetNamespace("MAPI")` → 受信トレイ `Items` を走査（Subject / Body / 差出人 / 受信日時 / スレッド）。
- 送信: `CreateItem(olMailItem)` → 宛先・本文設定 → `Save()`（下書き）/ 承認後 `Send()`。
- **前提:** COM は**従来版Outlook**専用。新しいOutlook(Windows標準)はCOM非対応 → 採用する場合は従来版に切替が必要（§15 で確認）。

### 4.2 Slack: デスクトップ UI Automation
- Slack デスクトップ（Electron）の画面を Windows UI Automation で読む。
- 受信: 対象ウィンドウ → 要素ツリーを辿り、メッセージのテキスト要素を抽出。
- 送信: メッセージ入力欄要素にフォーカス → 値設定 or キーストローク → Enter。
- **制約:** 見えている範囲しか取れない・UI変更で壊れやすい・送信時にPC操作を専有。クラウドAPIより脆い前提で運用（差し替え用に Slack API アダプタも将来追加可能な抽象化にする）。

### 4.3 LINE: デスクトップ UI Automation（受信のみ自動、送信は手動補助）
- 個人LINEに公式APIは無いため、デスクトップアプリ画面を UIA で読む。
- 受信: 表示中のトークから新着テキストを抽出。
- 送信: **自動送信はせず**、生成した下書きを提示 → 田上さんが手動コピペ送信（規約・凍結リスク回避）。自動化は合意後に限り検討。

> すべてのアダプタは共通トレイト `ChannelAdapter` を実装し、後で「デスクトップ↔API」を差し替えられるようにする。

---

## 5. 営業ロール特化

### 営業向け分類（TodoCategory）
- `NewLead`（新規リード）/ `Opportunity`（商談中）/ `FollowUp`（フォロー要・放置検知）/ `Proposal`（見積・提案）/ `PostSale`（受注後フォロー）/ `Internal`（社内）/ `Fyi` / `Ignore`

### 営業向けの優先度ロジック
- ホットリード・未返信の経過時間・商談ステージ・相手の重要度（関係グラフ由来）で優先度付け。
- **放置アラート:** 一定期間返信していない重要コンタクトを検知して TODO 化。

### 返信トーン
- 文体プロファイルに加え、営業文脈（初回接触/フォロー/クロージング）に応じた言い回しを切替。

### 営業資料（Google Drive 連携）
- 提案書・見積・カタログ等を **Google Drive** に置き、連携API（本セッションで接続済）で検索・取得・添付。
- 用途: 返信時に該当資料をAIが探して**下書きに添付/リンク提案**、商談ステージに応じた資料サジェスト。
- スクレイピング不要で安定。最初は「資料検索→下書きにリンク添付」から。

---

## 6. 人間関係の可視化（軽量CRM＋関係グラフ）— 徐々に育てる

### モデル
- `Contact`（人物：氏名・会社・役職・チャネル別ID・重要度スコア）
- `Company`（会社：名称・業種・関連コンタクト）
- `Interaction`（接触履歴：いつ・どのチャネル・要約・方向[受信/送信]）
- `Relationship`（関係：田上↔コンタクト、強さ＝接触頻度/直近性、温度＝商談ステージ）

### 育て方（gradual）
- メッセージごとに Gemini で人物/会社/商談を抽出し、既存コンタクトに名寄せ（メール/表示名で突合）。
- 接触のたびに `Interaction` を追加 → 関係の「強さ・鮮度」を再計算。
- 最初は薄いグラフから始め、やり取りが増えるほど自動で密になる。

### 可視化
- 関係グラフ（ノード=人物/会社、エッジ=接触の強さ）を **Web UI**（`axum`＋簡易フロント、iPhoneから閲覧可）で表示。
- コンタクト詳細：直近のやり取り・未対応TODO・商談ステージを一覧。

---

## 7. 技術スタック（Rust / Windows）

| 用途 | クレート | 備考 |
|---|---|---|
| 非同期ランタイム | `tokio` | 実行基盤 |
| Windows COM/UIA | `windows`(windows-rs) | Outlook COM・UI Automation |
| UIA 簡易ラッパ(任意) | `uiautomation` | 要素探索を楽に |
| LLM | `reqwest` → **Gemini REST** | `generativelanguage.googleapis.com`。差し替え可能な `Llm` trait |
| Webhook/UI サーバ | `axum` | 承認UI・関係グラフ表示 |
| 永続化 | `sqlx` + SQLite | 軽量・運用容易 |
| シリアライズ | `serde` / `serde_json` | API・設定 |
| 設定 | `figment` + TOML | 環境別設定 |
| 機密情報 | `keyring`（OSキーチェーン）/ env | APIキー・資格情報 |
| TUI 承認 | `ratatui` + `crossterm` | ローカル承認（任意） |
| ログ | `tracing` | 観測性・監査 |
| エラー | `anyhow` / `thiserror` | |

- **Gemini:** APIキー方式の REST 呼び出しが簡単。`Llm` trait で抽象化し、将来 Claude 等に差替可能。

---

## 8. ワークスペース構成（Cargo workspace）

```
tagamiAi/
├── Cargo.toml                # [workspace]
├── crates/
│   ├── core/                 # ドメインモデル + トレイト
│   ├── store/                # SQLite 永続化 (sqlx)
│   ├── llm/                  # Gemini クライアント（Llm trait）
│   ├── desktop-outlook/      # Outlook COM アダプタ（メール）
│   ├── desktop-slack/        # Slack UIA アダプタ
│   ├── desktop-line/         # LINE UIA アダプタ
│   ├── triage/               # 営業分類エンジン
│   ├── relationship/         # 関係抽出・名寄せ・スコア（CRM）
│   ├── responder/            # 文体プロファイル + 下書き生成
│   ├── web/                  # axum: 承認UI + 関係グラフ可視化
│   └── app/                  # daemon + オーケストレーション
└── bin/
    ├── tagami-daemon/        # 常駐プロセス
    └── tagami-cli/           # 承認TUI / 運用コマンド
```

---

## 9. ドメインモデル（抜粋・擬似コード）

```rust
pub enum Channel { Outlook, Slack, Line }

pub struct IncomingMessage {
    pub id: MessageId, pub channel: Channel, pub thread_ref: String,
    pub sender: ContactRef, pub body: String,
    pub received_at: DateTime<Utc>, pub raw: serde_json::Value,
}

pub enum TodoCategory { NewLead, Opportunity, FollowUp, Proposal, PostSale, Internal, Fyi, Ignore }
pub struct Todo { pub id: TodoId, pub message_id: MessageId, pub category: TodoCategory,
    pub priority: u8, pub summary: String, pub status: TodoStatus }

pub struct ReplyDraft { pub id: DraftId, pub todo_id: TodoId, pub text: String,
    pub model: String, pub status: DraftStatus }

// 関係グラフ
pub struct Contact { pub id: ContactId, pub name: String, pub company: Option<String>,
    pub channel_ids: Vec<(Channel, String)>, pub importance: f32 }
pub struct Interaction { pub contact_id: ContactId, pub channel: Channel,
    pub direction: Direction, pub summary: String, pub at: DateTime<Utc> }
pub struct Relationship { pub contact_id: ContactId, pub strength: f32, pub stage: DealStage,
    pub last_contact: DateTime<Utc> }

#[async_trait]
pub trait ChannelAdapter {
    fn channel(&self) -> Channel;
    async fn fetch_new(&self) -> Result<Vec<IncomingMessage>>;
    async fn send_reply(&self, thread_ref: &str, text: &str) -> Result<SendOutcome>;
}

#[async_trait]
pub trait Llm {                  // Gemini 実装。後で差し替え可能
    async fn classify(&self, msg: &IncomingMessage) -> Result<Classification>;
    async fn extract_entities(&self, msg: &IncomingMessage) -> Result<Entities>;
    async fn draft_reply(&self, ctx: &ReplyContext) -> Result<String>;
}
```

---

## 10. 永続化（SQLite スキーマ概略）

```sql
CREATE TABLE messages   (id TEXT PK, channel TEXT, thread_ref TEXT, sender TEXT, body TEXT,
                         received_at TEXT, raw JSON, processed INTEGER DEFAULT 0);
CREATE TABLE todos      (id TEXT PK, message_id TEXT, category TEXT, priority INTEGER,
                         summary TEXT, status TEXT);
CREATE TABLE drafts     (id TEXT PK, todo_id TEXT, text TEXT, model TEXT, status TEXT, created_at TEXT);
CREATE TABLE contacts   (id TEXT PK, name TEXT, company TEXT, importance REAL);
CREATE TABLE channel_ids(contact_id TEXT, channel TEXT, ext_id TEXT);
CREATE TABLE interactions(id TEXT PK, contact_id TEXT, channel TEXT, direction TEXT,
                         summary TEXT, at TEXT);
CREATE TABLE relationships(contact_id TEXT PK, strength REAL, stage TEXT, last_contact TEXT);
CREATE TABLE style_samples(id TEXT PK, channel TEXT, context TEXT, text TEXT);  -- 文体学習
```

---

## 11. 「徐々に学習」する仕組み

- **文体:** ファインチューニングはせず、**プロンプト＋few-shot**。田上さんの過去送信文を `style_samples` に蓄積し、類似文脈の事例を添付。
- **修正の取り込み:** 田上さんが下書きを編集した差分を `style_samples` に追加 → 次回以降に反映。
- **関係性:** 接触のたびに関係スコア・商談ステージを更新し、グラフが自然に充実。
- 段階的に「定型・低リスクは自動送信」を許可範囲で拡大（合意ベース）。

---

## 12. 承認フロー（human-in-the-loop）

- 既定: **下書き → 田上さん承認 → 送信**。全自動送信は当面しない。
- UI 候補:
  - **Web UI（推奨）:** `axum`＋簡易フロント。iPhone から承認・関係グラフ閲覧。
  - **TUI:** 開発初期のローカル承認に。
- LINE は送信自動化せず「下書き提示＋手動コピペ」。

---

## 13. セキュリティ / 運用

- APIキー・資格情報は **OSキーチェーン（`keyring`）** か暗号化ファイル。リポジトリに置かない（`.gitignore`）。
- メッセージ本文は機微情報。ローカル保存・アクセス制御前提。外部送信は **Gemini API（生成）** に限定し明示。送信内容の最小化を検討。
- 監査ログ（誰宛に何を送ったか）を `tracing`＋DBに記録。
- UI自動操作は田上さんのPCを専有しうるため、実行タイミング（夜間/手動トリガ等）を設計で配慮。

---

## 14. 段階的実装計画

### Phase 0: 基盤
- Cargo workspace 雛形、`core` モデル/トレイト、`store`(SQLite)、設定・ログ・秘密情報、`llm`(Gemini) の最小実装。

### Phase 1: メール (Outlook COM) PoC ← 最初の価値
- 受信トレイ走査 → 正規化 → Gemini 分類 → Todo。
- 文体下書き → Outlook **下書き**保存 → TUI/Web で承認 → 送信。

### Phase 2: 関係グラフ（CRM）
- メールから人物/会社/商談を抽出・名寄せ → contacts/interactions/relationships。
- Web UI で関係グラフ＋コンタクト詳細を可視化。放置アラート。

### Phase 3: Slack（デスクトップ UIA）
- Slack 画面読取＋送信。VegibusHQ で検証。

### Phase 4: LINE（デスクトップ UIA・手動送信補助）
- 表示トークの読取＋下書き提示。

### 横断: 学習ループ（編集差分の反映）、自動送信の段階的許可。

---

## 15. 未確定事項 / 確認したいこと

1. **メール方式:** 新Outlook(UIA)で進めるか、安定重視で**従来版Outlook+COM**（インストール済）にするか。UIAは実機で動作確認済だがWebView変更に弱い。
1b. **Slack対象範囲:** ワークスペースが4つ（M2Labo / やさいバス四国 / hmcomworkspace / VegiBus_HQ）。AI田上はどれを対象にするか。
2. **Gemini:** 使用モデル（例: gemini-2.x flash/pro）とAPIキーの用意。無料枠/有料枠の方針。
3. **送信主体（Slack）:** 田上さん本人として送るか、Bot/別表示か。
4. **自動送信の許容:** どのカテゴリなら全自動を許すか（当面は全件承認）。
5. **UI自動操作の脆さ・PC専有:** Slack/LINE 取得の実行タイミングと許容範囲。
6. **営業の対象範囲:** 取引先の業種・典型シナリオ（新規開拓中心か既存フォロー中心か）。
7. **文体・関係データの利用許可:** 田上さんの過去メッセージ取得方法と同意。

---

## 16. 次のアクション（提案）

1. §15 の確認（特に Outlook 版＝COM前提、Gemini APIキー）。
2. Phase 0：Cargo workspace 雛形と `core`/`store`/`llm(Gemini)` 生成。
3. Phase 1：Outlook COM でメール PoC（受信→分類→下書き→承認）。
```
