# P1-3 Design v11 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v11，2026-11-25)

## v11 修正摘要（針對 v10 審查 #1–#3）
| 原阻擋 | 修正位置 | 核心修正 |
|--------|----------|----------|
| #1 串流曲終→記憶體軌交接未閉合，gap 啟動點未接上 | D:161、176–183、242 | 明列串流 EOF → 記憶體軌共同曲終交接入口：串流曲終進入 WaitingForNext，由 NextCoordinator 準備好的下一軌（Memory）在曲終/gap 耗盡時安裝並恢復 Playing，gap 啟動一次，串流與記憶體軌共用同一入口 |
| #2 `StreamInstalled` 完成世代與待安裝來源未綁定 | D:87–105、138–140、176–182 | `StreamMeta` 新增 `gen: u64`，安裝前核對 `meta.gen == done_target.1`；`player_load`/`Seek`/`ClearNext` 遞增 `pending_gen` 並清空 `pending_stream_source/meta`；`StreamInstalled` 使用完成世代 |
| #3 開流執行緒與時機三種描述不一致 | D:87、176、244 | 統一：`poll_completion`（命令執行緒）收到 Stream PreparedTrack → 背景執行緒開流 → 結果存入 `pending_stream_source/meta`；安裝在曲終/gap 耗盡由 `PlaybackCore::next()` 執行；`sink.play()` 由命令執行緒 tick 處理 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 在 `OutputProxy::next()` 實際輸出時推進、完成通知時機、各路徑取樣率換算是否正確
4. gap 長度計算 `gap_ms × 48000 × 2 / 1000` 是否正確，消耗位置是否單一
5. 去除靜音裁切後 `native_frame_pos` 從 0 起算，leading 偏移獨立接入進度，`TrackResampler::set_track` 不從 leading 起算
6. SwitchLoad/PendingLoad 載入契約是否完整（暫停/無音源分支）
7. AppConfig 9 個 key（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
8. 預載串流交接流程：開流在命令執行緒、安裝在曲終/gap 耗盡、sink.play() 在命令執行緒、串流中繼資料獨立欄位、與 WaitingForNext 接合、離開 WaitingForNext 的狀態轉移
9. 串流世代綁定：`StreamMeta.gen` 與 `done_target.1` 匹配，Load/Seek/ClearNext 使 `pending_stream_source/meta` 失效
10. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
11. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
12. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
