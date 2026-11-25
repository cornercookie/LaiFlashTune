# [#3] P1 — SE7 實作審查殘留缺陷追蹤 Design.md

## 目標與範圍

修正 SE7 實作審查（Codex，Spec → Code → Security 三階段）中發現、屬於**既有程式碼**、不在 SE7 變更範圍內的 15 項缺陷（`playback_pipeline.rs` 9 項、`next_track.rs` 6 項）與 2 項跨檔案安全顧慮。

**非範圍**：新功能、UI 變更、資料庫遷移、FFmpeg 版本升級。

---

## 圖：缺陷分類與依賴關係

```mermaid
flowchart TD
    subgraph PP[playback_pipeline.rs]
        PP1[played_out_frames 計數單位 — 審查確認無誤，不修]
        PP2[apply_dsp 已廢棄無呼叫 — 不修]
        PP3[boundaries Vec 無界增長]
        PP4[CancelHandle::cancel 同步阻塞 40s]
        PP5[TrackChain::take_next 清除策略與索引同步]
        PP6[NextCoordinator::set_next frozen 仍遞增 generation]
        PP7[poll_completion/take_next 雙播放風險]
        PP8[prepare_track 錯誤處理 — 兩套協調器分離]
        PP9[短檔案 collect()/wait_with_output 無界 PCM]
    end

    subgraph NT[next_track.rs]
        NT1[set_next bounded(1) 封鎖傳送]
        NT2[Drop join worker 不中斷 in-flight prepare]
        NT3[PreparedTrack.seq/token 分配與現行呼叫鏈對齊]
        NT4[wait_with_output() 無輸出上限 OOM]
        NT5[檔名 - 開頭注入 ffmpeg 選項]
        NT6[validate/precheck 分開開檔 TOCTOU]
    end

    subgraph XF[跨檔案]
        XF1[TOCTOU / reparse-point 競態 — handle 持有機制待定]
        XF2[原始路徑流入 ScanError/前端未遮蔽]
    end

    PP3 -.->|記憶體 DoS 優先| PP9
    PP4 -.->|阻塞音訊執行緒 優先| NT2
    NT5 -.->|命令注入 優先| NT4
    XF1 -.->|權限規則| NT6
    XF2 -.->|scanner.rs 修正| NT6
```

---

## 現況（附檔案與行號）

### playback_pipeline.rs

| # | 缺陷 | 檔案位置 | 現況程式碼片段 |
|---|------|----------|----------------|
| 1 | `played_out_frames` 單聲道樣本當 48kHz 立體聲幀計算，**但前端已正確換算** → 現況無進度錯誤，審查確認 | L313-319 (`add_played_out_frames` 呼叫點) | `next()` 每回傳一個 `f32` 即 `add_played_out_frames(1)`；但 [player.rs:230](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs:230>) 已將樣本數除以「取樣率 × 聲道數」換算 `position_ms`，前端直接使用此欄位，**無 2× 過報**。**不需修正計數單位**，僅需同步文件說明。 |
| 2 | `apply_dps`（已廢棄、無呼叫）傳入 mono frame；實際路徑 `apply_dsp_block()` 已正確送立體聲幀給 FxChain | L494-499 (`apply_dsp` 定義處) | `apply_dsp()` 僅有定義、無呼叫；實際路徑 [playback_pipeline.rs:374](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:374>) 呼叫 `apply_dsp_block(&[l, r], …)` 並於 L667-670 送立體聲幀。**無單聲道送入 FxChain 問題**，本項為誤判，**不修**。 |
| 3 | `boundaries` Vec 無界增長，長時間工作階段記憶體 DoS 風險 | L277-289 (`push_boundary`, `boundaries`, `clear_expired_boundaries`) | `push_boundary` 只 `push`，`clear_expired_boundaries` 只在 `sync_identity` 呼叫，若長時間不換軌（單曲循環、電台）永不清理 |
| 4 | `CancelHandle::cancel` 同步阻塞最長 40 秒，可能阻塞引擎/音訊執行緒 | L1325-1349 (`CancelHandle::cancel`) | `kill()` → 10s deadline → 最多再等 30s，全在同步 `cancel()` 內；`Drop` of `NextCoordinator` 會呼叫 `cancel()` |
| 5 | `TrackChain::take_next` 清除所有軌僅保留一軌，與註解「最多三份副本」矛盾 | L641-642, L862-877 | `self.tracks.clear()` → 推入新軌；註解說「最多三份副本」，實作保留 1 份 |
| 6 | `NextCoordinator::set_next` 在 `preparing_frozen` 拒絕請求時仍遞增 `generation`，耗費世代而不發出 prepare | L1460-1469 | `gen = fetch_add(1)` 在 `if preparing_frozen { log; return gen }` 之前 |
| 7 | `poll_completion` 與 `take_next` 操作同一槽位，可能重複發出同一軌（雙播放） | L233-251, L254-260 | `poll_completion` 放入 `slot`；`take_next` 取出 `slot`；若引擎在 `poll_completion` 後未立即 `take_next`，再次 `poll_completion` 會覆寫同一 `slot`，舊軌未被取走 → 洩漏；反之若 `take_next` 先跑，`poll_completion` 再放入 → 同一軌被取兩次 |
| 8 | `prepare_track` 錯誤被靜默捨棄，呼叫者取得 generation 但永遠 poll 到 None | L187-189 (worker 迴路) | `let result = prepare_track(...); completion_tx.send((result.clone(), gen))`；但 `result` 為 `Err` 時，`slot` 不更新，引擎持續等待該 `gen` 永不完成 |
| 9 | 短檔案 `stream.by_ref().collect()` 與 `ffmpeg wait_with_output()` 將整首 PCM 緩衝於記憶體，上限達 15 分鐘長檔閾值（約 300+ MB 立體聲） | L1615, 1653, 1709 | `collect()` 無上限；`wait_with_output()` 收集所有 stdout 到 `output.stdout` |

