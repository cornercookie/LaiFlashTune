本輪為唯讀 Design／Code 審查，未修改 repo。v15 兩項 Spec 修正已確認；以下為對照現有程式推導的可實作性問題，非執行時重現結果。

D＝[Design.md](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md>)；程式檔以下以檔名表示。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| 1 | 阻擋 | D:179、181、187 | **更換下一首未使舊開流失效，可能播放已被取代的歌曲。** | 設計只將 Load／Seek／ClearNext 納入來源清除；`player.rs:1082–1086` 的 SetNext 只更新協調器世代。若串流 B 已開流，再 SetNext(C)，B 的 `meta.gen == install_gen` 仍成立，且安裝分支優先採用 B。 | 將背景開流與待安裝來源綁定 SetNext 的請求世代；替換下一首時同步撤銷舊來源，交付及安裝均核對。 | 待修 |
| 2 | 阻擋 | D:181、247、283 | **音訊回呼替換來源會同步觸發可能阻塞的 FFmpeg 回收。** | `OutputProxy::next()` 在 `playback_pipeline.rs:1553` 持有核心鎖；設計要求其中呼叫 `set_stream_source()`。該函式於 `:498–499` 直接覆寫並析構舊來源；`audio_decode.rs:746–750` 的 `FfmpegStream::drop()` 呼叫取消，而 `playback_pipeline.rs:1688–1717` 包含 kill、輪詢等待，最長約 40 秒。 | 回呼只移交舊來源所有權；在既有回收執行緒執行析構、取消與等待，避免在回呼或核心鎖內回收。 | 待修 |
| 3 | 阻擋 | D:179、224–225、247 | **串流開啟與靜音偏移只指定 SymStream，無法涵蓋現有 FFmpeg 格式。** | `scanner.rs:51` 將 WMA 指定為 FFmpeg；`playback_pipeline.rs:2171–2201` 產生 `StreamWrapper::FfmpegStream`，不是新增偏移欄位的 `FfmpegDeferred`。其 handle 的 `open_with_seek()` 在 `:1665–1667` 明確回傳未實作錯誤。 | 依實際串流變體／解碼後端分流，補齊 FFmpeg 的掃描、偏移開流與取消接線，不將所有 Stream 都送入 `SymStream::open()`。 | 待修 |
| 4 | 重要 | D:181、247 | **串流交接只重設 TrackResampler，下一軌會失去速度／節奏／音高處理。** | EOF 在 `playback_pipeline.rs:442–443` 將 `rate_stage.take()`；串流沒有 TrackChain 當前軌，`sync_identity()` 於 `:509–510` 返回，無法走到 `:586` 的重建。`apply_dsp_block()` 於 `:686–692` 在 RateStage 缺席時直接通過。 | 串流安裝步驟明列重建 RateStage，沿用現有載入路徑的處理，保留當前音效設定。 | 待修 |
| 5 | 重要 | D:179–185、249 | **同一串流同時留在協調器槽與待安裝來源，缺少一次性消費契約。** | `poll_completion()` 在 `playback_pipeline.rs:1895–1900` 將複本放入 slot 並回傳結果；設計另將開流結果放入 pending。Stream 安裝只 take pending，未消費 slot；在 tick 改為 Playing 前，下一次等待分支仍可從 slot 取回同一 Stream，重新標成待開流。 | 明定開流取得所有權時消費對應 slot，並區分「等待來源」與「已安裝、等待通知處理」，避免重複取出。 | 待修 |
| 6 | 次要 | D:11 | 程式基準中的建置腳本路徑錯誤。 | `src-tauri/src/build.rs` 不存在；實際 `COMMANDS` 位於 `src-tauri/build.rs:7`。正文三處註冊安排本身已有列出。 | 更正基準路徑為 `src-tauri/build.rs`。 | 待修 |

已執行：`node --test "src/player/*.test.mjs"`，5 個測試檔全過；`tsc --noEmit` 通過。三個新指令的 camelCase 引數及三處註冊已列入設計，但尚未實作驗證。

本輪未執行會寫入產物的 Cargo 測試與 Tauri 建置，不能判定實作後必然全過；亦未做播放聽測或記憶體峰值量測。指定 `git grep` 及未追蹤 Design 補查均無命中，Design 前後 SHA-256 一致。

結論：不通過