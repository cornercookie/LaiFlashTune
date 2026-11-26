# P1-3 Design v2 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v2，2026-11-25)

## v2 修正摘要（針對 v1 審查 S1–S10）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| S1 開始播放/切歌未接實際入口 | D:19、R1/R3 | 接 `Shell.tsx:578-600` `playSong → playerLoad → playerPlay` 實際路徑；`Next|Prev` 不再為空 |
| S2 Seek 改變暫停狀態、時間矛盾 | D:18、R2 | 保留 `engine.is_playing`，暫停中不播放，淡出/淡入統一 `seek_ms`/`play_ms` |
| S3 修剪重複跳過、樣本數非播放位置 | D:118、R5 | 完整聲道 frame 對齊，`ms = frames / (sample_rate × channels) × 1000`，保留原始 duration 與 Seek 座標 |
| S4 曲間停頓/修剪位置與資料鏈不完整 | D:113、138、R4/R5 | 更正為 `playback_pipeline.rs:925` `TrackChainItem`、`:1749` `NextCoordinator`、`:1911` `take_next`、`:1081` `prepared_to_trackchain_item` |
| S5 單調時鐘非音訊時間 | D:66、78-86、R1 | FadeGate 改用輸出 frame 計數推進包絡，定義完成通知與緩衝消耗關係 |
| S6 FadeGate 共享與舊動作取消契約缺失 | D:62-75、R1 | 區分 EqShared 設定與 active_segment + pending_gen，新指令遞增 gen 撤銷舊完成通知 |
| S7 AppConfig 欄位與 Default 缺漏、互斥未涵蓋 session | D:126-140、R4/R7 | 補列 AppConfig 8 欄位與 Default，gap/crossfade 互斥持久化與套用順序 |
| S8 400 MB/10 s 保護不足以界定串流掃描 | D:119、R5 | 串流頭部掃描限制標為提案，不冒充現有保護 |
| R6 睡眠計時現況與原始碼不符 | D:22、R6 | Shell.tsx:516-532 已有倒數，:524 呼叫 `playerPause()`，納入本次驗收 |
| S10 完成條件缺可重現方法 | D:163、完成條件 | 補足素材、操作步驟、觀察點、記錄方式；區分包絡數值驗證與實際聽測 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate frame-based 包絡、Load 路徑整合、暫停中 Seek 行為、去除靜音 frame 對齊、playback_pipeline.rs 實際交接位置是否正確
4. AppConfig 新欄位與 Default、gap/crossfade 互斥持久化是否完整
5. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
6. 完成條件是否可量測、可重現（cargo test / node --test / tauri build）
7. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
