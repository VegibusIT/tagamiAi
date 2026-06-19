# AI田上 (tagamiAi)

Slack・LINE・メールを横断して受信を集約し、「TODO化 → 田上さんの文体で返信下書き → 承認 → 送信」を行う **Rust 製**の自動返信ツール。

## 概要

```
Slack / LINE / メール（受信）→ TODOリスト → 田上さんの文体で返信
```

- 言語 / ランタイム: Rust + tokio (async)
- チャネル: メール(Gmail) ◎ / Slack(VegibusHQ) ○ / LINE(個人) △
- 方針: いきなり全自動送信せず **下書き → 承認 → 送信**（human-in-the-loop）

詳細は **[設計書 (DESIGN.md)](DESIGN.md)** を参照。

## ステータス

設計フェーズ（v0.1 ドラフト）。実装は Phase 0（基盤）→ Phase 1（Gmail PoC）の順で進める予定。