### next_track.rs

| # | 缺陷 | 檔案位置 | 現況程式碼片段 |
|---|------|----------|----------------|
| 1 | `set_next` 使用 `bounded(1)` 封鎖式傳送，與「只保留最新」意圖矛盾：連續呼叫會阻塞呼叫端直到 worker 消費前一個請求，而非取代 | L149, 207 (`pending_tx: Sender<PrepareRequest>`) | `bounded(1)` + `send()`（阻塞）而非 `try_send()` + 滿時 `try_recv()` 丟舊 |
| 2 | `Drop` 設定 `cancel_flag` 並 `join` worker，但 in-flight `prepare_track`（封鎖 ffmpeg/ffprobe/symphonia）從未被中斷，Drop 可能無限期阻塞 | L170-192, 273-281 | `Drop` 只 `cancel_flag.store(true)` + `drop(pending_tx)` + `join()`；`prepare_track` 內無檢查 `cancel_flag` |
| 3 | `PreparedTrack.seq`/`token` 硬編碼為 0，但**引擎實際使用 `playback_pipeline::NextCoordinator`**，其 `PrepareRequest.seq` 固定 0；`token` 已由前端 [Shell.tsx:728](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src/player/Shell.tsx:728>) 產生並傳入，**非由引擎分配** | L313-315, 394-396, 441-443 | `PreparedTrack { seq: 0, token: 0, ... }` 三處；現行協調器與呼叫鏈不匹配 |
| 4 | `wait_with_output()` 收集所有 ffmpeg stdout，無位元組上限，惡意或誤報檔案解碼於記憶體中可能 OOM | L423 (`child.wait_with_output()`) | 直接 `wait_with_output()`，無 `stdout` 大小限制 |
| 5 | 檔名若以 `-` 開頭（如 `-i`），作為 ffmpeg 參數傳入後可能被解析為 ffmpeg 選項（檔名注入） | L414 (ffmpeg args) | `.args([..., "-i", path, ...])`；`path` 為 `-i test.mp3` 會被解析為 `-i` 選項 |
| 6 | `validate_local_path`、`precheck_packet_budget` 的 `File::open`、`SymStream::open` 分開在不同時機開啟路徑，Windows 上存在 symlink/reparse-point 交替竊取窗口 | L287, 302, 306 | `validate_local_path` 開一次、`precheck_packet_budget` 再開、`SymStream::open` 第三次開 |

### 跨檔案安全顧慮

| # | 缺陷 | 相關檔案 |
|---|------|----------|
| 1 | TOCTOU / reparse-point 競態：路徑驗證與子行程建立之間未緩解，**單純保留 File handle 仍不足**——子程序仍需路徑字串並重新解析；需完整 handle 傳遞機制或開檔共享模式防護 | `next_track.rs`、`playback_pipeline.rs`、`player.rs` 的 ffmpeg/ffprobe 呼叫點 |
| 2 | 原始本地路徑（可能含使用者名稱）流入 `ScanError` 與前端，未套用與日誌相同的遮蔽處理 | `scanner.rs` |

---

## 做法與實作步驟

### 階段 0：共用輔助與測試基礎

1. **新增 `src-tauri/src/security.rs`** 集中：
   - `sanitize_path_for_log(path: &Path) -> String`：去除使用者目錄、遮蔽機敏段，供 `ScanError`、前端事件、日誌共用。
   - `open_validated_file(path: &Path) -> Result<File, String>`：**驗證並保留 handle 不關閉**，回傳 `File` 給呼叫者；後續 `precheck_packet_budget`、`SymStream::open` 均接受 `File` 而非路徑，避免重新開檔競態。
   - `spawn_ffmpeg_limited(args: &[&str], max_stdout: usize) -> Result<Vec<u8>, String>`：以 `stdout.take().take(max_stdout)` 限制輸出位元組，**並能區分正常 EOF 與超限**（超限回傳具體錯誤）。

2. **新增單元測試**：
   - `security::tests::sanitize_path_for_log`：含使用者目錄、UNC、長路徑。
   - `security::tests::open_validated_file_holds_handle`：驗證通過後 handle 不關閉、後續操作同一 handle。
   - `security::tests::spawn_ffmpeg_limited_detects_overlimit`：輸出超限時回傳錯誤而非靜默 EOF。
   - `playback_pipeline::tests::boundaries_cap`：推入超過上限確認自動裁切。
   - `next_track::tests::set_next_nonblocking_replace`：連續呼叫確認非阻塞且只保留最新。
   - `next_track::tests::filename_injection_rejected`：`-i test.mp3`、`--help` 等注入向量。

---

### 階段 1：playback_pipeline.rs（優先：記憶體 DoS、阻塞風險）

