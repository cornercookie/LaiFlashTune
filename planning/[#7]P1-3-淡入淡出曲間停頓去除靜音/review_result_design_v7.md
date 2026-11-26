本輪唯讀 Spec 複查，未修改 repo、未執行測試或建置；Design 雜湊前後一致。指定 `git grep` 及未追蹤 Design 的補充搜尋皆無命中。

R1–R6 涵蓋 Planning 功能需求；九個設定 key、Default、互斥持久化、三處指令註冊及測試／建置均已列入設計。以下僅列 v7 修正 #7 後仍存在的交接契約問題。

**D**＝[Design.md](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md>)；Rust 檔案位於 `src-tauri/src/`。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| 7a | 阻擋 | D:161、222；對照 D:147 | **將預載完成當成播放交接時機，會提前切入下一首。** 未限定目前軌結束及 gap 耗盡，也無暫停狀態限制。 | `playback_pipeline.rs:1891–1900` 的 `poll_completion` 收到準備結果就入槽並回傳，沒有等待曲終；`:405–408` 優先讀取串流來源。依新增步驟推論，A 尚在播放時 B 準備完成，即會安裝 B 並 `sink.play()`，違反 D:147「A 結束→gap→B」及 Planning:20 的曲間停頓要求。 | `poll_completion` 僅保存準備結果；由曲終／gap 耗盡的交接通知觸發命令迴圈取走待播資料，再安裝播放。明確保留暫停狀態與世代檢查。 | #7 尚未關閉，須修正觸發時機。 |
| 7b | 重要 | D:161、222、273 | **新增偏移換算與計數器單位不一致。** `rate × channels` 未區分原檔格式與內部格式。 | `PreparedTrack.rate/channels` 來自原檔（`playback_pipeline.rs:1989–1990`）；`played_out_frames` 在固定 48 kHz 雙聲道層累計（`:349–357`），`player.rs:275` 固定除以 `48000 × 2`。若按 44.1 kHz 雙聲道換算 3 秒偏移，寫入 264,600，讀回僅 **2756 ms**，不符合 Planning:27 的 0:03。 | 明定使用 `position_ms × 48000 × 2 / 1000`；或以 `seek_origin_ms` 保存偏移、計數器歸零。須避免與 leading 偏移重複相加。 | 新增公式須修正。 |
| 7c | 重要 | D:161、222 | **安裝串流來源尚未包含新軌重取樣器與歌曲身分更新。** 僅列出的操作不足以完成交接。 | `set_stream_source` 只替換 iterator（`playback_pipeline.rs:498–499`）；現有直接載入另設定新軌 rate/channels（`player.rs:468–485`）。自動身分同步則取自 TrackChain（`playback_pipeline.rs:507–543`），但 D:161 明定串流不進 TrackChain。依此流程推論，可能沿用 A 的重取樣參數；Seek 又優先讀取 A 的 TrackChain 路徑（`player.rs:702–712`）。 | 在同一交接步驟列明：清除舊 TrackChain、以 B 的格式設定重取樣器、更新 seq/token、歌曲資料及目前路徑，並銜接既有邊界與進度更新流程。 | #7 的安裝契約仍須補齊。 |

以上為文件與原始碼的靜態核對及流程推論，未做實際播放驗證；不代表 Code／Security 階段結論。

結論：不通過