# P1-3 Design v10 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v10，2026-11-25)

## v10 修正摘要（針對 v9 審查 #1–#3）
| 原阻擋 | 修正位置 | 核心修正 |
|--------|----------|----------|
| #1 離開 WaitingForNext 狀態轉移未定義 | D:161、220、222 | 安裝後由命令執行緒 tick 處理 `StreamInstalled` → `sink.play()` + 設 `Playing`，暫停狀態保留；記憶體軌與串流軌曲終共用 `TrackChain::next()` 曲終＋gap 耗盡觸發交接；串流曲終進入 WaitingForNext，由下一個 PreparedStream 經相同開流→安裝→sink.play() 重新安裝 |
| #2 done_target = Switch 兩種動作衝突 | D:92、124、161、222 | 區分 `SwitchLoad`（手動切歌載入新軌）與 `StreamInstalled`（自然交接已安裝僅恢復播放），各自指定世代、所需資料與淡入時機 |
| #3 安裝步驟退漏重取樣器與完整播放身分更新 | D:161、192、220 | 恢復 v8 安裝契約：清除舊 TrackChain、以新軌 rate/channels 重設重取樣器、更新 seq/token/path/decoder；保留 v9 的 stream_song_id/stream_duration_ms/stream_seek_origin_ms 快照欄位 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 在 `OutputProxy::next()` 實際輸出時推進、完成通知時機、各路徑取樣率換算是否正確
4. gap 長度計算 `gap_ms × 48000 × 2 / 1000` 是否正確，消耗位置是否單一
5. 去除靜音裁切後 `native_frame_pos` 從 0 起算，leading 偏移獨立接入進度，`TrackResampler::set_track` 不從 leading 起算
6. SwitchLoad/PendingLoad 載入契約是否完整（暫停/無音源分支）
7. AppConfig 9 個 key（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
8. 預載串流交接流程：開流在命令執行緒、安裝在曲終/gap 耗盡、sink.play() 在命令執行緒、串流中繼資料獨立欄位、與 WaitingForNext 接合、離開 WaitingForNext 的狀態轉移
9. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
10. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
11. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