| 步驟 | 缺陷 | 具體修改 | 驗收方式 |
|------|------|----------|----------|
| 1.1 | **PP3 boundaries 無界** | - `PlaybackCore::new` 初始化 `boundaries: Mutex<VecDeque<BoundaryRecord>>`<br>- 新增常數 `MAX_BOUNDARIES: usize = 1000`（足容納數千首歌單）<br>- `push_boundary`：`push_back` 後 `while len > MAX { pop_front() }`<br>- `clear_expired_boundaries` 改用 `VecDeque::retain` | 測試推入 2000 筆 → 長度 ≤ 1000；`clear_expired_boundaries` 正確保留 current_seq 之後 |
| 1.2 | **PP4 CancelHandle::cancel 同步阻塞 40s** | - `CancelHandle::cancel` 改為**非阻塞發起**：`flag.store(true)` + `child.lock().take().map(|mut c| { c.kill(); thread::spawn(move || { wait_with_timeout(c, 40s); }) });`<br>- 新增 `CancelHandle::wait_for_recycle(timeout)` 供 Drop/回收執行緒呼叫（可選） | 內建測試：`CancelHandle::new(10s).cancel()` 立即返回；背景執行緒在 40s 內完成回收 |
| 1.3 | **PP9 短檔案 collect()/wait_with_output 無界 PCM** | - 新增 `MAX_PCM_BYTES: usize = 400 * 1024 * 1024`（400 MB，與 [player.rs:28](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs:28>) 現行上限一致）<br>- `prepare_track` symphonia 分支：使用有界讀取器逐樣本計數，**樣本上限 = `MAX_PCM_BYTES / 4`**（`f32` 4 bytes），不再除以聲道數<br>- 超限時回傳 `Err("PCM size exceeds limit")`，**不靜默截斷**<br>- `prepare_with_ffmpeg` 短檔分支：改用 `spawn_ffmpeg_limited(..., MAX_PCM_BYTES)`，實作為：**讀取 stdout 並計數，多讀 1 byte（`read_exact(&mut [0u8; 1])`）判斷是否還有資料**；若超限或多讀到資料則 `kill()` 子程序、回傳 `Err("PCM size exceeds limit")`；正常 EOF 才回傳 `Ok(samples)` | 測試：450 MB 偽造輸入 → 準備失敗（非截斷）；300 MB 正常 → 準備成功；恰好 400 MB → 正常成功 |
| 1.4 | **PP1 played_out_frames 計數單位** | **審查確認：現況無進度錯誤，不修正計數單位**<br>- 前端已用 `sample_rate × channels` 換算 `position_ms`<br>- 僅同步文件說明：`played_out_frames` 為「已播輸出樣本數」非幀數 | 手動驗證：播放 10 秒 48kHz 立體聲 → `position_ms ≈ 10000`、文件正確 |
| 1.5 | **PP2 apply_dsp mono frame** | **審查確認：`apply_dsp()` 已廢棄無呼叫，實際路徑 `apply_dsp_block()` 已正確送立體聲幀**<br>- 不修改舊函式；標記為誤判、不修 | 既有測試通過即驗證 |
|| 1.6 | **PP5 TrackChain::take_next 清除策略與索引同步** | - 依「最多三份副本」：**保留目前軌（若尚未耗盡）+ 新傳入的下一軌 + 最多一首預載軌**，共 ≤ 3 首<br>- 實作：先保存目前軌（`current_index < len` 且 `tracks[current_index].native_frame_pos < tracks[current_index].samples.len() as u64`）、**型別已對齊（`native_frame_pos: u64`、長度轉 `u64` 比較）**、清除 `current_index` 之前的已播軌、保留目前軌與其後至多 2 首、再推入新 `next`；`current_index` 歸零指向目前軌<br>- 程式碼：<br>  ```rust<br>  let current = (self.current_index < self.tracks.len()<br>      && self.tracks[self.current_index].native_frame_pos < self.tracks[self.current_index].samples.len() as u64)<br>      .then(|| self.tracks[self.current_index].clone());<br>  let preload = self.tracks.get(self.current_index + 1).cloned();<br>  self.tracks.clear();<br>  if let Some(c) = current { self.tracks.push(c); }<br>  if let Some(p) = preload { self.tracks.push(p); }<br>  if let Some(item) = next { self.tracks.push(item); }<br>  self.current_index = 0;<br>  ``` | 測試：5 首歌單、`current_index = 1`（第 2 首未耗盡）、傳入新第 6 首 → 保留第 2、3、6 首、`current_index = 0`、`current()` 指向第 2 首 |
| 1.7 | **PP6 set_next frozen 仍遞增 generation** | - 先檢查 `preparing_frozen`，再 `fetch_add`<br>- 程式碼順序對調 | 測試：`set_preparing_frozen(true)` → `set_next` 回傳 gen 不變、未送入 pending |
| 1.8 | **PP7 poll_completion / take_next 雙播放** | - 引入「槽狀態機」：`SlotState::Empty | Preparing(gen) | Ready(track) | Taken`<br>- `poll_completion`：僅在 `Empty | Preparing(gen)` 時寫入 `Ready`；`take_next`：將 `Ready` → `Taken` 並回傳，`Taken` 狀態下 `poll_completion` 拒絕寫入 | 測試：並發 `poll_completion` + `take_next` 1000 次 → 無雙播放、無洩漏 |
| 1.9 | **PP8 prepare_track 錯誤處理 — 區分兩套協調器** | - **`playback_pipeline::NextCoordinator`（引擎用）**：worker 已傳回 Err，**不改動**；引擎 `poll_completion` 收到 Err 記錄日誌、**前端僅負責顯示錯誤**（現況 `engineError`）、**不自動重排**（`Agents/service-architecture.md`：選曲屬前端，準備失敗不觸發 `enqueueNext`）<br>- **`next_track::NextCoordinator`（舊/獨立）**：worker 迴路同步修正傳回 Err（現況 L187-189 為靜默丟棄）<br>- 兩套協調器錯誤處理**分離記載**，不混淆 | 測試：故意給壞檔 → 各自 worker 回傳 Err → 對應 poll 記錄錯誤、前端顯示錯誤（引擎版） |

---

### 階段 2：next_track.rs（優先：命令注入、OOM、TOCTOU）

