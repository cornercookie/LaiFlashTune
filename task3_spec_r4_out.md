SC#1 保持關閉：`Design.md:110` 與 `Design.md:141-142` 均分別驗證節奏、音高、速度、殘響、回音及全關閉直通，兩表一致。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| SC#2 | 重要 | `Design.md:75、101`（Q3） | 新增引用仍不足以證明重建 RateStage 不截斷尾端：混用了 TrackResampler 的保留機制與 DspSource 的排空流程。 | `src-tauri/src/playback_pipeline.rs:432-444、908-909` 僅將 **TrackResampler** 尾端移至其 `pending_output`。`src-tauri/src/dsp.rs:550-568` 中，變速處理啟用時須傳入 `flush=true` 才排空相應緩衝；`:659-669` 的 EOF 判定與呼叫屬於 **DspSource**。Design 未指定新增至 PlaybackCore 的 RateStage 在捨棄前如何執行此流程。**推測影響：**照目前重建步驟實作，尚存於舊 RateStage 的尾端可能被丟棄。 | 明定順序：將上游舊軌尾端送完至舊 RateStage → 呼叫 `run(..., flush=true, ...)` → 保留其輸出並經後續 DSP 排出 → 才捨棄及重建；同步修正 Q3 的保證敘述。 |

本輪僅做指定範圍的文件與程式靜態複查，未修改檔案、未執行測試。Q3 尚不能關閉。

結論：不通過