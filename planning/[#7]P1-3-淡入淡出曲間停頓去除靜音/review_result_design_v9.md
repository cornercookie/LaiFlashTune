本輪為唯讀 Spec 複查，未修改 repo、未執行測試或建置。Design 與 Planning 雜湊前後一致；指定 `git grep` 及未追蹤 Design 的補充搜尋均無命中。

v8 #1 的開流／Sink 執行緒分工、#2 的串流快照來源已補入。R1–R6、九個設定 key、Default、互斥持久化、三處指令註冊與驗收方法均有列入；固定 48 kHz 雙聲道下的 gap 公式正確。以下為本次修改仍未閉合的契約。

位置簡稱：**D**＝[Design.md](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md>)；**P**＝[playback_pipeline.rs](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs)；**E**＝[player.rs](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs)；**UI**＝[Shell.tsx](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src/player/Shell.tsx)。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| 1 | 阻擋 | D:161、220、222 | **串流安裝後仍未定義離開 `WaitingForNext` 的狀態轉移。** 「與記憶體軌共用曲終交接」也未交代串流結束後如何接記憶體軌。 | P:351–361 在 `WaitingForNext` 只回靜音；P:498–500 的 `set_stream_source()` 只替換來源。現有 Play 必須另外設定 `PlaybackState::Playing`（E:924–931）。D 新流程只列安裝來源與 `sink.play()`；記憶體下一軌仍交給 `TrackChain::next()`，但等待狀態不會呼叫它。依流程推論，可能持續靜音或無法接續短檔。 | 明列等待狀態下的共同交接入口、Memory／Stream 分支，以及安裝後恢復 `Playing` 的責任與暫停保留規則；串流曲終同樣接入 gap 計算。 | v8 #3 尚未關閉。 |
| 2 | 阻擋 | D:92、124、161、222 | **`done_target = Switch` 同時代表兩種不同動作，消費契約衝突。** | D:124 規定 Switch 完成後從 `pending_load` 載入新軌並啟動淡入；D:161、222 卻在串流已安裝後使用同一通知，要求直接 `sink.play()`。自然交接沒有建立 `PendingLoad`，也未指定該通知使用哪個完成世代。依不同段落實作，可能漏載手動切歌，或自然交接誤走載入流程。 | 區分「手動淡出完成、待載入」與「自然交接已安裝、待恢復」通知，或明列互斥判斷；各自指定世代、所需資料與淡入時機。 | v9 新增的核心動作歧義，待修。 |
| 3 | 阻擋 | D:161、192、220；對照 D:274 | **重寫後的安裝步驟未承接 v8 已列的重取樣器與完整播放身分更新。** 只更新 `song_id`／總長度不足以完成交接。 | 新流程未列清除舊 TrackChain、依新 `rate/channels` 重設重取樣器、更新 `seq/token/path/decoder`；這些只剩 D:274 版本紀錄。P:498–500 不會代做；P:507–512 的身分同步依賴 TrackChain。UI:775–807 要求 `play_seq` 改變才更新歌曲及排下一首；E:704–730 的 Seek 仍讀 TrackChain 或 `current_path`。依流程推論，可能沿用舊取樣格式、畫面不換歌，或 Seek 載回舊檔。 | 將 v8 已確認的安裝契約恢復至 §5 與實作步驟，明列各欄位寫入與讀取端；保留 v9 新增的串流快照來源。 | 快照來源修正成立，但安裝流程出現退漏，待修。 |

以上為文件與原始碼的靜態核對及流程推論，不代表實際播放驗證或 Code／Security 階段通過。

結論：不通過