# [#6]P1-2-音量曲線與響度補償 Design v3 — 設計審查複查請求

## 背景
SonicLedger：Tauri v2 + Rust + React 可攜式 Windows 音樂播放器。本任務為 `planning/P1-playback-quality/Design.md` 第 2 項「音量曲線與響度補償」。這是設計審查 v3 複查（v1 與 v2 見 `Review.md`）。

## 當前設計
檔案：`planning/[#6]P1-2-音量曲線與響度補償/Design.md` (v3)

## v3 修正摘要（吸收 Codex 設計審查 v2）

| 原阻擋 | 修正位置 | 核心內容 |
|--------|----------|----------|
| C4 阻擋：傳輸欄名不一致（後端 `volumeDb`，前端讀 `volume_db`） | Design.md R4、第 1 點 EngineSnapshot 段、第 5 點前端 | Rust 保留 `volume_db: Option<f32>`（serde `volume_db` → JSON `volumeDb`）；TS 與前端讀取統一為 `volumeDb: number | null`，靜音 `null` 顯示「靜音」；同步修正 R4 |
| C2 重要：舊 `volume` 欄位去留與 snapshot 對應未明 | Design.md 第 1 點「新增欄位」 | 以 `volume_raw` 取代舊 `volume` 欄位作 UI 滑桿值，`snapshot.volume = volume_raw`；Sink 一律使用 `volume_gain` |

## 複查重點
請依序輸出 Spec、Code、Security 三個階段，每階段一個獨立的 markdown 表：

`# | 嚴重度 | 位置 | 問題 | 證據 | 建議`

嚴重度：阻擋 / 重要 / 次要 / 建議。每一條都需要證據（檔案:行號或官方文件），核對不了的標「推測」。
最後一行：`結論：通過 / 有條件通過 / 不通過`。

特別請確認：
1. **C4 是否關閉**：Rust `volume_db: Option<f32>`（camelCase 輸出 `volumeDb`）、TS `EngineSnapshot.volumeDb: number | null`、前端讀取 `s.volumeDb` 並用 `=== null` 判斷靜音，無 `-inf`、`undefined` 導致 `toFixed()` 失敗。
2. **C2 是否關閉**：Design 已明寫 `volume_raw` 取代舊 `volume` 作 UI 值、`snapshot.volume = volume_raw`、Sink 一律用 `volume_gain`。
3. 是否有新的阻擋（Spec/Code/Security）。

三階段皆 `結論：通過` 後，設計方可進入實作。
