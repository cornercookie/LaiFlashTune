# P1-3 Design v6 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v6，2026-11-25)

## v6 修正摘要（針對 v5 審查 #1–#4、#7–#8）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 裁切後起算位置矛盾 | D:162、R5 | `TrackResampler::set_track` 從 0 起算（不從 leading_silence_samples），裁切後樣本從 0 消耗，leading 偏移僅供原檔時間換算 |
| #2 毫秒轉樣本數取樣率未定義 | D:62、75、97–114 | FadeGate `total_samples` 用各消耗端自身取樣率與聲道數換算；gap 固定 48 kHz 雙聲道，不受各路徑取樣率影響 |
| #3 暫停/無音源 Switch 路徑缺失 | D:19、127–140 | 有舊軌輸出才等待淡出；暫停/無音源時直接載入，新軌輸出前設定淡入 |
| #4 gap 消耗位置兩套契約 | D:97–103、149、220 | gap 僅在固定 48 kHz core 層產生與遞減，移除 OutputProxy 的 gap 遞減 |
| #7 預載串流偏移消費端未安排 | D:161、189、198 | 實際交接/開流端讀取 `FfmpegDeferred.position_ms` 並安裝來源 |
| #8 驗收前提不符 | D:249–251 | 曲間停頓改用「排入兩首到佇列」而非順序模式自然播完；長檔素材補上已知尾端靜音 |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 在 `OutputProxy::next()` 實際輸出時推進、完成通知時機、各路徑取樣率換算是否正確
4. gap 長度計算 `gap_ms × 48000 × 2 / 1000` 是否正確，消耗位置是否單一
5. 去除靜音裁切後 `native_frame_pos` 從 0 起算，leading 偏移獨立接入進度，`TrackResampler::set_track` 不從 leading 起算
6. Switch/PendingLoad 載入契約是否完整（暫停/無音源分支）
7. AppConfig 9 個 key（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
8. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
9. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
10. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