| 步驟 | 缺陷 | 具體修改 | 驗收方式 |
|------|------|----------|----------|
| 2.1 | **NT5 檔名注入** | - `prepare_with_ffmpeg`：`Command::new(ffmpeg).args(["-nostdin", "-v", "error", "-protocol_whitelist", "file,pipe", "-i", path, ...])`<br>- **關鍵**：在 `-i` 之後立即放 `path`，**不再有後續選項位置參數**；ffmpeg 會把 `-i` 後的第一個非選項參數視為輸入檔，**即使檔名以 `-` 開頭也不會被解析為選項**（實測確認）<br>- **移除** `if path.starts_with('-')` 額外防護（錯誤判斷） | 測試：檔名 `-i test.mp3`、`--help`、`-filter_complex` → 全部正常當作輸入檔處理 |
| 2.2 | **NT4 wait_with_output 無上限 OOM** | - 改用 `spawn_ffmpeg_limited`（階段 0 新增）<br>- `stdout` 以 `take(MAX_PCM_BYTES)` 讀取，**超限回傳具體錯誤**（非靜默 EOF） | 測試：`yes | head -c 500M | ffmpeg -i - -f f32le -` → 準備失敗、記憶體不飆升、錯誤訊息明確 |
| 2.3 | **NT1 set_next bounded(1) 封鎖傳送** | - `pending_tx: Sender<PrepareRequest>` → `bounded(1)` 保持<br>- `set_next`：改用 `try_send`；`Err(TrySendError::Full(_))` 時 `let _ = pending_rx.try_recv();` 丟舊、再 `try_send` 新<br>- 移除阻塞 `send()` | 測試：連續 10 次 `set_next` 無阻塞、只保留最後一筆 |
| 2.4 | **NT2 Drop 不中斷 in-flight prepare — 需完整取消機制** | - **統一取消控制**：每筆 `PrepareRequest` 攜帶 `cancel: Arc<AtomicBool>`（專屬取消旗標）；`CancelHandle` 直接持有**同一 `Arc<AtomicBool>`**（不再有獨立 `cancelled.lock()`），子程序登記共用同一 mutex<br>- **全域配額（4 槽）保留固定 deadline、與取消共用 mutex**：`audio_decode::FfprobeQuota` 維持 `Arc<Mutex<usize>> + Condvar`（槽位計數 0..4），**此 mutex 即為取消端設旗標並 `notify_all()` 的同一把鎖**；等待邏輯**完全對齊現有 `audio_decode.rs:200–217`**：<br>  ```rust\n  let deadline = Instant::now() + Duration::from_secs(10);\n  let mut guard = quota.mutex.lock().unwrap();\n  while *guard == 0 && !cancel.load(Ordering::Relaxed) {\n      let remaining = deadline.saturating_duration_since(Instant::now());\n      if remaining.is_zero() { return Err("ffprobe quota timeout"); }\n      guard = quota.condvar.wait_timeout(guard, remaining).unwrap().0;\n  }\n  if cancel.load(Ordering::Relaxed) { return Err("cancelled"); }\n  *guard -= 1;\n  Ok(())\n  ```\n  **關鍵**：取消端 `CancelHandle::cancel()` 在 **同一 mutex 內** `cancel.store(true, Ordering::Relaxed)` 並 `condvar.notify_all()`；等待前、醒來後均檢查取消條件，**保留總 deadline、消除喚醒遺失窗口**。<br>- **格式探測配額（新增）**：`SymStream::open()` 在獨立執行緒執行，**同樣受全域工作配額限制**（共用 `audio_decode::FfprobeQuota`，上限 4 槽）。主執行緒先取得槽位再啟動探測執行緒；**配額憑證移交探測執行緒持有，只在該工作實際退出時釋放**；**未結束的探測持續佔用槽位，防止累積**。取消/逾時只結束呼叫端等待，不強制釋放配額。<br>- **ffprobe 接入取消**：`ffprobe_json(path, cancel: &Arc<AtomicBool>)` 在啟動子程序前檢查旗標；子程序啟動後將 `Child` 登記到請求的 `CancelHandle`（同 mutex 內檢查取消再存入）；取消時 `kill()` 已登記 child<br>- **短檔 ffmpeg 接入取消**：`spawn_ffmpeg_limited(..., cancel: &Arc<AtomicBool>)` 讀取 stdout 迴圈前檢查旗標；子程序同樣登記到 `CancelHandle`；取消時 `kill()`<br>- **預檢取消**：`precheck_packet_budget()` 及其 `scan_mp4_stsz`、`scan_mp4_boxes`、`check_stsz_samples` 接受 `cancel: &Arc<AtomicBool>`，**每次讀取/解析樣本表前檢查 `cancel.load(Ordering::Relaxed)`，為真則提早回傳 `Err("cancelled")`**<br>- **解碼迴圈取消（含格式探測）**：<br>  - `SymStream::open()` 在建立解碼器、探測格式（`.format()`）階段**不可中斷**（同步呼叫，無取消點）；此階段會遍歷容器結構（MP4 需解析 `moov`），耗時不固定。設計方案：**在獨立執行緒執行 `SymStream::open()`，主執行緒以 `join_timeout(5s)` 等待**；取消時設旗標並 **直接 drop 掉 join handle**（不等待），背景執行緒持有檔案與解析資源繼續執行至結束或異常退出；Drop 時同樣 drop handle、不 `join()` 等待。**Drop 100ms 內返回不依賴背景執行緒退出**；背景資源由 OS 在進程結束時回收。<br>  - `SymStream::refill()`、`FfmpegStream` 迴圈定期檢查 `cancel_flag`，為真則提早結束<br>- **完成通道非阻塞**：worker 用 `try_send`，滿時丟棄舊請求，避免 `send_timeout` 卡住<br>- `Drop`：`cancel()`（設旗標 + `notify_all()` 喚醒配額等待 + 取得並殺掉 child）→ `drop(pending_tx)` → **worker handle 直接 drop（不 `join()`）**，Drop 100ms 內完成；背景工作獨立退出或由 OS 回收 | 測試：啟動長準備 → Drop NextCoordinator → 配額等待被喚醒、預檢被中斷、正在跑的 ffprobe/ffmpeg 被殺、解碼迴圈退出、100ms 內完成 Drop、無卡住；並發 4 筆探測中取消 1 筆 → 其他 3 筆不受影響；取消先於 wait_timeout → worker 立即返回、不等待 10 秒；配額滿載 10 秒無取消 → 返回 timeout 錯誤；取消發生於格式探測階段 → Drop 100ms 內完成、不等待 `join()`；背景探測執行緒獨立結束；反覆準備慢解析檔案 → 探測執行緒數不超過配額上限；取消後配額最終釋放 |
|| 2.5 | **NT3 PreparedTrack.seq/token 分配與現行呼叫鏈對齊** | - **現況釐清**：引擎使用 `playback_pipeline::NextCoordinator`，其 `PrepareRequest.seq` 固定 0；`token` 由前端 [Shell.tsx:728](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src/player/Shell.tsx:728>) 產生並透過 `player_set_next` 傳入<br>- **修正方案**：<br>  1. `playback_pipeline::NextCoordinator::set_next` 接受 `seq, token` 參數（由 `player.rs` 分配 seq、前端傳入 token）<br>  2. `PrepareRequest` 攜帶 `seq, token` 到 worker<br>  3. `prepare_track` 簽名加入 `seq, token`，建構 `PreparedTrack` 時帶入<br>  4. `next_track.rs` 版本同步修正（**四處**建構 `PreparedTrack` 皆帶入參數，L313/L337/L398/L444）<br>- **不改變 token 所有權**：前端仍產生 token、引擎分配 seq | 測試：準備完成的 `PreparedTrack.seq/token` 非 0、與呼叫鏈分配一致；去重正常 |
| 2.6 | **NT6 TOCTOU：handle 持有機制** | - `open_validated_file(path)` 回傳 **持有不關閉的 `File` handle**（以 `FILE_SHARE_READ` 開啟，禁止寫入/刪除共享，防止目標被替換）<br>- `precheck_packet_budget(&File)`、`SymStream::open(File)` 均接受 handle，**不再重新開檔**<br>- **檔案位置契約（新增）**：預檢完成後，**同一 handle 必須執行 `seek(SeekFrom::Start(0))` 復位至開頭**，再交給 `SymStream::open()` 或 `ffprobe_json`；`seek` 失敗視為錯誤返回，**不得依賴重新開檔復位**<br>- `ffprobe_json` 仍需路徑：在同一 handle 上呼叫 `GetFinalPathNameByHandleW` 取得**最終規範路徑**，傳給 ffprobe/ffmpeg<br>- **長檔/串流模式**：準備階段 `open_validated_file` 取得 handle，**用於預檢、ffprobe 與後續播放**，**handle 持續持有至播放結束或取消**；**不建立 FfmpegStream**，改回傳 `PreparedTrack { data: TrackData::Stream(StreamWrapper::FfmpegDeferred { path, sample_rate, channels, duration_ms, seq, token, song_id, norm_gain, generation, handle: Option<File> }) }`（**攜帶完整曲目中繼、準備時世代 `generation`、與驗證 handle**）；實際串流播放時，**同一 handle 以 `GetFinalPathNameByHandleW` 取得最終路徑**，傳給 ffprobe/ffmpeg（`-i <最終路徑>`）啟動子程序<br>- **子程序競態緩解**：核心路徑（預檢、SymStream.open）以 `FILE_SHARE_READ` 持有 handle，**已消除核心路徑競態**；子程序路徑（ffmpeg/ffprobe）仍需以路徑字串開啟，**存在理論重開檔競態**，設計階段承諾**子程序開啟瞬間仍有理論風險**，標記為需實測驗證 | 測試：驗證通過 → 惡意執行緒替換 symlink → `precheck`/`SymStream::open` 仍操作原 handle；handle 開啟期間嘗試替換檔案 → 失敗（共享違規）；長檔串流重開 handle 仍受保護；子程序路徑競態以實測為準；**預檢後 seek(0) 驗證：MP4/FLAC 正常解碼、不落入 FFmpeg 降級** |

