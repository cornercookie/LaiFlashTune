# [#6]P1-2-音量曲線與響度補償 實作審查請求（Design v4）

## 背景
SonicLedger：Tauri v2 + Rust + React 可攜式 Windows 音樂播放器。本任務為 `planning/P1-playback-quality/Design.md` 第 2 項「音量曲線與響度補償」。設計審查 v3 已三階段皆通過，現進行實作審查。

## 實作摘要（Design.md v4）
- `dsp.rs`：新增 `Bq::highshelf`、`DspSource` 響度擱架（EQ 後、FX 前）
- `eq.rs`：`EqShared` 新增 loudness 與 volume_curve 欄位
- `models.rs`：`VolumeCurve` enum、`AppConfig` 新增兩欄、`PlayerCommand` 新增兩變體、`EngineSnapshot` 新增 `volume_db`/`volume_curve`/`loudness_compensation`
- `player.rs`：`AudioEngine` 新增 `volume_raw`/`volume_gain`/`cached_volume_db`、`volume_to_gain()`、`SetVolume` 曲線感知、新 command handler、`publish()` 同步 dB 與設定
- `playback_pipeline.rs`：`LoudnessShelves` 結構、EQ 後 FxChain 前插入響度擱架
- `commands.rs`：`player_set_volume_curve`、`player_set_loudness_compensation`
- `main.rs`：註冊新 command、啟動套用設定
- `build.rs`：COMMANDS 加入兩 command
- `capabilities/default.json`：權限與 command 清單
- `db.rs`：`load_config` 讀取兩新 key
- `src/types.ts`、`src/lib/api.ts`、`src/player/Shell.tsx`：前端型別、API wrapper、狀態列 dB 顯示與設定面板

驗證結果（由實作端報告，需審查者核對）：
- `cargo test --release --manifest-path src-tauri/Cargo.toml`：54 通過（含新 curve/loudness 測試）
- `node --test "src/player/*.test.mjs"`：5 組全過
- `npx tauri build`：成功產出 MSI 與 NSIS
- `git grep -n -i -e 'a[i]mp'`：空結果（無禁用詞）

## 審查階段（依序輸出 Spec、Code、Security 三個獨立表格）

每階段格式：
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議`

嚴重度：阻擋 / 重要 / 次要 / 建議。每一條都需要證據（檔案:行號或程式碼實際內容），核對不了的標「推測」。
最後一行：`結論：通過 / 有條件通過 / 不通過`。

### Spec（實作與 Design v4 對齊）
逐項比對 Planning、Design 與實際程式碼；檢查漏做、偏離、回歸與測試證據，確認完成條件皆已滿足：
- 對數曲線 0–100% → −60 dB ~ 0 dB（0% 靜音）
- 線性曲線切換
- 響度補償預設關，小音量低頻/高頻補償，100% 歸零，兩路徑（本機 `playback_pipeline.rs` 與電台 `DspSource`）
- 狀態列顯示 dB，靜音顯示「靜音」
- 兩新 Tauri command 已註冊於 main.rs、build.rs、capabilities/default.json
- `cargo test`、`node --test`、`tauri build` 全過

### Code（正確性、錯誤處理、可維護性）
- `volume_to_gain()` 邊界（0%、25%、50%、75%、100%）
- 零增益旁路不進入 `log10`
- 指令邊界僅接受 `'log'/'linear'`，否則回錯
- `volume_raw` 取代舊 `volume` 作 UI 值，`snapshot.volume = volume_raw`，Sink 一律 `volume_gain`
- 響度擱架係數來自 `EqShared`，兩路徑共用，版本號變更時 retune
- 新單元測試覆蓋 curve 節點與 0 dB 旁路
- `cargo test --release` 54 通過、`node --test` 5 組通過

### Security（路徑、子程序、授權、輸入驗證）
- 兩新 command 已註冊於 `main.rs` invoke handler、`build.rs` COMMANDS、`capabilities/default.json` 權限
- command 引數僅接受兩定義值，非法值回錯
- `load_config` 讀取兩新 key，缺欄回退 Default
- 無新增檔案路徑輸入、子程序呼叫、網路、登錄檔或權限提升

三階段皆 `結論：通過` 後，實作審查完成，可進入收尾（更新 `Agents/` 與 `planning/ROADMAP.md` 狀態）。
