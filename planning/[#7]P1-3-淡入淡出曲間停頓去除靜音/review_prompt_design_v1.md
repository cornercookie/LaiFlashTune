# P1-3 Design v1 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v1，2026-11-25 初版)

## v1 修正摘要
無（首版）

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 淡入淡出狀態機、曲間停頓、去除靜音做法在現有程式與套件上是否可行（threading/cancellation races、音訊回呼鎖、子程序清理、記憶體上限）
4. 資料變更（8 個 app_config key、TrackChainItem/EngineSnapshot/EqShared 欄位）是否缺漏；Tauri command 引數用 camelCase
5. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
6. 完成條件是否可量測、可重現（cargo test / node --test / tauri build）
7. 風險是否涵蓋；控制項要能驅動引擎才能畫
8. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
