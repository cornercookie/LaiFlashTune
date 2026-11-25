# [#6]P1-2-音量曲線與響度補償 Design v2 — 設計審查複查請求

## 背景
SonicLedger：Tauri v2 + Rust + React 可攜式 Windows 音樂播放器。本任務為 `planning/P1-playback-quality/Design.md` 第 2 項「音量曲線與響度補償」。這是設計審查 v2 複查（v1 見 `review_prompt_design_v1.md` / `Review.md`）。

## 當前設計
檔案：`planning/[#6]P1-2-音量曲線與響度補償/Design.md` (v2)

## v2 修正摘要（吸收 v1 所有阻擋與建議 S1）

| 原阻擋 | 修正位置 | 核心內容 |
|--------|----------|----------|
| C1 響度補償只接 DspSource，本機歌曲不套用 | Design.md 架構圖 2、第 2 點、實作步驟 5 | 響度補償同時接入本機路徑（`playback_pipeline.rs` `apply_dsp_block`/`apply_dsp_block_post`/尾段）與電台路徑（`DspSource`），係數單一來源 `EqShared` |
| C2 `volume_raw/volume_gain` 與既有 `volume` 遷移、初始化、換裝置未說明 | Design.md 第 1 點、R1、實作步驟 4 | 明定 `volume_raw`（滑桿值）/`volume_gain`（送 sink）語義；初始化（player.rs:86、112）與換裝置（player.rs:331）皆用 `volume_gain` |
| C3 持久化設定未套用至引擎啟動流程 | Design.md 第 4 點、實作步驟 7 | `main.rs` 開啟資料庫後、啟動播放器前，`load_config` 讀取兩 key 並寫入 `EqShared`/`AudioEngine`，再還原 session 與 UI |
| C4 `volume_db` 傳輸契約不成立（負無限大無法序列化） | Design.md R4、第 1 點、models.ts/TS | `volume_db: Option<f32>`（Rust `volumeDb`，靜音 `None`）/ TS `number | null`，前端顯示「靜音」 |
| C5 錯誤稱限幅為「防削波」 | Design.md 第 2 點、風險第 4 點 | 改為「限幅後可能產生削波失真，設計不承諾防削波」 |
| SE1 新 command 漏列註冊與授權 | Design.md 指令契約表、實作步驟 7–9 | 兩新 command 納入 `main.rs` invoke handler、`build.rs` COMMANDS、`capabilities/default.json` 權限 |
| SE2 command 與設定載入邊界驗證不足 | Design.md R2、第 3 點、實作步驟 6 | command 僅接受 `'log'/'linear'`，否則回錯；內部用 enum，非法值回錯/回退 Default |
| SE3 零增益分支未定義（可能進 log10） | Design.md 第 2 點、風險第 6 點 | 零增益（靜音）旁路：bass_db = treble_db = 0，不呼叫 log10；正增益才計算對數 |
| S1 響度補償驗收重現方式不足 | Design.md R5、完成條件、實作步驟 13 | 新增開關前後固定輸入輸出樣本比較測試 |

## 複查重點
請依序輸出 Spec、Code、Security 三個階段，每階段一個獨立的 markdown 表：

`# | 嚴重度 | 位置 | 問題 | 證據 | 建議`

嚴重度：阻擋 / 重要 / 次要 / 建議。每一條都需要證據（檔案:行號或官方文件），核對不了的標「推測」。
最後一行：`結論：通過 / 有條件通過 / 不通過`。

特別請確認：
1. 本機歌曲的 DSP 路徑（`playback_pipeline.rs:396、664-728、469、576、733`）是否確實套用響度擱架，與電台路徑（`player.rs:811-817` `DspSource`）係數是否共用 `EqShared`。
2. 兩新 command 是否已註冊於 `main.rs:100-127`、`build.rs:7-47`、`capabilities/default.json:24-38`。
3. `volume_db` 傳輸契約是否可序列化（`Option<f32>` / `number | null`，無 `-inf`）。
4. 零增益（0%）是否旁路，不進入 `log10`。
5. 是否還有新的阻擋。

三階段皆 `結論：通過` 後，設計方可進入實作。
