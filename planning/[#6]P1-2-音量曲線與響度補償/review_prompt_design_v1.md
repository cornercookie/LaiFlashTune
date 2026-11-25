# [#6]P1-2-音量曲線與響度補償 Design v1 — 設計審查請求

## 背景
SonicLedger：Tauri v2 + Rust + React 可攜式 Windows 音樂播放器。本任務為 `planning/P1-playback-quality/Design.md` 第 2 項「音量曲線與響度補償」。

## 當前設計
檔案：`planning/[#6]P1-2-音量曲線與響度補償/Design.md` (v1)

## v1 修正摘要
（初版，無前版修正）

## 審查重點

### Spec 階段
- 需求、產品原則、完成條件是否對得上，有沒有缺漏。
- 對數曲線 −60 dB ~ 0 dB、0% 靜音、線性切換、響度補償預設關、狀態列顯示 dB，是否皆可量測並對應 Acceptance Criteria。
- 是否違反「控制項要能驅動引擎才可以畫；做不到的功能明確寫做不到」。

### Code 階段
- 做法在現有程式與套件上是否可行。請確認以下現況行號：
  - `src-tauri/src/player.rs:926-931` `SetVolume` 目前直接 `sink.set_volume(volume.clamp(0.0, 1.0))`，線性。
  - `src-tauri/src/dsp.rs:150-164` 僅有 `Bq::lowshelf`，無 `highshelf`；響度補償需新增高頻擱架。
  - `src-tauri/src/dsp.rs:676-687` 幀迴圈順序為 `eq.process` → `fx.frame` → `clamp(-1.0,1.0)`，響度擱架應插在 EQ 與 FX 之間。
  - `src-tauri/src/models.rs:142-164` `AppConfig` 現有欄位與 `Default`。
  - `src-tauri/src/db.rs:854-887` `load_config` 缺欄保留 Default；`db.rs:916` `save_config_kv` 逐 key 寫入。
  - `src-tauri/src/commands.rs:153` `player_set_volume` 簽名；Tauri command 引數須 camelCase。
  - `src/player/Shell.tsx:1587-1592` 音量滑桿 0–1 step 0.01；`:1179` 狀態列僅顯示百分比。
  - `src/types.ts:119-148` `EngineSnapshot`，需同步新增 `volume_db`。
- 前後端 dB 單一真相來源：換算僅在 Rust 做，前端僅顯示 `EngineSnapshot.volume_db`，避免兩端各自換算脫節。
- `cargo test --release --manifest-path src-tauri/Cargo.toml` 與 `node --test "src/player/*.test.mjs"` 皆須能綠燈。

### Security 階段
- 本任務不新增檔案路徑輸入或子程序呼叫，但需確認：
  - `save_config_kv` 寫入的 `app_config` key 僅為固定兩 key，無路徑遍歷或任意鍵寫入風險。
  - `volume_curve` 僅接受 `"log"` / `"linear"`，拒絕任意字串，避免後續 `match` 跑到預設分支造成非預期行為。
  - 響度補償 dB 計算需避免 `log10(0)` 產生 `-inf` / `NaN` 流入濾波器係數。
  - 無對外網路、無 Tauri capability 變更，無登錄檔寫入。

## 輸出格式
請依序輸出 Spec、Code、Security 三個階段，每階段一個獨立的 markdown 表：

`# | 嚴重度 | 位置 | 問題 | 證據 | 建議`

嚴重度：阻擋 / 重要 / 次要 / 建議。每一條都需要證據（檔案:行號或官方文件），核對不了的標「推測」。
最後一行：`結論：通過 / 有條件通過 / 不通過`。

三階段皆 `結論：通過` 後，設計方可進入實作。
