本輪唯讀 Spec 複查，未修改 repo、未執行測試或建置。Design 雜湊前後一致；指定 `git grep` 與未追蹤 Design 的補充搜尋均無命中。

R1–R6 與完成條件已涵蓋 Planning；九個設定 key、Default、互斥持久化及三處指令註冊均已列入。#7b 的偏移單位修正成立；gap 公式在固定 48 kHz 雙聲道下正確。以下為 v8 交接修改仍未閉合的契約。

**D**＝[Design.md](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md>)；下列 Rust 檔案位於 `src-tauri/src/`。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| 1 | 阻擋 | D:161、220；對照 D:126、256 | **觸發時機已修正，但開流與 Sink 操作被移入音訊取樣臨界區。** 與文件「回呼不可阻塞、不得持鎖操作 sink」矛盾。 | `playback_pipeline.rs:1553–1570` 顯示 `OutputProxy::next()` 持有 `core_lock` 呼叫 `core.next()`；`audio_decode.rs:325–369` 的 `SymStream::open()` 執行封包預檢、格式探測、seek 與首次解碼。`sink` 則由 `AudioEngine` 持有（`player.rs:44–48`），不在 PlaybackCore。依新增流程推論，開流耗時會阻塞音訊取樣。 | 保留曲終／gap 觸發條件；把開流交給背景準備或命令執行緒，明定準備完成後的安裝交握；Sink 操作留在命令執行緒。 | #7a 原問題已修，但引入執行緒契約衝突。 |
| 2 | 阻擋 | D:161、220、222 | **「更新 song_id」沒有接到實際快照讀取來源；清除 TrackChain 後，歌曲身分與總長度會失去來源。** | `player.rs:324–343` 的 `get_current_duration()`／`get_current_song_id()` 只讀 TrackChain，沒有時回傳 `0`／`None`；`publish()` 在 `:283–291` 每次重新讀取。`Shell.tsx:775–807` 又要求有效 `song_id` 才更新目前歌曲、消耗佇列及排下一首。因此僅更新 seq/token/path/decoder 或一次寫入快照仍不足。 | 明定串流目前軌中繼資料的保存位置，包含 `song_id`、原始 `duration_ms`；將快照與 Seek 的讀取端接到該來源，列入實作步驟。 | #7c 的身分更新契約仍未完整。 |
| 3 | 重要 | D:161、220；對照 D:147 | **`pending_stream` 與既有 EOF 狀態機尚未接合，串流結束後的下一次交接也沒有入口。** | 現有 `TrackChain::next()` 在未轉成記憶體軌時回傳 `None`（`playback_pipeline.rs:1058–1066`）；上游結束後進入 `WaitingForNext`（`:478–483`），此狀態的 `core.next()` 只回靜音（`:351–361`）。v8 未指定如何避免／解除此狀態。安裝後又清除 TrackChain，而唯一 `take_next()` 入口仍放在 TrackChain，後續 Stream 曲終無法走該入口。 | 明定「待交接」與真正 EOF 的分支、安裝後播放狀態及暫停保留規則；讓記憶體與串流曲終共同觸發同一交接流程。 | 依現有程式推論可能停在靜音或中斷連續交接，須補齊。 |

以上為文件與原始碼的靜態核對及流程推論，不代表實際播放驗證或 Code／Security 階段通過。

結論：不通過