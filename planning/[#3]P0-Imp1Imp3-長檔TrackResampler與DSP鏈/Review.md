# [#3]P0-Imp1Imp3-長檔TrackResampler與DSP鏈 Review.md

> 審查紀錄：設計審查（實作前）→ 實作審查（實作後），各分 Spec／Code／Security 三階段。流程見根目錄 `AGENTS.md`。

## 設計審查（實作前）

### Spec 階段
| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| SC#1 | 重要 | Design.md 測試案例表「速度/節奏/音高參數生效」 | AC2 單元測試僅斷言「輸出樣本數與內容發生變化」，無法分別證明節奏、音高、速度、回音、殘響各自是否生效 | Design.md v1 測試案例表第 4 行（`planning/[#3]P0-Imp1Imp3-長檔TrackResampler與DSP鏈/Design.md`） | 將 AC2 測試拆分為分別驗證：`tempo != 1.0`（樣本數變、音高不變）、`pitch != 0.0`（樣本數不變、內容變）、`speed != 1.0`（兩者皆變）、`reverb_on`（殘疊尾音）、`echo_on`（延遲重複） | 已修正（v2：測試案例表拆分為分別驗證） |
| SC#2 | 次要 | Design.md 設計待確認事項 Q1–Q3 | Q3「重新建立 RateStage、捨棄舊實例」缺乏可追溯依據，可能截斷舊軌尾端 | Design.md v6 Q3 理由欄（`planning/[#3]P0-Imp1Imp3-長檔TrackResampler與DSP鏈/Design.md`） | 更正 Q3：上游舊軌尾端先經 EQ，再以 `flush=false` 全部餵入舊 `RateStage`，全部餵完後以空輸入呼叫一次 `flush=true` 排空殘餘（`dsp.rs:550-568`、`dsp.rs:659-669`）；`run` 產出僅經後續音效→正規化→限幅排出，不再重複進入 RateStage 或 EQ，與 `apply_dsp()`（EQ→RateStage→音效→正規化→限幅）於 RateStage 後方階段對齊 | 已修正（v7：Q3 補上先經 EQ＋flush=false 全部餵入＋單次 flush=true 排空，順序與 apply_dsp 對齊） |

結論：通過（SC#1、SC#2 皆已關閉，v7 Design Spec 審查通過）

### Code 階段
| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| F1 | 阻擋 | Design.md:60-62、151（v7）→ v8 已修正 | `RateStage::new()`、`run()` 為私有方法，僅型別可見 | `src-tauri/src/dsp.rs:540` 為 `pub(crate) struct`，但 547、550 行的 `new()`、`run()` 都是私有方法 | 將 `new()`、`run()` 改為 `pub(crate)`，傳入 `INTERNAL_CHANNELS as usize`；`dsp.rs` 列入變更清單 | 修正中（v10） |
| F2 | 阻擋 | Design.md:63-69、88、110（v7）→ v8 已修正 | `apply_dsp(&mut [f32; 2]) -> [f32; 2]` 一進一出契約不足以承接 RateStage | `src-tauri/src/dsp.rs:499-527` WSOLA 先累積再成批產出；`550-576` 可變數量樣本追加至 Vec | 改為 `apply_dsp_block(&self, input: &[f32], out: &mut Vec<f32>)`；PlaybackCore 保留 `dsp_out` 佇列 | 修正中（v10） |
| F3 | 阻擋 | Design.md:49-53、69、100（v7）→ v8 已修正 | `process()` 與 `pop_output()` 消耗契約未對齊；把區塊未滿當成靜音會插入額外音訊時間 | `src-tauri/src/playback_pipeline.rs:944-953、985` `process(input)` 已餵入樣本並自行呼叫 `pop_output()` | 先從 `dsp_out` 取樣本，不足再拉上游；`None` 表示未滿區塊而非靜音 | 修正中（v10） |
| F4 | 阻擋 | Design.md:61-62、129、150（v7）→ v8 已修正 | RateStage 建立時機未涵蓋長檔初次載入 | `src-tauri/src/player.rs:420-435` 長檔使用空 TrackChain；`playback_pipeline.rs:413-417` `sync_identity()` 遇到沒有目前軌就返回 | 軌載入/切換時（初次載入、手動換軌、seek、自然交接）建立 RateStage，依 `FxParams.allow_rate` 設定 | 修正中（v10） |
| F5 | 阻擋 | Design.md:66、93、110（v7）→ v8 已修正 | 「參數已透過版本號即時套用」不適用於現有 PlaybackCore 的 FxChain | `src-tauri/src/player.rs:159-160` 只建立預設 FxChain；899-900 僅更新 EqShared；`playback_pipeline.rs:494-500` 直接呼叫 `frame()`，沒有刷新參數 | PlaybackCore 新增 `dsp_params` 快照，命令執行緒更新，`apply_dsp_block` 讀取快照供 RateStage 與 FxChain 使用 | 修正中（v10） |
| F6 | 阻擋 | Design.md:20、50、152（v7）→ v8 已修正 | FFmpeg 長檔與 Symphonia 失敗後備路徑未納入 RateStage 建立 | `src-tauri/src/player.rs:468、471-472` FFmpeg 準備流程；539-549 長檔分支只呼叫 `TrackResampler::new()`，沒有 `set_track` | FFmpeg 長檔分支建立 RateStage 並呼叫 `set_track`；Symphonia 後備路徑納入統一安裝入口 | 修正中（v10） |
| F7 | 阻擋 | Design.md:75、82、101（v7）→ v8 已修正 | 不能直接沿用現行 EOF 流程保證「排空後進入 WaitingForNext」 | `src-tauri/src/playback_pipeline.rs:358-372` 每次遇到鏈結束都呼叫 `flush_tail()`；rubato `process_partial(None)` 是補零區塊，非查詢是否排空 | 上游只 flush 一次 → 全部尾段經 EQ／舊 RateStage → RateStage 只終結一次 → 排完 `dsp_out` → 進 WaitingForNext；不可重複 `flush_tail` | 修正中（v10） |
| F8 | 重要 | Design.md:76（v7）→ v8 已修正 | DSP 檢查點映射僅為註解依據，無對應實作 | `src-tauri/src/playback_pipeline.rs:53-55` 是註解；實際 453-455 行直接使用 `current_boundary_out_start()`，初值為 0 | 將映射列為待實作；以舊軌最後一個 DSP 輸出幀位置提交邊界 | 修正中（v10） |
| F9 | 重要 | Design.md:75、118（v7）→ v8 已修正 | 1024-frame 區塊拉取、天然背壓不足以證明不阻塞回呼 | `src-tauri/src/playback_pipeline.rs:31、1201、1214-1218` 核心鎖內拉取 2048 幀 | 區塊數值改為 2048；尾段用游標或佇列批次消耗；需量測換軌最壞耗時 | 修正中（v10） |
| Code 審查第 1 輪 | 通過 | — | — | — | — | F1–F9 皆在 v10 已修正，v10 Code 複查通過 |
| Code 審查第 2 輪 | 通過 | R2-1–R2-5 | 皆已關閉（v10） | — | — | R2-2、R2-3、R2-4 在第 2 輪關閉 |
| Code 審查第 3 輪 | 通過 | R3-1、R3-2 | 皆已關閉（v10） | — | — | R3-1、R3-2 在第 3 輪關閉 |

### Security 階段
| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| S1 | 次要 | Design.md:67–81 | 新增 Mutex 未明定鎖的釋放時點與巢狀持鎖限制（不阻擋） | 設計僅寫「取得快照」。現有 playback_pipeline.rs:486–501 明確先釋放 EQ 鎖，再取得 FX 鎖；eq.rs:50–52 複製參數後即釋放鎖。新增四把鎖尚未實作，無法核對實際持鎖時間。 | 明記 dsp_params 複製後立即解鎖；佇列取出／追加使用短鎖區段，DSP 各階段避免交疊持鎖。命令更新沿用相同規則。 | 通過（第 1 輪） |

## 實作審查（實作後）

### Spec 階段
| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
|---|---|---|---|---|---|---|
| — | — | — | — | — | — | — |

### Code 階段
|| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
||---|---|---|---|---|---|---|
|| I1 | 阻擋 | src-tauri/src/playback_pipeline.rs:593-601、`src-tauri/src/player.rs:914-917` | `dsp_params` 快照未更新、`FxChain::set()` 未呼叫，參數變更不生效 | `apply_dsp_block` 僅呼叫 `frame()`，未 `set()`；`player.rs SetFx` 僅更新 `EqShared` | `apply_dsp_block` 在 `frame()` 前呼叫 `p.set(&fx_params)`；`SetFx` 更新後同步 `*dsp_params.lock().unwrap() = Some(fx_params)` | 已修正 |
|| I2 | 阻擋 | src-tauri/src/playback_pipeline.rs:341-433 | 短檔 TrackChain 路徑使用舊單樣本 `apply_dsp(s)`，右聲道未處理 | `next_playing()` TrackChain 分支 `pop_output()` + `process()` 單樣本直接 `apply_dsp(s)` | 重寫 `next_playing()` 為單一 loop：所有來源 → `track_resampler.process()` → `track_out` → `apply_dsp_block([L,R])` → `dsp_out` | 已修正 |
|| I3 | 阻擋 | src-tauri/src/playback_pipeline.rs:389-410 | EOF 未實作一次性排空，舊 RateStage 被直接捨棄 | 舊程式碼鏈結束時直接 `flush_tail()` + `pop_output()` 一次後進 WaitingForNext | `next_playing()` 上游結束時 `flush_tail()` → 耗盡 `pop_output()` → `dsp_out`/`track_out` 皆空才進 WaitingForNext | 已修正 |
|| I4 | 重要 | src-tauri/src/player.rs:420-434 | Symphonia 長檔首次載入未建立 RateStage | `player.rs` Symphonia 長檔分支只建立 `TrackResampler`，無 `RateStage::new()` | Symphonia 長檔分支加入 `*rate_stage.lock().unwrap() = Some(RateStage::new(...))` | 已修正 |
|| I5 | 重要 | src-tauri/src/player.rs:480-484 | 手動換軌／seek 未清空 `track_out`/`dsp_out` | `load_at()` 安裝新來源後未清空 DSP 輸出佇列 | `load_at()` 安裝新來源後清空 `track_out` 與 `dsp_out` | 已修正 |
|| I6 | 重要 | src-tauri/src/playback_pipeline.rs:486-524 | 自然交接直接替換 RateStage，舊 RateStage 內部緩衝未排空 | `sync_identity()` 直接 `*rate_stage = Some(RateStage::new(...))` | `sync_identity()` 在替換前先以 `flush=false` 餵入舊 `pending_output`（經 EQ），再以 `flush=true` 空輸入排空殘餘，輸出進入 `dsp_out` | 已修正 |
|| I7 | 重要 | — | 測試覆蓋不足，未驗證新 drain order / RateStage rebuild | 既有測試未覆蓋 `dsp_out` 優先順序與軌切換尾端排空 | 新增 `test_playback_core_dsp_out_drain_order`、`test_playback_core_rate_stage_tail_drain` | 已修正（54/54 通過） |

結論：通過（I1–I7 皆已修正，cargo test --release 54/54 通過）

### Security 階段
|| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論 |
||---|---|---|---|---|---|---|
|| S1 | 次要 | open_with.rs:101、player.rs:368-370、audio_decode.rs:233、532 | 檔案路徑與子程序參數已有現有防護（validate_local_path 拒絕遠端/UNC 路徑；ffprobe/ffmpeg 使用參數陣列與 file,pipe 白名單），本次未新增路徑操作、登錄檔存取、本機連接埠、Tauri capability 或權限提升 | 現有防護足夠，無需額外變更 | 已檢查，無新增安全缺失 |

結論：通過（限本次 Implementation Security 範圍，未發現新增的阻擋、重要或次要安全缺失；僅基於唯讀差異與呼叫鏈檢查）