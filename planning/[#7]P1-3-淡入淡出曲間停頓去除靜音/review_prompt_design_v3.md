# P1-3 Design v3 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v3，2026-11-25)

## v3 修正摘要（針對 v2 審查 #1–#8）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 去除靜音重複跳過、frame/樣本單位混用 | D:44、R5 | 只在一處跳過（collect_pcm_limit 後），完整聲道 frame 對齊，`ms = frames × 1000 / (rate × channels)`，與 player.rs:275 一致 |
| #2 淡出後恢復全增益、完成通知未綁緩衝 | D:60–75、R1 | 段落結束鎖定 to_gain 不恢復 1.0，每個輸出樣本獨立線性插值，FadeGate 在 OutputProxy 每輸出樣本消耗 |
| #3 Load 後保險 Play 與取消衝突、FadeSegment 缺待載入資料 | D:19、R3 | 定義 `PendingLoad`、`FadeTarget::Switch`，新 Play 撤銷舊 Switch（gen 不符），Next|Prev 目標來自 NextCoordinator 待播槽 |
| #4 停頓與預載修剪資料鏈未補齊 | D:45、R4/R5 | 指定 `TrackChain::next()`（`:1058-1061`）產生靜音不累計進度；補齊 PreparedTrack → TrackChainItem 偏移傳遞 |
| #5 兩條 DSP 路徑淡段共享不一致 | D:58、R1 | 設定 EqShared，執行中段落 PlaybackCore.FadeGate；OutputProxy 讀 core.fade_gate；DspSource 新增 Arc<FadeGate> |
| #6 互斥持久化與啟動順序只是待辦 | D:127、142–145 | 權威來源 app_config，雙向清零保存，啟動套用順序，session 快照規則 |
| #7 串流頭部掃描無具體方案 | D:134、R5 | 背景執行緒，60 s / 50 MB 上限，到限視為無靜音，取消 kill ffmpeg，標為提案 |
| #8 完成條件缺可重現方法 | D:178–192 | 補充素材、操作、觀察點、記錄方式；分開包絡數值驗證與聽測 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 樣本-based 包絡、Load/PendingLoad 流程、暫停中 Seek、去除靜音 frame 對齊、playback_pipeline.rs 實際交接位置是否正確
4. AppConfig 新欄位與 Default、gap/crossfade 互斥持久化是否完整
5. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
6. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
7. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
