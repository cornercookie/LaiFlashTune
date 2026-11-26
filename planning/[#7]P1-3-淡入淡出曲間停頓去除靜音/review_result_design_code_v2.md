本輪為唯讀 Design／Code 複查，未修改 repo。前輪 #6 路徑已修正；#1 來源失效、#4 RateStage 重建、#5 slot 消費已補入正文，但仍有以下問題。

D＝[Design.md](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md>)；程式位置均相對於 `src-tauri/src/`。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| 1 | 阻擋 | D:132、179–180、246、282 | **串流安裝仍指定在音訊回呼執行，前輪 #2 未關閉。** | D:132 說安裝僅在命令執行緒 tick；D:179–180、246 卻要求 `PlaybackCore::next()` 呼叫 `set_stream_source()`。現有 `OutputProxy::next()` 在 `playback_pipeline.rs:1553` 持核心鎖，於 `:1570` 呼叫 `core.next()`；`:499` 覆寫來源會析構舊值，`audio_decode.rs:746–750` 的析構呼叫取消，可能進入 `playback_pipeline.rs:1688–1717` 的等待回收。 | 統一正文與實作步驟：回呼只標記交接就緒；命令執行緒完成來源交換，舊來源移出鎖後交回收執行緒析構。 | 待修 |
| 2 | 阻擋 | D:102、134–142、186 | **新增 SetNext／ClearNext 遞增共用世代，會取消無關的播放控制動作。** | `pending_gen` 同時判定淡段完成通知與串流有效性。依設計推演：Pause 淡出世代為 g，期間收到 SetNext 後變成 g+1，Pause 完成通知被捨棄，`sink.pause()` 不執行；Seek／SwitchLoad 也會遭遇相同問題。現有 `player.rs:1082–1095` 使用獨立協調器世代管理下一首。 | 將淡段動作世代與下一首來源世代分開；SetNext／ClearNext 只撤銷下一首來源，不使當前 Pause／Seek／SwitchLoad 失效。 | 本輪修正引入，待修 |
| 3 | 重要 | D:178、224、246–248 | **FFmpeg 補實作定位錯誤，取消所有權尚未接清。** | D:178 指向 `audio_decode.rs:1665–1667`，實際未實作的是 `playback_pipeline.rs:1665–1667` 的 `FfmpegStreamHandle::open_with_seek()`；`audio_decode.rs:670` 是另一個已有實作的方法。實際預載於 `playback_pipeline.rs:2176–2201` 複製原 stream 的取消 handle，而原 stream 析構會呼叫取消；因此不能直接沿用該 handle 開新程序。 | 明確指定修改實際 handle 方法；新開流建立獨立取消控制，交代來源持有、過期結果丟棄及回收的所有權。同步修正檔案定位與實作步驟。 | 前輪 #3 部分修正 |
| 4 | 重要 | D:180、246 | **RateStage 重建只補正文，實作步驟仍漏列。** | D:180 已要求重建 RateStage；D:246 的完整串流安裝清單仍只有重取樣器。現有 EOF 在 `playback_pipeline.rs:442–443` 取走 RateStage；正常載入則在 `player.rs:542–543` 明確重建。照實作步驟執行仍可能遺漏速度／節奏／音高處理。 | 將 D:180 的 RateStage 重建要求同步至第 3 步串流安裝清單。 | 前輪 #4 尚未完整關閉 |

三個新指令的 camelCase 引數與三處註冊已列入設計，尚未實作驗證。本輪實測 Node 五個測試檔全過、`tsc --noEmit` 通過；指定 `git grep` 及未追蹤 Design 補查均無命中，Design 前後 SHA-256 一致。

Cargo 測試與 Tauri 建置會寫入產物，本輪未執行；不能據此保證實作後全過。未執行播放聽測、競態重現或記憶體峰值量測。

結論：不通過