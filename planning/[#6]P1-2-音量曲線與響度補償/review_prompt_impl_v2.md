# [#6]P1-2-音量曲線與響度補償 實作審查複查請求（Design v5）

## 背景
SonicLedger：Tauri v2 + Rust + React 可攜式 Windows 音樂播放器。本任務為 `planning/P1-playback-quality/Design.md` 第 2 項「音量曲線與響度補償」。這是實作審查 v5 複查（v4 見 `Review.md` 實作審查第 1 輪）。

## v5 修正摘要（吸收 v4 實作審查所有阻擋）

| 原阻擋 | 修正位置 | 核心內容 |
|--------|----------|----------|
| S1 阻擋：dB 顯示與兩個設定狀態無法同步 | `src/types.ts`、`src/player/Shell.tsx` | 前端欄名改為 Rust 輸出的 camelCase：`volumeDb`、`volumeCurve`、`loudnessCompensation`；`Shell.tsx` 狀態接收處讀取 `s.volumeDb` 等 |
| S2 阻擋：音量曲線與響度設定未持久化 | `src-tauri/src/commands.rs` | 兩個 command 處理器新增 `db_state` 參數，`send_cmd` 後呼叫 `save_config_kv("volume_curve"/"loudness_compensation", …)` |
| S3 阻擋：宣稱新增測試卻不存在 | `src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/playback_pipeline.rs` | 新增 5 個測試：curve 五節點、linear、響度 0 dB 歸零、響度關閉旁路、`LoudnessShelves` 旁路；總測試 54 → 59 |
| C1 阻擋：開啟響度補償不重算補償量 | `src-tauri/src/player.rs` | `SetLoudnessCompensation` handler 會呼叫 `update_loudness_params(&engine, engine.volume_raw)` 依當前音量重算 bass/treble；`update_loudness_params` 關閉或靜音時歸零 |
| C2 次要：`cached_volume_db` 未使用 | `src-tauri/src/player.rs` | `SetVolume` / `SetVolumeCurve` handler 更新快取；`publish()` 直接使用 `self.cached_volume_db` |

驗證結果（由實作端報告，需審查者核對）：
- `cargo test --release --manifest-path src-tauri/Cargo.toml`：59 通過（含新增 5 個測試）
- `node --test "src/player/*.test.mjs"`：5 組全過
- `npx tauri build`：成功（MSI 5.30 MiB、NSIS 3.75 MiB）
- `git grep -n -i -e 'a[i]mp'`：空結果
- `Design.md` 已更新為 v5，記錄所有實際變更

## 複查階段（依序輸出 Spec、Code、Security 三個獨立表格）

每階段格式：
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議`

嚴重度：阻擋 / 重要 / 次要 / 建議。每一條都需要證據（檔案:行號或程式碼實際內容），核對不了的標「推測」。
最後一行：`結論：通過 / 有條件通過 / 不通過`。

### Spec（實作與 Design v5 對齊，重點驗證 S1–S3 是否關閉）
- S1：前端 `EngineSnapshot` 與 `Shell.tsx` 是否讀取 `volumeDb`（camelCase），不再讀 snake_case `volume_db`？
- S2：兩個 command 處理器是否呼叫 `save_config_kv` 持久化？重開後 `load_config` 是否可恢復？
- S3：新增的 5 個測試是否確實存在並通過？
- 完成條件：對數曲線 0–100% → −60 dB ~ 0 dB（0% 靜音）；線性切換；響度補償預設關；小音量低頻/高頻補償；100% 歸零；兩路徑（本機 `playback_pipeline.rs` 與電台 `dsp.rs`）；狀態列顯示 dB；兩新 command 已註冊於 main.rs、build.rs、capabilities/default.json；`cargo test`、`node --test`、`tauri build` 全過。

### Code（重點驗證 C1、C2 是否關閉）
- C1：`SetLoudnessCompensation` handler 是否依當前 `volume_gain` 重算 bass/treble？關閉/靜音時是否歸零？
- C2：`cached_volume_db` 是否在音量／曲線變更時更新，`publish()` 是否直接使用？
- `volume_to_gain()` 五節點與 linear 測試是否通過？
- `cargo test --release` 59 通過、`node --test` 5 組通過。

### Security（重點驗證 S2 的寫入路徑）
- `save_config_kv` 寫入僅針對固定兩 key，無路徑遍歷或任意鍵寫入風險？
- command 引數值域驗證是否仍僅接受 `'log'/'linear'`？

三階段皆 `結論：通過` 後，實作審查完成，可進入收尾（更新 `Agents/` 與 `planning/ROADMAP.md` 狀態）。
