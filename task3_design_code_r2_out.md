v8 尚有 **3 項阻擋、2 項重要問題**。F4、F6、F7、F8 的原問題在設計層級可關閉；F1、F2、F3、F5、F9 尚未完整閉合。

以下 `Design.md` 均指本任務 v8。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| R2-1（F3） | 阻擋 | `Design.md:53、107、135` | 重取樣介面仍不相容，消耗規則也未統一。正文改用切片輸入、向量輸出，其他段落卻仍要求取兩次 `pop_output()`。 | 文件寫 `process(&[sample]) -> Option<Vec<f32>>`；實際 `src-tauri/src/playback_pipeline.rs:936` 是 `process(f32) -> Option<f32>`，並在 `947、953、985` 自行消耗一個輸出樣本。變更清單 `Design.md:160` 未交代此介面的改造。 | 明確選擇保留現有介面或改成區塊介面。保留時須接住 `process()` 已回傳的樣本，先排出剩餘輸出再餵入上游，統一正文與 Q2。 |
| R2-2（F2） | 阻擋 | `Design.md:48–55、64、69–76、95` | **同一個 `dsp_out` 同時被定義為 DSP 輸入與最終輸出**，無法判定哪些樣本已處理，可能繞過或重複套用 DSP。此外，內部函式契約仍保留一幀進、一幀出的舊版本。 | `53–54` 要將 TrackResampler 輸出直接加入 `dsp_out`，再取出送 DSP；`64、76` 卻將它當成 DSP 最終輸出。`95` 仍寫回傳 `[f32; 2]`。實際 `src-tauri/src/dsp.rs:499–527、550–576` 會累積輸入並產出可變數量樣本；`playback_pipeline.rs:313–321` 對外仍逐樣本回傳。 | 固定資料流為「重取樣輸出暫存 → `apply_dsp_block` → `dsp_out` → 逐樣本回傳」；最終佇列不得再送入 DSP，並同步修正第 95 行。 |
| R2-3（F1／F5） | 阻擋 | `Design.md:65–73、100、160–161` | 可見性與 `usize` 已修正，但 **RateStage 契約漏掉必要參數，且引用不存在的 `FxParams.allow_rate`**；新增快照尚未形成一致的參數套用契約。 | 文件的 `run(input, flush, out)` 與實際 `src-tauri/src/dsp.rs:550` 的 `run(input, p: &FxParams, flush, out)` 不符；`FxParams` 在 `dsp.rs:18–48` 沒有 `allow_rate`，它實際是來源建構參數（`617、630`）。既有 `player.rs:899–900` 只更新 EqShared；`eq.rs:58–67` 負責清理參數與更新版本。FxChain 必須經 `dsp.rs:311–324` 的 `set()` 更新設定及配置效果狀態。 | 保留 `run` 的 `&FxParams` 參數；以來源類型決定是否建立／略過 RateStage。明定快照由清理後的 EqShared 取得，於區塊開始套用 `FxChain::set(&params)`，同一快照傳給 RateStage。 |
| R2-4（新增，與 F4 直接相關） | 重要 | `Design.md:64–68、76、126` | 新增 `dsp_out` 只寫建構初始化，沒有定義 seek／手動換軌時的清空規則。**推測：**若沿用佇列，跳轉後會先播放舊位置殘留聲音。 | `src-tauri/src/player.rs:479–485` 在同一個 core 安裝新來源；`618–645` 的 seek 也重用 core。文件只要求重建 RateStage，未處理新加入的最終輸出佇列。 | 明定手動換軌／seek 安裝來源前清空 `dsp_out` 與相關暫存、重設終結狀態；自然交接則依既定順序保留並排完舊軌輸出。 |
| R2-5（F9） | 重要 | `Design.md:53、125、202` | F9 只部分落實：風險表仍宣稱「1024-frame、天然背壓」及 RateStage「不主動累積」，與修正追蹤表及程式相矛盾。 | `src-tauri/src/playback_pipeline.rs:31、1201、1214–1218` 是核心鎖內拉取 2048 幀；`917` 仍以 `Vec::remove(0)` 消耗尾段。`src-tauri/src/dsp.rs:500–507` 明確累積 WSOLA 輸入直到門檻。 | 將第 202 行承諾落入正式做法與風險表：尾段採游標或佇列批次消耗、正確區分區塊大小，並保留換軌最壞耗時的實作量測要求；刪除尚無依據的背壓保證。 |

可關閉項目的核對依據：

- **F4：**`Design.md:68` 已涵蓋四種建立時機，補足長檔空 TrackChain 導致 `sync_identity()` 提早返回的問題（`player.rs:435`、`playback_pipeline.rs:415–417`）；新增佇列生命週期另見 R2-4。
- **F6：**`Design.md:20、162` 已納入 FFmpeg 長檔及 Symphonia 後備入口，對應 `player.rs:468、471–472、539–549`。
- **F7：**`Design.md:89、152` 已明定一次性排空與等待順序，對應現行重複 flush 呼叫點（`playback_pipeline.rs:358–372`）。
- **F8：**`Design.md:83` 已撤回映射既有實作的暗示，明列待實作及 DSP 輸出邊界依據，對應現行占位值（`playback_pipeline.rs:453–455`）。

本輪僅做文件與原始碼靜態複查，未執行測試、建置或音訊量測；未修改任何 repo 檔案。

結論：不通過