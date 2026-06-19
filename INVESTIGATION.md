# デスクトップ自動化 実機調査メモ

> 目的: Slack / LINE / メール(Outlook) の各デスクトップアプリを **UI Automation (UIA)** で安定的に読取・操作できるかを、実機(Windows 11, ryuji環境)で検証した記録。
> 調査日: 2026-06-19

---

## 1. 環境・アプリ インベントリ

| アプリ | 種別 | 識別子 / プロセス | 備考 |
|---|---|---|---|
| **新しい Outlook** | Store/WebView2(Chromium) | `Microsoft.OutlookForWindows` v1.2026.609.400 / プロセス **`olk.exe`** | 起動中・`Ryuji Yasukochi` でサインイン済み |
| **従来版 Outlook** | Win32(Office) | `C:\Program Files\Microsoft Office\root\Office16\OUTLOOK.EXE` | インストール済（未起動）。**COM自動化が可能** |
| **Slack** | Store/Electron(Chromium) | `91750D7E.Slack` v4.50.136 / プロセス **`Slack`** | ワークスペース4つ（下記） |
| **LINE** | Store/**Qt** | `LINE` v26.2.0 / プロセス **`LINE`** | Qt製。UIA対応が弱い見込み |

### Slack ワークスペース（4つ検出）
`M2Labo` / `やさいバス四国` / `hmcomworkspace` / **`VegiBus_HQ`**
→ 「VegibusHQだけ？」への答え: **No、複数ある**。AI田上の対象範囲は要決定（§5）。

---

## 2. 核心テクニック: Chromium のアクセシビリティを起こす

WebView2 / Electron(Chromium)系アプリは、**既定ではWeb内部のa11yツリーを構築しない**（省メモリのため）。支援技術(AT)クライアントの存在を検知して初めて構築する。実機で確認した起動手順:

1. **UIAクライアントとして常駐する** — `AddAutomationFocusChangedEventHandler` 等のイベントハンドラを登録（ATクライアントとみなされる）。
   - → **新Outlook(WebView2)** はこれだけで起動（19要素 → **244要素**に展開）。
2. **レンダラ子ウィンドウに `WM_GETOBJECT` を送る** — 子ウィンドウ `Chrome_RenderWidgetHostHWND` に `SendMessage(WM_GETOBJECT=0x3D, 0, OBJID_CLIENT=-4)`。
   - → **Slack(Electron)** はこれで起動（14要素 → **265要素**に展開）。

実装では **両方を併用**するのが堅実（登録 → 子ウィンドウ列挙 → 各 RenderWidgetHost に WM_GETOBJECT → 短時間待ち → ツリー読取）。

3. **頑固な WebView2 には `UiaRootObjectId = -25` を使う** — `OBJID_CLIENT(-4)` で開かないアプリ（**Copilot for Windows** が該当）には、`SendMessage(WM_GETOBJECT, 0, -25)`（UIA専用ルートID）を窓＋全子窓に送ると開く。実機で Copilot が 12 → **111要素**に展開し、入力欄 `[編集]'Copilot へメッセージを送る'`(ValuePattern) と会話本文を取得確認。
   - 併せて構造変更イベントハンドラ（`AddStructureChangedEventHandler`）を張るとATシグナルが強まる。

---

## 3. アプリ別 検証結果

### 3.1 新しい Outlook (`olk.exe`) — ◎ 読取可
- ATクライアント登録後、サブツリー **244要素**。
- 読めたUI: `New` / `Delete` / `Archive` / **`Reply`** / **`Reply all`** / `Move` / `Categorize` などのボタン、本文域は「ドキュメント」要素で **ValuePattern + TextPattern 対応**。
- 返信ボタンが押せ、本文域が読める/書ける見込み → **操作可能性が高い**。
- **実証(深掘り):** フォルダツリー（Inbox 2914 unread / Junk[652] / Drafts[16]…、アカウント=`ryuji.yasu@gmail.com`）と、メッセージ一覧の各行から **差出人・件名・時刻・プレビュー・未読状態** を取得確認。本文は `TextPattern.DocumentRange.GetText()` で取得可（特定メールはそれを選択/展開してから読む）。
- → **メール経路は受信読取・分類・返信操作すべて UIA で到達可能**と確認（PoC最大リスク解消）。
- 補足: 新Outlookの当該アカウントは Gmail(`ryuji.yasu@gmail.com`) を集約表示 → 望めば将来 Gmail連携への切替も容易（今回はUIAで確定）。

### 3.2 Slack (`Slack` / Electron) — ○ 読取可（要レンダラ起動）
- `Chrome_RenderWidgetHostHWND` に WM_GETOBJECT 送信後、レンダラのサブツリー **265要素**。
- 読めたUI: アクティブ文書(ValuePattern)、ワークスペースタブ、ナビ（ホーム/DM/アクティビティ/ファイル/後で/管理者）、アクティビティタブ（すべて/DM/メンション/スレッド）。
- メッセージ本文・入力欄・送信ボタンの読取/操作は次段で要確認。

### 3.3 LINE (`LINE` / Qt) — △ 最難関
- LINEは **Qt製**（窓クラス `Qt663...`）。Electron/WebViewの起動テクは効かない。
- 調査時はトレイ格納でメインウィンドウ非存在（取得できたのはトレイ/IME用の不可視窓のみ）。
- Qtアプリは一般にUIA公開が貧弱 → **安定したUIA読取は期待薄**。現実解は:
  - **受信:** OCR（画面キャプチャ＋文字認識）または Qt accessibility の限定利用。
  - **送信:** 自動送信せず **下書き提示＋手動コピペ**（規約・凍結リスクも回避）。
- 要追加調査: LINEメインウィンドウを開いた状態でのUIA公開度（次回）。

---

## 4. 安定性の評価（ユーザ要件「安定させる」に対して）

| チャネル | 最も安定な方式 | 代替（UIA一本化希望時） |
|---|---|---|
| **メール** | **従来版 Outlook + COM**（堅牢・推奨） | 新Outlook UIA（起動テク要・WebView変更に弱い） |
| **Slack** | Slack Web API（最堅） | デスクトップ UIA（レンダラ起動要・実用可） |
| **LINE** | （公式API無し） | UIA困難 → OCR読取 + 手動送信 |

> 「安定」を最優先するなら、メールは**従来版Outlook(COM)**、Slackは可能なら**API**が王道。全アプリUIA統一でも Outlook/Slack は実用可能と確認できたが、LINEだけは別方式(OCR/手動)が現実的。

---

## 5. Rust 実装へのマッピング

- **UI Automation:** `windows` クレート `Windows::Win32::UI::Accessibility`（`CUIAutomation`, `IUIAutomation`, `IUIAutomationElement`, `TreeScope_Subtree`, `CreateTrueCondition`, `FindAll`）。ATクライアント化はイベントハンドラ登録で。
- **子ウィンドウ操作:** `Windows::Win32::UI::WindowsAndMessaging`（`EnumChildWindows`, `GetClassNameW`, `SendMessageW(WM_GETOBJECT, 0, OBJID_CLIENT)`）。
- **読取:** `IUIAutomationTextPattern` / `get_CurrentName`。**入力:** `IUIAutomationValuePattern::SetValue`。**ボタン:** `IUIAutomationInvokePattern::Invoke`。
- **メール(従来版Outlook COM案):** `windows` の COM(`IDispatch`)で `Outlook.Application` → `GetNamespace("MAPI")` → Items 走査 / `CreateItem` → Save/Send。
- **LINE(OCR案):** 画面キャプチャ + OCR（Windows.Media.Ocr or Tesseract）。

---

## 6. 残タスク（次の調査・検証）

1. **新Outlook:** メール一覧行（差出人/件名）と本文の深い読取、下書き作成→送信の一連動作を実証。
2. **Slack:** メッセージ本文・入力欄(ValuePattern)・送信の実証。対象ワークスペース確定後に実機検証。
3. **LINE:** メインウィンドウを開いた状態でUIA公開度を再確認 → 不可なら OCR PoC。
4. **安定運用:** UIA読取のリトライ/タイムアウト戦略、アプリ前面化の扱い、実行タイミング（PC専有の回避）。

---

## 7. ローカルデータ保存先 調査（実機）

「データはどこかに保存されているはず」を検証 → **3アプリとも本体データはディスク上に存在**を確認。

| アプリ | 保存先 | 形式 | 読取可否 |
|---|---|---|---|
| **Slack** | `…\Packages\91750D7E.Slack…\LocalCache\Roaming\Slack\IndexedDB\https_app.slack.com_0.indexeddb.leveldb` | Chromium **IndexedDB(leveldb)** | 要leveldb読取＋V8デシリアライズ |
| **新Outlook** | `%LOCALAPPDATA%\Microsoft\Olk\EBWebView\Default\IndexedDB\https_outlook.office.com_0.indexeddb.leveldb`（+ `Attachments\` に添付実体, 計443MB） | Chromium **IndexedDB(leveldb)** | 同上 |
| **LINE** | `%LOCALAPPDATA%\LINE\Data\db\qw….edb`（+ keep_/album_/chatStats_/AutoSuggest, 計490MB） | `.edb`拡張子だが**ESE非標準＝ファイル全体を暗号化**（実機確認: ヘッダがESE署名でない・平文ゼロ・高エントロピー。ロックは無く共有読取は可） | **暗号鍵が必要**＝最難 |

### 制約（実機で判明）
1. **排他ロック:** アプリ起動中は leveldb を開けない（Outlookで `being used by another process` を実証）。→ 読むには**アプリ終了**か**ファイルコピー/VSSスナップショット**。
2. **フォーマット:** IndexedDBの値は**V8シリアライズ/Snappy圧縮**で平文ではない → デシリアライザ実装が必要。
3. **暗号化:** LINE(.edb)は本文暗号化の見込み。
4. **無契約・版依存:** 内部フォーマットはアプリ更新で変わりうる。

> 収穫: **Slackと新Outlookは同一のChromium IndexedDB形式** → Rustで共通リーダーを作れば両対応可能。ただしライブ運用はロックがネック。

---

## 8. 手段 × アプリ 比較と推奨

| | UIA(ライブ) | OCR(ライブ) | ローカルキャッシュ |
|---|---|---|---|
| **新Outlook** | ◎ 実証244要素・Replyボタン可 | ○ | △ leveldb(ロック/要デシリアライズ) |
| **Slack** | ○ 実証265要素(要レンダラ起動) | ○ | △ leveldb(同上) |
| **LINE(Qt)** | △ 窓を開けて要再確認 | **◎ OCR実証(日本語可)** | ✕ ESE暗号化で最難 |

### Copilot for Windows（エンジン）— UI自動化 実証
- アプリ実体: `mscopilot.exe`（WebView2/Chromium）、窓 `Chrome_WidgetWin_1` title='Copilot'。
- `WM_GETOBJECT(-25)` 起こしで **111要素**展開。**入力欄=ValuePattern（プロンプト投入可）／回答=ドキュメント要素（読取可）** を確認。
- → **Slack(UIA) ⇄ Copilot(UIA) の自動往復ループが全部品実証済み**。エンジンは Copilot for Windows（UI駆動）で確定。Llm trait で Gemini にも差し替え可能。
- 知識ベース（文体・関係・ログ）は **Google Drive 格納**（ディスク圧迫回避）。

**推奨アーキテクチャ（安定重視）:**
- **新Outlook / Slack = UIA でライブ読取＋送信**（ロック回避・ユーザ使用中も動く）。キャッシュは「過去ログ一括取得」用途でアプリ終了時に補助利用。
- **LINE = OCR で読取 ＋ 入力注入(SendInput)/クリップ貼付で送信**。窓を開けた状態でのQtのUIA公開度は次回確認。
