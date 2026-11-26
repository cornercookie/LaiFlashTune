# P1-3 Design v4 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v4，2026-11-25)

## v4 修正摘要（針對 v3 審查 #1–#8）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 frame/樣本混用、重複跳過、進度不讀偏移 | D:44、R5 | 只在一處跳過，`ms = samples × 1000 / (rate × channels)` 僅除以 rate 之外不再除以 channels，與 player.rs:275 一致；`native_frame_pos` 與 `leading_silence_samples` 同單位 |
| #2 base_gain 鎖定矛盾、電台完成通知早於緩衝 | D:60–75、R1 | 段落結束鎖定 `base_gain = to_gain` 不恢復 1.0，移除全通分支矛盾，OutputProxy 每輸出樣本推進 |
| #3 Load 保險 Play 撤銷 Switch、PendingLoad 未解決 | D:19、118、129、R3 | 定義 `PendingLoad`，前端 `playSong` 移除保險 `playerPlay()`，後端 `Load`/`Play` handler 檢查進行中的淡段，Switch 淡出歸零後載入 |
| #4 gap 無進度標記、受格式影響 | D:20、138、R4 | gap 在 `PlaybackCore::next()`（`:351`）層級以 `gap_remaining` 樣本產生靜音，不累計 `played_out_frames` |
| #6 持久化互相覆蓋、無獨立 crossfade key | D:175、198–201、R11 | gap/crossfade 獨立 app_config key，session 僅從權威值更新不反向覆寫，補 `crossfade_ms` 欄位 |
| #7 串流掃描接入點錯誤 | D:185–188、R5 | 接入 `player.rs:457-467`（直接載入）與 `playback_pipeline.rs:1817`（預載 `prepare_track`），偏透過 `position_ms` 傳入 |
| #8 驗收覆蓋不完整 | D:217–236、完成條件 | 補充曲間停頓、長檔、睡眠計時的素材、操作、預期結果與記錄方式 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate sample-based 包絡、Load/PendingLoad 流程、暫停中 Seek、去除靜音 frame 對齊、PlaybackCore::next() gap 實作是否正確
4. AppConfig 新欄位（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
5. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
6. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
7. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
