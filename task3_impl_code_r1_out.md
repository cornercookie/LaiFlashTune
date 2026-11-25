已完成唯讀 Code 審查，未修改 repo。四個欄位、`apply_dsp_block()` 的處理順序、`RateStage::new/run` 的 `pub(crate)` 與 `&FxParams`、FFmpeg 長檔的 `set_track`／`RateStage` 建立均已加入，但實際呼叫鏈仍有以下缺口。

下表 `Design.md` 指本任務 v10；程式路徑皆位於 `src-tauri/src/`。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| 1 | 阻擋 | [playback_pipeline.rs:584](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:584) | **音效參數未接通，控制項無法驅動新增 DSP 鏈。** | Design.md:71、75、78 要求更新快照並呼叫 `FxChain::set()`。全域搜尋確認 `dsp_params` 僅初始化為 `None`（169）及讀取（584–585）；`player.rs:913–914` 只更新 `EqShared`。`FxChain` 在 `player.rs:160` 建立後未呼叫 `set()`，因此速度／節奏／音高及其他音效仍使用預設關閉值。 | 按 `EqShared` 版本更新快照，並以同一份參數呼叫 `FxChain::set(&params)`。 |
| 2 | 阻擋 | [playback_pipeline.rs:454](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:454) | **短檔混用單樣本與立體聲幀處理，造成聲道配對及跨區塊順序錯誤。** | Design.md:23、56 要求共同進入立體聲 DSP。程式 455 先將重取樣區塊首樣本以舊 `apply_dsp(s)` 回傳；下一次呼叫卻在 352–375 將剩餘輸出兩兩配對。首個 L 已被取走，配對從 `[R0,L1]` 開始，末尾 R 留到後續區塊；455 也繞過 RateStage。 | 長短檔的重取樣結果全部進入 `track_out`，統一組成 `[L,R]` 後處理；移除單樣本 DSP 回傳路徑。 |
| 3 | 阻擋 | [playback_pipeline.rs:423](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:423) | **EOF 未實作一次性排空，且尾端首樣本繞過 RateStage。** | Design.md:100 明定上游及 RateStage 各終結一次。程式 381–383 在串流 EOF 落入舊分支，426 每次鏈結束都呼叫 `flush_tail()`，432 直接走單樣本 DSP；`flush_tail()`（1002–1046）沒有已終結檢查。新增 RateStage 呼叫（602）永遠為 `flush=false`。 | 加入 EOF 階段狀態：上游 flush 一次、尾端全部經完整 DSP、RateStage 空輸入終結一次，排完輸出才進入等待。 |
| 4 | 重要 | [player.rs:416](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs:416) | **Symphonia 長檔首次載入沒有 RateStage。** | Design.md:74、76 要求本地長檔建立 RateStage。416–439 只建立 TrackResampler 與空 TrackChain；479–481 的共同安裝點也未建立。`sync_identity()` 在 `playback_pipeline.rs:480` 遇空 TrackChain 直接返回，因此此路徑會在 603–605 略過變速處理。 | 在共同來源安裝入口依來源類型建立 RateStage，涵蓋 Symphonia、FFmpeg 與重新載入。 |
| 5 | 重要 | [player.rs:479](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs:479) | **手動換軌／seek 未清空新增佇列，會先播放舊位置殘留樣本。** | Design.md:93、166 要求安裝新來源前清空。`load_at()` 的安裝點 479–481 只替換來源與重取樣器；seek 經 659 呼叫同一路徑。佇列清空只存在於 `sync_identity()`（514–515），但 `next_playing()` 在 346–348 就先回傳舊 `dsp_out`。 | 在手動安裝來源的共同入口清空兩個佇列，重建 RateStage 並重設終結狀態。 |
| 6 | 重要 | [playback_pipeline.rs:497](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:497) | **自然交接直接丟棄舊 RateStage，未完成設計要求的尾端處理。** | Design.md:90–92 要求舊尾端先經舊 RateStage，終結並排完後才能重建。實作 500–509 只 flush／切換 TrackResampler，513 立即替換 RateStage，514–515 清空佇列；沒有舊 RateStage 的終結呼叫。啟用變速並有內部緩衝時，殘餘資料會被丟棄。 | 將自然交接與手動重設分開；先完成舊軌 DSP 排空，再建立新 RateStage。 |
| 7 | 重要 | [playback_pipeline.rs:2026](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:2026) | **既有測試不足以驗證本次新路徑。** | Design.md:156–166 列出串流、參數、佇列、EOF 與 seek 測試；本次差異沒有新增測試。現有等待測試 2038–2047 只確認出現零樣本，未斷言進入 `WaitingForNext`；`dsp.rs:814–825` 的參數測試使用 `DspSource`，並非新增的 PlaybackCore 路徑。 | 補上設計已列出的整合路徑測試，尤其是左右聲道不同、實際參數更新、有限次取樣後進入等待，以及 seek 清空。 |

本輪為靜態程式與呼叫鏈核對；未執行建置、測試或音訊實測，未採信文件中的「52/52」作為本輪驗證結果。結束前確認三個程式檔與 Design.md 雜湊未變。

結論：不通過