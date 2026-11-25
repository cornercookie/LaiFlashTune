本輪僅做 Security 靜態唯讀審查，未修改 repo、未執行測試。已確認 Design 為 v10，審查前後雜湊一致。**未發現阻擋問題，另有 1 項次要設計缺漏。**

下表 `Design.md` 指本任務的 `planning/[#3]P0-Imp1Imp3-長檔TrackResampler與DSP鏈/Design.md`。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| S1 | 次要 | Design.md:67–81 | 新增 Mutex 未明定鎖的釋放時點與巢狀持鎖限制。**推測**：若以 guard 借用參數並持有至 DSP 完成，命令更新會等待整段處理；目前無法證實鎖序倒置或死鎖。 | 設計僅寫「取得快照」。現有 [playback_pipeline.rs:486–501](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:486) 明確先釋放 EQ 鎖，再取得 FX 鎖；[eq.rs:50–52](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/eq.rs:50) 複製參數後即釋放鎖。新增四把鎖尚未實作，無法核對實際持鎖時間。 | 明記 `dsp_params` 複製後立即解鎖；佇列取出／追加使用短鎖區段，DSP 各階段避免交疊持鎖。命令更新沿用相同規則。 |

其餘關注項目的核對結果：

- **`pending_output`：未確認跨軌殘留問題。** Design.md:90、93 區分自然交接保留尾端與手動換軌／seek 清空輸出。[`set_track()` 確實不清除 `pending_output`](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:821)，但現行手動載入建立新實例，並於 [player.rs:479–481](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs:479) 替換整個重取樣器；seek 於 [player.rs:639–645](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs:639) 先移除舊實例再載入。因此舊容器不會由新實例繼續輸出。此判定以保留現有替換流程為前提。
- **`None`：v10 已正確區分。** Design.md:48–59 明定 `process(None)` 表示累積中、繼續拉取上游，`pop_output(None)` 表示緩衝耗盡；符合 [playback_pipeline.rs:914–953](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/playback_pipeline.rs:914)。現行第 335 行確實會插入靜音，但已由本次設計明確修正，不列為未解設計問題。
- **FFmpeg 路徑：未發現本次變更新增路徑遍歷。** Design.md:176 調整來源安裝流程；現行 [player.rs:370](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/player.rs:370) 先驗證本機路徑，再進入 FFmpeg 分支；[audio_decode.rs:575–587](/C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/src-tauri/src/audio_decode.rs:575) 以獨立參數傳入 `-i`，限制協定為 `file,pipe`。播放器允許使用者選取本機音樂檔，沒有僅限 `data/` 內讀取音樂的授權邊界，不能單憑路徑含 `..` 判為越權。

結論：通過