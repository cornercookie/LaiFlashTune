本輪僅做 Spec 唯讀審查；未修改檔案、未執行測試或建置。Planning 與 Design v1 的 SHA256 在審查前後一致。以下 `Design.md`、`planning.md` 均指指定任務資料夾。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| 1 | 阻擋 | `Design.md:20,29–30,152` | **R1 遺漏 FFmpeg 長檔入口**，將「長檔已配置重取樣器」概括為全部現況，並判定 `player.rs` 無需修改。 | `planning.md:9,15,20` 未將長檔限定於 Symphonia；`src-tauri/src/player.rs:539–549` 的 FFmpeg 長檔分支只建立 `TrackResampler::new()`，未呼叫 `set_track()`；`src-tauri/src/playback_pipeline.rs:936–938` 在沒有目前音軌時直接回傳 `None`。 | 將 FFmpeg 長檔納入 R1、架構圖及修改清單，明列原生取樣率／聲道的配置。 |
| 2 | 阻擋 | `Design.md:63–69,88,141` | **變速輸出契約與完成條件矛盾**：`apply_dsp` 固定一幀輸入、一幀輸出，卻要求速度／節奏改變輸出樣本數；沒有交代可變長度輸出的保存與取用。 | `planning.md:21` 要求節奏效果；`src-tauri/src/dsp.rs:550–574` 的 `RateStage::run()` 輸出至可變長度 `Vec`；`:668–674` 的既有呼叫者會繼續拉取，直到有輸出或來源結束。 | 明確定義 RateStage 的輸入、可變長度輸出及緩衝排出契約，讓 `next_playing()` 與 AC2 使用同一套契約。 |
| 3 | 重要 | `Design.md:110,141` | **AC2 判定不足且音高預期錯誤**。「輸出不同」無法分別證明各效果生效；第 141 行又要求音高調整改變樣本數。 | `src-tauri/src/dsp.rs:19–24` 定義節奏不改音高、音高不改節奏；`:758–784` 已分別驗證時長與頻率，其中音高測試要求長度比例接近 1。僅開殘響造成輸出不同，也可能滿足 Design 第 110 行。 | 分開列出速度、節奏、音高、回音、殘響的輸入及預期；音高應驗證頻率改變、時長保持。沿用既有測試依據，勿自行新增門檻。 |
| 4 | 阻擋 | `Design.md:60–68,93,150` | **即時參數更新被誤列為已有能力**，實作步驟未交代 PlaybackCore 如何將更新送入 FxChain／RateStage，無法保證控制項驅動音效。 | `Agents/business-logic.md:104` 要求改參數不重開音源；`src-tauri/src/player.rs:158–160` 只建立預設 FxChain；`src-tauri/src/playback_pipeline.rs:484–507` 未讀取版本或呼叫 `FxChain::set()`。更新機制實際位於另一條路徑的 `src-tauri/src/dsp.rs:643–649`。 | 將版本讀取、參數快照與兩個處理階段的更新列入設計，保持不重開音源；修正「已即時套用」的現況敘述。 |
| 5 | 重要 | `Design.md:75,99–103` | **Q3 的確認依據不足，且理由不等同現有尾端處理**。文件只有「已決定」，未提供允許捨棄 RateStage 緩衝的確認來源。是否另有使用者確認，本輪核對不到。 | Q1 有 `Agents/business-logic.md:103` 支持；Q3 卻以 TrackResampler 作類比。`src-tauri/src/playback_pipeline.rs:908–909` 是保留尾端；`src-tauri/src/dsp.rs:556–568` 也有排出 RateStage 緩衝的操作，並非直接捨棄。 | 補上 Q3 的原始確認來源；沒有來源則標待確認，明列捨棄對舊軌尾端輸出的影響，不以「與現有邏輯一致」作為確認依據。 |
| 6 | 重要 | `Design.md:111–112,143–144` | **完成條件未完整承接專案規則**。Cargo 測試、Tauri 建置與 exe 資源核對已列，但缺 Node 測試及程序清理要求。 | `AGENTS.md:50–52` 明定 `node --test "src/player/*.test.mjs"` 全過、建置前關閉 release exe，以及不遺留開發／測試程序。此為文件規則缺漏，程式行號不適用。 | 補列既定完成條件，或明確引用並承接 `AGENTS.md:49–52`。 |
| 7 | 次要 | `Design.md:127–128` | **實作順序圖仍有未避開的 Mermaid 語法風險**：未加引號的標籤包含 `new()`、`next_playing()` 與巢狀 `[L,R]`。渲染失敗屬**推測**，本輪未執行解析器。 | 第 127 行直接使用 `B[…立體聲幀 [L,R]，…]`；括號位於節點定界符內且未引用。純圖表語法問題，程式行號不適用。 | 所有節點標籤加雙引號，將 `[L,R]` 改為「左右聲道幀」，再驗證兩張圖可解析。 |

電台略過速度／節奏／音高的設計，與 `Agents/business-logic.md:103` 及 `src-tauri/src/player.rs:773–775` 一致；「改參數不重開音源」文字也一致，但仍須補齊第 4 項的更新路徑。

結論：不通過