---

### 階段 3：跨檔案安全顧慮

| 步驟 | 缺陷 | 具體修改 |
|------|------|----------|
| 3.1 | **XF1 TOCTOU / reparse-point — handle 持有機制** | - 階段 0 `open_validated_file` 回傳持有 handle（**`FILE_SHARE_READ` 禁止寫入/刪除共享**），**核心路徑（預檢、解碼）已消除重新開檔競態**<br>- `player.rs::load_at`、`playback_pipeline.rs::prepare_track`、`next_track.rs::prepare_with_ffmpeg` 全部改用 `open_validated_file` 取得 handle 傳遞下游<br>- **長檔/串流**：準備階段 `open_validated_file` 取得 handle，**用於預檢、ffprobe 與後續播放**，**handle 持續持有至播放結束或取消**；**不建立 FfmpegStream**，改回傳 `PreparedTrack { data: TrackData::Stream(StreamWrapper::FfmpegDeferred { path, sample_rate, channels, duration_ms, seq, token, song_id, norm_gain, generation, handle: Option<File> }) }`（**攜帶完整曲目中繼、準備時世代 `generation`、與驗證 handle**）；實際串流播放時，**同一 handle 以 `GetFinalPathNameByHandleW` 取得最終路徑**，傳給 ffprobe/ffmpeg（`-i <最終路徑>`）啟動子程序<br>- **串流迭代器統一契約**（新增）：**所有串流來源（Ffmpeg、SymStream）統一回傳 `StreamYield`**，不再用 `Option<f32>`。<br>  - `StreamYield::Data(f32)`：正常樣本<br>  - `StreamYield::Pending`：暫無資料，**非阻塞讀取、立即回傳**，保留來源、繼續輪詢<br>  - `StreamYield::Ended`：真正 EOF（通道關閉、收到空 chunk）或失敗，**才清除來源、觸發換軌**<br>  - **取樣端不阻塞**：`OutputProxy::next()`（持有核心鎖）改為非阻塞讀取；背景執行緒預讀緩衝、以通道送樣本，取樣端 `try_recv()` 即回傳 `Pending`/`Data`/`Ended`，**不再有 `recv_timeout` 同步等待**。<br>  - **既有 SymStream 轉接**：在背景執行緒以 `Iterator<Item=f32>` 拉取、以通道送入取樣端，轉接層把 `None` 映為 `Ended`、樣本映為 `Data`。<br>- **串流狀態結構**（新增）：`StreamTrackState { source: Box<dyn Iterator<Item=StreamYield> + Send>, sample_rate: u32, channels: u16, duration_ms: i64, song_id: i64, norm_gain: f32, path: String, decoder: String, seq: u64, token: u64 }`，**包含 seq/token**，取代原 `stream_source: Mutex<Option<Box<dyn Iterator...>>>`，讓 `sync_identity()` 能讀取完整中繼資料與身分<br>- **延後開啟流程**（不阻塞音訊執行緒，世代隔離在準備階段完成）：<br>  1. **`TrackChain::next()` 讀到 `FfmpegDeferred` 時**（`playback_pipeline.rs:943`）→ **不在音訊執行緒開啟**，改：<br>     - 取出 `path, sample_rate, channels, duration_ms, seq, token, song_id, norm_gain, generation, handle`（**世代與 handle 已隨軌保存，不再現場讀取/開檔**）<br>     - 送入 **新增 `deferred_open_tx: Sender<DeferredOpenRequest>`**（worker 執行緒專用通道），請求攜帶 `generation`（來自 `FfmpegDeferred`）與 `handle`<br>     - 回傳 `StreamYield::Pending`（輸出靜音，進入 `WaitingForNext`）<br>  2. **Worker 執行緒**（`PlaybackCore` 持有 `deferred_open_rx`）：<br>     - 收到請求 → 直接使用傳入的 `handle` 啟動 ffmpeg 子程序（`-ss` 對應 seek 位置），建立 **背景預讀執行緒**：以 `recv_timeout(100ms)` 讀取 stdout、**寫入 `crossbeam_channel::bounded(128)` 緩衝通道**；取樣端以 `try_recv()` 非阻塞讀取、回傳 `StreamYield`<br>     - 成功 → 送入 `deferred_done_tx: Sender<DeferredOpenResult>`（`Ok(DeferredOpenResult { source: Box<dyn Iterator<Item=StreamYield> + Send>, sample_rate, channels, duration_ms, seq, token, song_id, norm_gain, path, decoder, generation })` / `Err(DeferredOpenError { message, generation, path, song_id, seq, token })`）<br>  3. **引擎迴圈**（`player.rs` 主迴圈，或 `PlaybackCore::poll_deferred_open()`）：<br>     - `try_recv` `deferred_done_rx`<br>     - **過期隔離**：若 `result.generation != core.current_generation()` → 記錄並丟棄（**成功不安裝、失敗不觸發重新準備**）<br>  **暫停/停止保護（新增）**：<br>    - **Stop**：`PlayerCommand::Stop` → 清除 `deferred_open` 請求佇列、清除 `stream_state`、遞增 `core.current_generation()` 使所有 in-flight 結果失效、**不再重試**<br>    - **Pause**：`PlayerCommand::Pause` → 設定 `playback_state = Paused`；延後結果世代吻合時，**安裝來源但維持 `Paused` 狀態**，不設 `Playing`；使用者再次播放時才轉為 `Playing`<br>  - **成功且世代吻合**：按**固定順序**安裝（不呼叫 `load_at`，避免重新開檔）：<br>  1. 建立 `StreamTrackState`（用 result 所有欄位，**含 seq/token**，source 為 `Iterator<Item=StreamYield>`）<br>  2. `*core.stream_state.lock().unwrap() = Some(state)`（**先安裝新來源與完整中繼，供 `sync_identity()` 讀取**）<br>  3. **呼叫現有 `core.handoff_stream_track(result.seq, result.token)`**（新增方法，封裝完整換軌邏輯）：<br>     - `sync_identity()`（**此刻會從 `stream_state` 取得 seq/token/sample_rate/channels/song_id/norm_gain/duration_ms/path/decoder**，若 seq/token 不同才執行後續）<br>     - **同步清零 `seek_origin_ms`**（自然換軌從頭播放）；同曲 seek 則由 `load_at`/`restart_from` 重新設定指定位置，**不在 `sync_identity()` 一律清零**<br>     - `resampler.set_track(...)` 用新 sample_rate/channels（`sync_identity` 內已處理）<br>     - **設定播放狀態**：若當前為 `Paused` 則保留 `Paused`；否則設 `Playing`（離開 `WaitingForNext`）<br>  **身分比較、重取樣器切換、邊界發布、進度重置、回呼通知均在 `sync_identity()` 內完成**，不需外層再呼叫 `set_now_identity()`/`reset_played_out_frames()`<br> - **失敗且世代吻合**：完整重試流程（最多 1 次重試）：<br>  1. 記錄錯誤<br>  2. 從錯誤結果取得 `path, song_id, seq, token`（錯誤結構攜帶原請求）<br>  3. 呼叫 `coord.set_next(seq, token)` 觸發重新準備，**取得新世代 `new_gen`**<br>  3. **引擎同步更新 `next_coord_gen = new_gen`**（`player.rs:941` 對應），確保後續 `poll_completion` 以新世代接收結果<br>  4. **暫停意圖保留**：若當前 `playback_state == Paused` 則**保留 `Paused`**，不覆蓋為 `WaitingForNext`；否則設 `WaitingForNext`。**引擎進入「等待重試準備」子狀態**：輪詢 `poll_completion(new_gen)`，**僅當收到 `Ok(track)` 且 `track.generation == new_gen` 時**，才取出該準備結果（消耗待播槽）、**以該軌的中繼資料**再次派送 `deferred_open_tx`（攜帶 `new_gen`）<br>  5. **重試開啟再次失敗**：記錄錯誤、清除 `stream_state`、設定播放狀態 `Finished`、回報錯誤至前端、**不再重試**（避免無界循環）<br>  6. **重試準備失敗**：同步第 5 項終止分支 —— 清除 `stream_state`、設定播放狀態 `Finished`、回報錯誤至前端、**不再重試**<br>  7. 重試成功後依成功分支安裝（**成功分支設狀態時：依當下 `playback_state` —— 若為 `Paused` 則保留 `Paused`，否則設 `Playing`；不使用重試開始時的旗標**）<br>  **統一待播交接入口（新增，修正空鏈無法接下一首）**：<br>   - **任一時刻**引擎處於 `WaitingForNext`（含空 `TrackChain`、自然 EOF、手動換軌、暫停恢復），**同一函式 `try_consume_pending_next()`** 檢查：<br>     1. `poll_completion(current_gen)` → **僅更新 ready slot，不直接安裝/派送**（現行 `poll_completion` 已將 `track.clone()` 放入 slot）<br>     2. **`take_next()` 原子取出**：`if let Some(track) = coord.take_next()` → 以該軌中繼資料判斷並安裝（記憶體軌直接安裝、延後串流派送 `deferred_open_tx`）<br>     3. `deferred_done_rx.try_recv()` → 同「成功且世代吻合」分支安裝<br>   - **所有路徑**（`StreamYield::Ended` 清除後、`poll_deferred_open` 成功、`set_next` 完成、`Pause` 後恢復播放）均收斂到此入口，**不再分散在多處**<br>  4. **`sync_identity()` 修正**（`playback_pipeline.rs:482`）：優先讀取 `core.stream_state`（若為 `Some` 則用其所有中繼資料與 seq/token），否則回退 chain 目前軌。確保身分比較、重取樣器、邊界、進度、回呼完整同步。<br>  5. **串流 EOF 即時清除契約**（新增）：`next_stream()` 回傳 `StreamYield::Ended` 時，**引擎在取樣路徑中、回退 TrackChain 前**立即 `*core.stream_state.lock().unwrap() = None`，**再**回退 TrackChain 取樣。確保後續軌（記憶體或串流）的身分、取樣率、路徑、seek 來源均來自新軌，**不依賴 `WaitingForNext` 狀態**。<br>  6. **狀態發布與 seek 資料來源同步**：`player.rs:277` `get_current_duration/song_id`、`player.rs:638` `restart_from` 均優先讀取 `core.stream_state`（串流模式）或 `TrackChain.current()`（記憶體模式），確保畫面與 seek 指向正在播放的曲目。<br>  7. **handle 與子程序同生命週期**，播放結束或取消時一併關閉/殺掉。<br>- **子程序路徑**：核心路徑（預檢、SymStream.open）以 `FILE_SHARE_READ` 持有 handle，**已消除核心路徑競態**；子程序路徑（ffmpeg/ffprobe）仍需以路徑字串開啟，**存在理論重開檔競態**，設計階段承諾**子程序開啟瞬間仍有理論風險**，標記為需實測驗證 |
| 3.2 | **XF2 原始路徑流入 ScanError/前端** | - `scanner.rs` 所有 `ScanError { path, ... }` 的 `path` 改為 `sanitize_path_for_log(&path)`<br>- 前端事件 `scan-error`、`scan-progress` 的路徑欄位同步遮蔽<br>- 遮蔽規則：保留檔名、父目錄 1 層、磁碟機代號；使用者目錄（`C:\Users\<name>`）替換為 `%USERPROFILE%` |

