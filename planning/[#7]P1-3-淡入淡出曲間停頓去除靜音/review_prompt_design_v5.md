# P1-3 Design v5 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v5，2026-11-25)

## v5 修正摘要（針對 v4 審查 #1–#4、#6–#8）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 裁切後再跳過、進度偏移未接入 | D:44、R5 | `native_frame_pos` 從 0 起算（索引裁切後陣列），導靜音偏移獨立接入 `player.rs:275` 進度計算，公式不再除以 channels 兩次 |
| #2 包絡完成早於緩衝消耗 | D:97、110–114 | FadeGate 在 `OutputProxy::next()` 實際輸出取樣時推進，完成通知等淡出樣本消耗完成 |
| #3 Switch 載入契約不完整 | D:19、127–140 | 前端 `playSong` 移除保險 `playerPlay()`，後端 `Load`/`Play` handler 檢查進行中的淡段，設 `PendingLoad`，Switch 淡出歸零後載入 |
| #4 gap 長度少算一半 | D:20、46 | gap 用 `gap_ms × 48000 × 2 / 1000` 個交錯樣本計數，不累計 `played_out_frames` |
| #6 crossfade 啟動接線不完整 | D:186、210–213 | 新增 `crossfade_ms` 獨立 key（共 9 個），啟動載入 9 個 key 套入 `engine.crossfade_ms`/`core.set_crossfade_ms`，session 不反向覆寫 |
| #7 預載串流偏移未傳遞 | D:155、196–197 | 接入 `player.rs:457-467` 與 `playback_pipeline.rs:1817`，`FfmpegDeferred` 新增 `position_ms` 欄位 |
| #8 驗收證據不足 | D:249–251 | 補充曲間停頓（192,000 樣本 = 2000 ms 量測）、長檔（已知尾端靜音素材）、睡眠計時（錄音顯示先漸弱後靜音）的素材、操作、預期結果與記錄方式 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 在 `OutputProxy::next()` 實際輸出時推進、完成通知時機是否正確
4. gap 長度計算 `gap_ms × 48000 × 2 / 1000` 是否正確（48 kHz 雙聲道交錯樣本）
5. 去除靜音裁切後 `native_frame_pos` 從 0 起算，導靜音偏移獨立接入進度，公式不重複除以 channels
6. Switch/PendingLoad 載入契約是否完整（`Load`/`Play` handler 檢查進行中的淡段）
7. AppConfig 9 個 key（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
8. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
9. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
10. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
