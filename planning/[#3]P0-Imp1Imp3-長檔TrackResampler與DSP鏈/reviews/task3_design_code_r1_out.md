v7 尚有阻擋問題，主要涉及方法可見性、可變長度輸出、初始化及 EOF 排空。已唯讀核對本機鎖定版本的原始碼：rubato 0.16.2、rodio 0.19.0、symphonia／symphonia-core 0.5.5。

以下 `Design.md` 指本次 [v7 設計文件](</C:/tempDevelopmenyEnviroment/LaiFlashTune/SonicLedger/planning/[#3]P0-Imp1Imp3-長檔TrackResampler與DSP鏈/Design.md>)；程式路徑皆相對專案根目錄。

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 |
|---|---|---|---|---|---|
| 1 | 阻擋 | Design.md:60–62、151 | **只加入 `use RateStage` 仍無法編譯。** 型別可見不代表其方法可見。 | `src-tauri/src/dsp.rs:540` 為 `pub(crate) struct`，但 **547、550 行的 `new()`、`run()` 都是私有方法**。另外 `new()` 的聲道參數是 `usize`，`INTERNAL_CHANNELS` 是 `u16`。 | 將需要跨模組呼叫的方法改成 `pub(crate)`，傳入 `INTERNAL_CHANNELS as usize`；把 `dsp.rs` 列入變更清單。 |
| 2 | 阻擋 | Design.md:63–69、88、110 | **`apply_dsp(&mut [f32; 2]) -> [f32; 2]` 的一進一出契約不足以承接 RateStage。** 無輸出時無幀可回傳；多幀輸出時不能只保留一幀。 | `src-tauri/src/dsp.rs:499–527` 的 WSOLA 會先累積，再成批產出；`550–576` 將可變數量樣本追加至 `Vec`。既有 `DspSource` 使用輸出緩衝與游標（600–601、652–675、701–706），不是固定回傳一幀。 | 改為「餵入完整幀／區塊，追加零到多幀輸出」；PlaybackCore 保留 DSP 輸出佇列及逐樣本讀取位置，先消耗輸出，再拉上游。 |
| 3 | 阻擋 | Design.md:49–53、69、100 | **`process()` 與 `pop_output()` 的消耗契約未對齊，且把區塊尚未湊滿當成靜音會插入額外音訊時間。** | `src-tauri/src/playback_pipeline.rs:944–953、985` 顯示 `process(input)` 已餵入樣本並自行呼叫 `pop_output()`。若忽略其回傳值，再取兩次 `pop_output()`，會漏掉已彈出的聲道樣本。`None` 也可能只表示未滿 2048 原生幀（31、951–953），不是音訊中的靜音。 | 明定唯一消耗方式：保留 `process()` 已回傳的樣本，再補齊幀；或拆開餵入與取出 API。正常本地檔案累積期間繼續拉取輸入，不以靜音代替；來源結束另行處理。 |
| 4 | 阻擋 | Design.md:61–62、129、150 | **指定的 RateStage 建立時機未涵蓋長檔初次載入。** 初始化為 `None` 後，僅靠 `sync_identity()` 無法建立長檔的 RateStage。 | `src-tauri/src/player.rs:420–435` 長檔使用空 `TrackChain`；`src-tauri/src/playback_pipeline.rs:413–417` 的 `sync_identity()` 遇到沒有目前軌就返回。現有 `set_track()` 是 `TrackResampler` 方法（821），不持有 PlaybackCore 的 `rate_stage`。 | 明確指定 PlaybackCore 的載入／重設入口，涵蓋初次載入、手動換軌、seek 與自然交接；可使用既有來源安裝入口，或新增入口並調整呼叫者。 |
| 5 | 阻擋 | Design.md:66、93、110 | **「參數已透過版本號即時套用」不適用於現有 PlaybackCore 的 FxChain。** 只改為立體聲呼叫仍不會啟用回音、殘響等設定。 | `src-tauri/src/player.rs:159–160` 只建立預設 FxChain；899–900 僅更新 `EqShared`。`src-tauri/src/playback_pipeline.rs:494–500` 直接呼叫 `frame()`，沒有刷新參數。實際讀版本並呼叫 `fx.set()` 的路徑在 `src-tauri/src/dsp.rs:643–649`，屬於 DspSource。 | 在 PlaybackCore 加入參數刷新機制，取得同一份 `FxParams` 快照供 RateStage 與 `FxChain::set()` 使用；修正第 93 行的現況敘述。 |
| 6 | 阻擋 | Design.md:20、50、152 | **`player.rs` 不需修正的判斷漏掉 FFmpeg 長檔與 Symphonia 失敗後的後備路徑。** 這些來源仍不能重取樣。 | `src-tauri/src/player.rs:468、471–472` 會進入 FFmpeg 準備流程；539–549 的長檔分支只呼叫 `TrackResampler::new()`，沒有 `set_track()`。`src-tauri/src/playback_pipeline.rs:937–938` 因 `current_track` 為空而持續回傳 `None`；外層 resampler 卻是 `Some`，不會命中設計的缺失檢查。 | FFmpeg 長檔也須設定原生取樣率、聲道與軌道資料；可在該分支補齊，或統一來源安裝流程。 |
| 7 | 阻擋 | Design.md:75、82、101 | **不能直接沿用現行 EOF 流程來保證「排空後進入 WaitingForNext」。** 它會再次 flush 上游，而且文件的 EOF 分支未明定 RateStage 的一次性完成狀態。 | `src-tauri/src/playback_pipeline.rs:358–372` 每次遇到鏈結束都呼叫 `flush_tail()`；866–910 沒有已 flush 即返回的防護。rubato `src/lib.rs:134–166` 顯示 `process_partial(None, …)` 是再餵一個補零區塊，並非查詢是否已排空，因此可反覆產生輸出，無法到達等待分支。 | 明定 EOF 狀態：上游只 flush 一次 → 全部尾段經 EQ／RateStage → RateStage 只終結一次 → 排完 DSP 輸出 → 才進等待。不可用重複呼叫 `flush_tail()` 判斷結束。 |
| 8 | 重要 | Design.md:76 | **「DSP 檢查點映射已存在」目前只有註解依據，沒有對應實作。** 加入變速後不能沿用輸入幀數作為輸出邊界。 | `src-tauri/src/playback_pipeline.rs:53–55` 是註解；實際 453–455 行直接使用 `current_boundary_out_start()`。其初值為 0（152），`set_current_boundary_out_start()`（272）未找到呼叫者。 | 將映射列為待實作，或更正現況聲明；以舊軌最後一個 DSP 輸出幀的位置提交邊界，與新輸出佇列一起處理。 |
| 9 | 重要 | Design.md:75、118 | **「1024-frame 區塊拉取、天然背壓」不足以證明不阻塞回呼，且區塊數值與目前管線不符。** 實際是否超過裝置期限屬**推測，尚未量測**。 | `src-tauri/src/playback_pipeline.rs:31、1201、1214–1218` 在核心鎖內拉取 2048 幀；`pending_output.remove(0)`（917）逐樣本搬移資料。`src-tauri/src/dsp.rs:480–495` 有 WSOLA 相關搜尋與配置。rodio `src/stream.rs:214–218` 確認拉取發生於 cpal 回呼；rubato `src/lib.rs:134–141` 明示 partial API 會配置記憶體。 | 改正執行位置與區塊說明；尾段使用游標或佇列批次消耗，重用緩衝。若保留同步處理，量測換軌最壞耗時；若無法符合回呼期限，再改為背景處理與有界輸出緩衝。 |

針對指定可行性問題，逐項答覆如下：

1. **跨模組可見性：不足。** `RateStage` 型別可以匯入，`new()`／`run()` 不可直接呼叫，見第 1 項。

2. **跨執行緒與鎖序：未確認存在新鎖的倒置，但設計尚不足以排除。** commands 透過通道送命令（`commands.rs:135–138、477–478`），AudioEngine 的 `recv_timeout` 負責處理命令（`player.rs:804`）；實際取樣另在 cpal 回呼。現有 `apply_dsp()` 先釋放 EQ 鎖再取得 FX 鎖（`playback_pipeline.rs:486–501`），而 `init_dsp()` 同時持有 EQ → FX（`player.rs:150–160`）。建議新增 RateStage 後仍分段釋放內部鎖，先複製參數再處理；避免出現 FX → RateStage → EQ 的反向巢狀取得。RateStage 本身由數值與 `Vec` 組成，未看到阻止放入 `Mutex` 跨執行緒使用的型別。

3. **尾段順序可以實作，但不能據此宣稱完整變速且無縫。** v7 的「EQ → 舊 RateStage，產出只走後段音效／正規化／限幅」順序正確，沒有要求重複 EQ。實際 `flush=true` 會先正常運算，再：
   - `wsola.drain_raw()` 直接追加 tail 與剩餘 input，清除狀態（`dsp.rs:529–536`）。
   - `rs.drain_raw()` 從 `floor(t)` 起直接複製剩餘樣本，保留一幀歷史並重設位置（`dsp.rs:435–441`）。

   因此單次呼叫可排出待輸出的內部殘餘，但 raw 部分不再依原比例完成 WSOLA／插值處理；**無法由此保證尾段長度精確或交界無可聽不連續**。若要保留既有 EOF 策略，應如實描述其限制，並先修正第 2、7 項。這些方法是專案自有實作，並非 rubato API。

4. **實時約束：可能拖長音訊回呼，尚無證據證明必然卡頓或必然安全。** 見第 9 項；80ms 命令輪詢不是 DSP 的執行時間預算。

5. **單元測試可寫，不需音訊裝置。** 現有 `dsp.rs:723–784` 已有合成音、頻率與輸出長度測試；`playback_pipeline.rs:1789–1820` 可直接建立 PlaybackCore 與 TrackResampler。可注入有限的合成串流，驗證幀順序、參數更新與一次性 EOF 排空；PlaybackCore 等待時持續回傳靜音，測試須限制拉取次數，不能直接無限 `collect()`。Node 測試可做前端回歸，但不能證明 Rust DSP 正確。

本輪未修改任何檔案，未執行 Cargo／Node 測試或建置，也未進行裝置播放與耗時量測；審查前後 Design.md 的 SHA-256 相同。

結論：不通過