---

### 階段 4：整合驗證與文件更新

1. `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過（現有 52/0 + 新增測試）。
2. 手動驗證：
   - 長工作階段（播放 100 首歌單循環 1 小時）→ `boundaries` 長度 ≤ 1000、記憶體穩定。
   - 惡意檔名 `-i test.mp3`、`--help` → 拒絕或安全處理。
   - 100 MB 偽造音訊 → 準備失敗、無 OOM。
   - Windows symlink 切換測試（需管理員）→ 驗證後開啟仍存取原目標。
   - `ScanError` 事件檢查前端顯示路徑已遮蔽。
3. 更新 `Agents/system-architecture.md`：
   - SE7 章節補充：`CancelHandle` 非阻塞、`bounded` 上限、`PreparedTrack.seq/token` 分配規則、`open_validated_file` TOCTOU 緩解。
4. 更新 `planning/ROADMAP.md`：將本任務標記完成。

---

## 資料變更

無資料庫 schema 變更。新增常數與內部資料結構（`VecDeque`、`SlotState`）皆在記憶體中。

---

## 完成條件（可量測、可重現）

| 條件 | 驗證方式 |
|------|----------|
| `cargo test --release --manifest-path src-tauri/Cargo.toml` 0 失敗 | CI / 本地執行 |
| `playback_pipeline.rs` `is_finished` 不再對空向量 panic | 單元測試 `test_empty_trackchain_finished` |
| `boundaries` 有上限（≤ 1000），長時間工作階段不洩漏 | 壓測腳本：循環播放 1000 首 → 記憶體平穩 |
| `CancelHandle::cancel` 不阻塞音訊執行緒（< 5ms 返回） | 基準測試：`cancel()` 耗時 < 5ms |
| `next_track.rs` ffprobe 0 取樣率/聲道不導致除零 | 單元測試 `test_ffprobe_zero_rate_rejected` |
| 檔名以 `-` 開頭不被解析為 ffmpeg 選項 | 單元測試 `test_filename_injection_rejected` |
| `wait_with_output` 有輸出上限（400 MB）且超限回傳錯誤 | 單元測試 `test_ffmpeg_stdout_limit` |
| `played_out_frames` 文件正確（樣本數非幀數） | 文件檢查 |
| `apply_dsp()` 標記為廢棄、無呼叫 | 程式碼搜尋確認 |
| `TrackChain::take_next` 保留目前軌+新下一軌+預載、索引歸零、`current()` 正確 | 單元測試 `test_take_next_keeps_current_next_preload_index_zero` |
| 兩套協調器錯誤處理分離、前端僅顯示錯誤不重排 | 程式碼檢查、`test_coordinator_error_handling` |
| `PreparedTrack.seq/token` 與呼叫鏈分配一致 | 單元測試 `test_prepared_track_identity` |
| `open_validated_file` 持有 handle（FILE_SHARE_READ）、核心路徑無重新開檔 | 單元測試 `test_open_validated_file_holds_handle` |
| `ffprobe_acquire_slot` 可被取消喚醒 | 單元測試 `test_ffprobe_quota_cancel_wakeup` |
| 長檔串流重開 handle 仍受保護 | 單元測試 `test_stream_reopen_handle_protected` |
| **XF1 子程序路徑殘餘風險接受** | **設計決策：核心路徑（預檢、SymStream.open）以 FILE_SHARE_READ 持有 handle 已消除競態；子程序路徑（ffmpeg/ffprobe 以路徑字串開啟）存在理論重開檔競態，Windows 支援 handle 繼承（DuplicateHandle、PROC_THREAD_ATTRIBUTE_HANDLE_LIST），但 FFmpeg/ffprobe 採用路徑字串介面無法使用同一 handle，接受殘餘風險，不再追加緩解** |
| 以上缺陷各以 `Review.md` 條目標記「已修」或「不修（附理由）」 | 產出 `Review.md` |

---

## 風險

| 風險 | 影響 | 緩解 |
|------|------|------|
| `PreparedTrack.seq/token` 分配權移交引擎（seq）與前端（token），涉及 `playback_pipeline::NextCoordinator`、`player.rs`、`playback_pipeline.rs` 三方協調 | 變更範圍大、易導致去重失效、雙播放 | 先在 `player.rs` 分配 seq、前端傳入 token → `playback_pipeline::NextCoordinator::set_next(seq, token)` → worker 透過 `PrepareRequest` 帶入 → `PreparedTrack` 帶回；**不改變 token 所有權** |
| `played_out_frames` 文件修正（樣本數非幀數）若未同步前端換算 | 前端若仍以幀換算會顯示一半進度 | 僅修正文件說明；前端現況已正確換算 `position_ms`，**無行為變更** |
| `open_validated_file` 改用 `File` handle 傳遞，`SymStream`、`precheck_packet_budget` 簽名變更 | 涉及 `audio_decode.rs`、`scanner.rs`、`next_track.rs`、`playback_pipeline.rs`、`player.rs` 多檔 | 分兩步：1) 新增 `open_validated_file` 並行，舊路徑保留；2) 逐檔遷移，編譯通過後移除舊路徑 |
| TOCTOU 子程序路徑競態無法完全靜態消除 | FFmpeg/ffprobe 仍重新解析路徑，競態窗口縮小但未消失 | `open_validated_file` 以 `FILE_SHARE_READ` 開啟 handle 期間檔案不可替換，**核心路徑競態已消除**；子程序路徑（ffmpeg/ffprobe 以路徑字串開啟）存在理論重開檔競態，**Windows 支援 handle 繼承（DuplicateHandle、PROC_THREAD_ATTRIBUTE_HANDLE_LIST），但 FFmpeg/ffprobe 採用路徑字串介面無法使用同一 handle**，接受殘餘風險，不再追加緩解 |
| Windows symlink/reparse 測試需管理員權限，CI 可能無法跑 | 無法自動化驗證 TOCTOU 緩解 | 本地手動驗證 + 文件記錄測試步驟；CI 跑單元測試模擬 `GetFinalPathNameByHandleW` 行為 |

---

## 關聯

- planning/[#2]P0-SE7-lofty解析器限制（SE7 已完成，本任務為其審查產出的殘留缺陷）
- Agents/system-architecture.md（SE7 章節、播放管線架構、權限模型）
- Agents/permissions.md（檔案系統與子行程權限規則、S1 驗證流程）
- Agents/business-logic.md（播放狀態機、下一首準備協議）