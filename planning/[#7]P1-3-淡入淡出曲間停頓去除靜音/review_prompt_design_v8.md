# P1-3 Design v8 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v8，2026-11-25)

## v8 修正摘要（針對 v7 審查 #7a/7b/7c）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #7a 交接觸發時機過早 | D:161、222、223 | `poll_completion` 僅保存準備結果，不安裝播放；交接由 `TrackChain::next()` 曲終／gap 耗盡觸發，設 `pending_stream`，gap 耗盡後由 `PlaybackCore::next()` 安裝 |
| #7b 偏移單位不一致 | D:161、222 | 安裝 Stream 軌時設 `seek_origin_ms = position_ms` + `reset_played_out_frames()`，避免與 48 kHz 雙聲道內部計數重複相加 |
| #7c 安裝契約缺重取樣器與身分更新 | D:161、222 | 安裝 Stream 軌同步清除舊 TrackChain、以新軌 rate/channels 重設重取樣器、更新 seq/token/song_id/path/decoder |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 在 `OutputProxy::next()` 實際輸出時推進、完成通知時機、各路徑取樣率換算是否正確
4. gap 長度計算 `gap_ms × 48000 × 2 / 1000` 是否正確，消耗位置是否單一
5. 去除靜音裁切後 `native_frame_pos` 從 0 起算，leading 偏移獨立接入進度，`TrackResampler::set_track` 不從 leading 起算
6. Switch/PendingLoad 載入契約是否完整（暫停/無音源分支）
7. AppConfig 9 個 key（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
8. 預載串流交接流程是否完整（曲終觸發、開流、安裝來源、重取樣器、身分更新、進度）
9. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
10. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
11. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
