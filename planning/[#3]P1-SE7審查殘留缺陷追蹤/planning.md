# [#3] P1 — SE7 實作審查殘留缺陷追蹤

## 來源
SE7 lofty 解析器限制（planning/[#2]P0-SE7-lofty解析器限制）的獨立實作審查（Codex，Spec → Code → Security 三階段）。SE7 阻礙發現已全數修正並驗證（`cargo test --release` 52/0 通過），但審查同時揭露若干屬於**既有程式碼**、不在 SE7 變更範圍內的缺陷，故另立此任務追蹤，後續獨立處理。

## 任務類型
P1 既有缺陷修正（非新功能）

## 現況（附檔案與行號）

### next_track.rs — 除零風險（已修正）
- `prepare_with_ffmpeg()`：ffprobe 回報的 `sample_rate`/`channels` 可能為 0，直接用於 `samples.len() / (sample_rate * channels)` 除法會產生 inf/NaN。**已修正**：兩者任一為 0 即回傳 `Err` 拒絕建立 `PreparedTrack`（src-tauri/src/next_track.rs:379-385）。

### playback_pipeline.rs — 空向量恐慌（已修正）
- `TrackChain::is_finished()`：`self.tracks.len() - 1` 在 `tracks` 為空時會 underflow（debug panic、release 繞位）。**已修正**：加入 `self.tracks.is_empty()` 短路判斷（src-tauri/src/playback_pipeline.rs:693-695）。

### playback_pipeline.rs — 既有缺陷（待修正）
1. `played_out_frames` 單聲道樣本當 48kHz 立體聲幀計算，進度過報約 2×（線 313-319）。
2. `apply_dps` 傳入單聲道 mono frame `[s]` 給 `FxChain::frame`（期待所有聲道），立體聲延遲線通道佈局錯誤（線 494-499）。
3. `boundaries` Vec 無界增長，長時間工作階段記憶體 DoS 風險（線 277-289）。
4. `CancelHandle::cancel` 同步阻塞最長 40 秒，可能阻塞引擎/音訊執行緒（線 1325-1349）。
5. `TrackChain::take_next` 清除所有軌僅保留一軌，與註解所述最多三份副本的生命週期契約矛盾（線 641-642 與實作不符）。
6. `NextCoordinator::set_next` 在 preparing_frozen 拒絕請求時仍遞增 generation，耗費世代而不發出 prepare（線 1460-1469）。
7. `poll_completion` 與 `take_next` 操作同一槽位，可能重複發出同一軌（雙播放）（線 233-251, 254-260）。
8. `prepare_track` 錯誤被靜默捨絕，呼叫者取得 generation 但永遠 poll 到 None（線 187-189）。
9. 短檔案 `stream.by_ref().collect()` 與 `ffmpeg wait_with_output()` 將整首 PCM 緩衝於記憶體，上限達 15 分鐘長檔閾值（約 300+ MB 立體聲）（線 1615, 1653, 1709）。

### next_track.rs — 既有缺陷（待修正）
1. `set_next` 使用 bounded(1) 封鎖式傳送，與「只保留最新」意圖矛盾：連續呼叫會阻塞呼叫端直到 worker 消費前一個請求，而非取代（線 149, 207）。
2. `Drop` 設定 cancel_flag 並 join worker，但 in-flight `prepare_track`（封鎖 ffmpeg/ffprobe/symphonia）從未被中斷，Drop 可能無限期阻塞（線 170-192, 273-281）。
3. `PreparedTrack.seq` 與 `token` 硬編碼為 0，雖然註解稱由引擎分配，導致引擎端以 seq/token 做去重失效（線 313-315, 394-396, 441-443）。
4. `wait_with_output()` 收集所有 ffmpeg stdout，無位元組上限，惡意或誤報檔案解碼於記憶體中可能 OOM（SE6/Imp#7 阻塞仍待解決；線 423）。
5. 檔案名稱若以 `-` 開頭（例如 `-i`），作為 ffmpeg 參數傳入後可能被解析為 ffmpeg 選項（檔名注入；線 414）。
6. `validate_local_path`、`precheck_packet_budget` 的 `File::open`、`SymStream::open` 分開在不同時機開啟路徑，Windows 上存在 symlink/reparse-point 交替竊取窗口（線 287, 302, 306）。

### 跨檔案安全顧慮（待修正）
1. TOCTOU / reparse-point 競態：路徑驗證與子行程建立之間未緩解，Windows 符號連結/接合點可竄改存取目標（`next_track.rs`、`playback_pipeline.rs`、`player.rs` 的 ffmpeg/ffprobe 呼叫點）。
2. 原始本地路徑（可能含使用者名稱）流入 `ScanError` 與前端，未套用與日誌相同的遮蔽處理（`scanner.rs`）。

## 做法與實作步驟（建議順序）
1. 獨立開立本任務的 Design.md（依 AGENTS.md 流程：目標、範圍、現況、做法、完成條件、風險）。
2. 依序修正 `playback_pipeline.rs` 的 9 項缺陷，優先處理記憶體 DoS（boundaries 無界、短檔案無界 PCM）與阻塞風險（CancelHandle 40 秒）。
3. 修正 `next_track.rs` 的 6 項缺陷，優先檔名注入與 OOM 風險。
4. 跨檔案：補強 `validate_local_path`（C2，symlink/reparse 防護）與路徑遮蔽。
5. 每項修正後執行 `cargo test --release --manifest-path src-tauri/Cargo.toml` 確認無回歸。
6. 更新 `Agents/system-architecture.md` 與 `planning/ROADMAP.md`。

## 資料變更
無。

## 完成條件（可量測、可重現）
- `cargo test --release --manifest-path src-tauri/Cargo.toml` 0 失敗。
- `playback_pipeline.rs` `is_finished` 不再對空向量 panic；`boundaries` 有上限；`CancelHandle::cancel` 不阻塞音訊執行緒。
- `next_track.rs` ffprobe 0 取樣率/聲道不導致除零；檔名以 `-` 開頭不被解析為 ffmpeg 選項；`wait_with_output` 有輸出上限。
- 以上缺陷各以 `Review.md` 條目標記「已修」或「不修（附理由）」。

## 風險
- `next_track.rs` `seq`/`token` 硬編碼為 0 與 `set_next` 封鎖傳送涉及 `NextCoordinator` 與引擎身分配合，變更範圍較大，需與 player.rs 核對後再動。
- `playback_pipeline.rs` `played_out_frames` 計算變更會影響前端進度顯示，需同步驗證前端進度條。
- 所有子行程路徑變更需在 Windows 上驗證 symlink/reparse 防護不誤殺正當路徑。

## 關聯
- planning/[#2]P0-SE7-lofty解析器限制（SE7 已完成，本任務為其審查產出的殘留缺陷）
- Agents/system-architecture.md（SE7 章節）
- Agents/permissions.md（檔案系統與子行程權限規則）
