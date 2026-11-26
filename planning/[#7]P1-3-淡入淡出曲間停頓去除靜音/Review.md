# [#7]P1-3-淡入淡出曲間停頓去除靜音 Review.md

## 審查對象
- 設計：`Design.md` (v25，已通過設計 Spec/Code/Security 三階段)
- 實作：`src-tauri/src/`、`src/` 的 P1-3 功能程式碼
- 執行結果：`cargo test --release` 59/59 passed；`node --test "src/player/*.test.mjs"` 5/5 passed；`npx tauri build` 成功；`git grep -i -e 'a[i]mp'` 空

## 實作審查 — Spec 階段（v1，結論：不通過）

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 狀態 |
|---|--------|------|------|------|------|------|
| 1 | 阻擋 | player.rs:1013、playback_pipeline.rs:1683 | R1/R2/R3/R6 的淡段流程未接通。Play/Pause/Stop 仍直接操作 sink；Seek 直接重載；Load 直接載入。未建立 `FadeSegment`，命令迴圈未消費 `done_target`；`OutputProxy::next()` 未套用 fade gain。 | Design 第 17–22、146–154 行 | 接通命令建立淡段、兩條輸出路徑推進增益、命令執行緒消費完成通知 | 未修 |
| 2 | 阻擋 | Shell.tsx:609、player.rs:1218 | R3 手動切歌契約未落實。`playSong` 仍發保險 `playerPlay()`；`Next\|Prev => {}` 仍為空操作；`pending_load` 僅宣告未使用。 | Design 第 19、249 行 | 移除保險 Play，實作 `PendingLoad/SwitchLoad` 及 Next/Prev 待播槽處理與無下一首錯誤 | 未修 |
| 3 | 阻擋 | Shell.tsx:1824 | 八個新增控制項無法驅動引擎。傳入的都是 React setter，未接到實際呼叫 command 的處理函式。 | Design 第 249 行及 AGENTS 規則 | 將控制項接到實際呼叫 command 的處理函式，保留錯誤回報 | 未修 |
| 4 | 阻擋 | playback_pipeline.rs:461 | R4 曲間停頓未啟動。`gap_remaining` 僅初始化為 0，沒有依 `gap_ms × 48000 × 2 / 1000` 賦值；`WaitingForNext` 直接回傳靜音，無 gap 耗盡後交接。 | Design 第 176、279 行 | 在曲終一次性啟動 gap，於共同交接分支耗盡後安裝下一軌 | 未修 |
| 5 | 阻擋 | playback_pipeline.rs:1180、player.rs:1224 | 串流開流、ready 通知與安裝契約未實作。`TrackData::Stream` 被取出後轉換為 `None`；`poll_completion` 成功分支只記 log。`stream_install_ready` 僅宣告未發布/消費；`open_with_seek()` 仍回傳錯誤。 | Design 第 190–203 行 | 完成背景開流、世代核對、ready 發布及命令執行緒安裝/更新身分流程 | 未修 |
| 6 | 阻擋 | player.rs:534、playback_pipeline.rs:2139 | R5 長檔開頭靜音掃描缺漏。直接載入長檔沿用傳入 `position_ms`，無偵測 leading 偏移；預載長檔 `position_ms` 固定 0。 | Design 第 239–243、280 行 | 補上有界頭部掃描及偏移傳遞，包含取消、期限與回收契約 | 未修 |
| 7 | 阻擋 | player.rs:89、345、575 | R5 修剪後時間不再對應原檔。`trim_silence()` 回傳 frame 數卻存入 `leading_silence_samples`，再除以 `sample_rate × channels`；立體聲 3 秒導靜音算成 1.5 秒。直接載入與預載用裁切後樣本重算 `duration_ms`，違反保留原始總長度契約。 | Design 第 181、280 行 | 統一偏移單位，依設計保存交錯樣本數；修剪前保存原始總長度 | 未修 |
| 8 | 阻擋 | player.rs:357、Shell.tsx:777 | 新設定未回寫 `EngineSnapshot`。`publish()` 未指定六項 fade、gap、silence 欄位，快照保留 Default。 | Design 第 223、249 行 | 從 `EqShared` 發布八項實際設定，再由快照初始化/同步 UI | 未修 |
| 9 | 阻擋 | player.rs:1066、Shell.tsx:465 | gap/crossfade 執行中互斥與重啟權威來源不一致。SetGapMs 只清 `EqShared.crossfade_ms`，未清 `engine.crossfade_ms/core.crossfade_ms`；SetCrossfade 未清 `EqShared.gap_ms`。 | Design 第 254–256 行 | 後端同步互斥狀態；session 僅讀取權威值，不再反向套用舊 crossfade | 未修 |
| 10 | 重要 | Design 第 272、290–294 行 | 缺少本功能的指定測試與實際驗收證據。未新增 FadeGate、gap 長度/進度、trim、串流頭部掃描、參數互斥的測試斷言；無聽測、波形及簽章紀錄。 | Design 第 272、290–294 行 | 補齊設計要求的測試與驗收紀錄，將未執行項目明列 | 未修 |

完成條件實際有 10 項：第 1–5 項尚未符合；第 6 項的三處註冊與參數拒絕邏輯已靜態核對；第 7–9 項僅有測試/建置結果，本輪未重跑；第 10 項已重查，`git grep` 及指定審查檔案均無禁止名稱命中。

## 後續
逐一修正 #1–#9（阻擋）與 #10（重要），每修一項需 `cargo test --release` 與 `node --test "src/player/*.test.mjs"` 全過，然後重新送審 Spec 階段；通過後進 Code 與 Security 階段審查。
