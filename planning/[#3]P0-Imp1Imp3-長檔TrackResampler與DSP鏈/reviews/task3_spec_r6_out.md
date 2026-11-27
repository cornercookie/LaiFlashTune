本輪僅複查 Q3 與 SC#1，未修改任何檔案；三份任務文件前後雜湊一致。

- **Q3 排空機制已修正**：`Design.md:75、101` 的「全部 `flush=false` 餵入，再空輸入單次 `flush=true`」可排出 RateStage 殘餘。依據為 `src-tauri/src/dsp.rs:550–574`、WSOLA 排空 `529–536`、Resampler 排空 `435–441`。在保留所有產出的前提下，可支持「不因捨棄舊 RateStage 而截斷其緩衝」。
- **排空後半段已對齊，但輸入端仍有缺口**：音效→正規化→限幅符合 `Design.md:63–68`；剩餘問題如下。
- **SC#1 保持關閉**：`Design.md:110` 與 `140–142` 均分別列出節奏、音高、速度、殘響、回音及關閉音效的驗證，兩表一致。相關程式依據為 `src-tauri/src/dsp.rs:114–118、363–385、758–784`；此為測試定義核對，非測試已通過。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| 1 | 阻擋 | `Design.md:75、101`；對照 `63–68` | Q3 未交代上游尾端在餵入 RateStage 前先經 EQ，卻規定產出不得再進 EQ。尾端完整處理順序仍不明確；照列出的步驟實作會漏做 EQ。 | `src-tauri/src/playback_pipeline.rs:892–909` 僅將重取樣結果移入 `pending_output`，尚未做 EQ；現行 `345–346、484–490` 在取出後才做 EQ。`src-tauri/src/dsp.rs:550–574` 的 RateStage 本身不含 EQ。因此「已過 RateStage」不能證明「已過 EQ」。 | 在兩處 Q3 明定：**上游舊軌尾端先經 EQ，再以 `flush=false` 餵入舊 RateStage**；餵完後空輸入單次排空。所有 `run` 產出維持僅走音效→正規化→限幅，不重複 EQ 或 RateStage。 |

本輪為靜態 Spec 複查，未執行測試或建置。環境 PATH 找不到 `codex` CLI，故由目前 Codex 直接審查，未另執行專案指定的 `codex exec`。

結論：不通過