# P1-3 Design v7 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v7，2026-11-25)

## v7 修正摘要（針對 v6 審查 #7）
| 原重要 | 修正位置 | 核心修正 |
|--------|----------|----------|
| #7 預載串流交接流程未安排 | D:161、198、220–222 | §5 串流交接明確指定消費端為 `player.rs` 命令迴圈 `poll_completion` 處理（`:1105-1112`），在 `Ok(track)` 分支中判斷 `TrackData::Stream`，讀取 `FfmpegDeferred.position_ms`、`SymStream::open` 開流、安裝 `core.set_stream_source()`、寫入 `played_out_frames`（`position_ms × rate × channels / 1000`）、呼叫 `sink.play()`；實作步驟第 5 點同步加入 `poll_completion` Stream 處理 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 在 `OutputProxy::next()` 實際輸出時推進、完成通知時機、各路徑取樣率換算是否正確
4. gap 長度計算 `gap_ms × 48000 × 2 / 1000` 是否正確，消耗位置是否單一
5. 去除靜音裁切後 `native_frame_pos` 從 0 起算，leading 偏移獨立接入進度，`TrackResampler::set_track` 不從 leading 起算
6. Switch/PendingLoad 載入契約是否完整（暫停/無音源分支）
7. AppConfig 9 個 key（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
8. 預載串流交接流程是否完整（消費端、開流、安裝來源、更新進度）
9. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
10. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
11. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
