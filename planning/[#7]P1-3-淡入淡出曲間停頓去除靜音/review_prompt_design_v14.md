# P1-3 Design v14 — Design 設計審查請求 (Spec 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Spec 階段），確認需求、產品原則、完成條件是否對得上，有沒有缺漏。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v14，2026-11-25)

## v14 修正摘要（針對 v13 審查 #1–#2）
| 原阻擋 | 修正位置 | 核心修正 |
|--------|----------|----------|
| #1 串流 EOF 的 gap 啟動未接入實作步驟 | D:164、180–183、244 | 串流曲終進入 WaitingForNext 時若 gap_ms > 0 且 crossfade_ms == 0，一次性設 gap_remaining，等待期間不得重設計數；Memory 軌從 NextCoordinator.take_next() 取得，prepared_to_trackchain_item → 重建 TrackChain（清空舊、append 新軌）、重設 TrackResampler，設 done_target = StreamInstalled；Memory／Stream 共用 WaitingForNext 耗盡後的交接入口 |
| #2 過期開流結果仍可覆蓋有效的待安裝來源 | D:96、106、179–184、247 | StreamMeta 新增 gen: u64；開流交付前先核取 `meta.gen == pending_gen`，核取與寫入須與取消操作序列化；過期結果不得覆寫來源、metadata 或 install_gen；安裝前再核取目前有效世代；安裝成功後以該世代建立 Some((StreamInstalled, install_gen)) |

## 審查重點
1. 需求對照表 R1–R6 是否完整涵蓋 planning.md Acceptance Criteria
2. 現況（檔案：行號）是否與原始碼實際內容相符（需以原始碼行號為準，核對不了的標推測）
3. FadeGate 在 `OutputProxy::next()` 實際輸出時推進、完成通知時機、各路徑取樣率換算是否正確
4. gap 長度計算 `gap_ms × 48000 × 2 / 1000` 是否正確，消耗位置是否單一
5. 去除靜音裁切後 `native_frame_pos` 從 0 起算，leading 偏移獨立接入進度，`TrackResampler::set_track` 不從 leading 起算
6. SwitchLoad/PendingLoad 載入契約是否完整（暫停/無音源分支）
7. AppConfig 9 個 key（含 crossfade_ms）與 Default、gap/crossfade 互斥持久化是否完整
8. 預載串流交接流程：開流在命令執行緒、安裝在曲終/gap 耗盡、sink.play() 在命令執行緒、串流中繼資料獨立欄位、與 WaitingForNext 接合、離開 WaitingForNext 的狀態轉移
9. 串流世代綁定：`StreamMeta.gen` 與 `install_gen` 匹配，Load/Seek/ClearNext 使 `pending_stream_source/meta` 失效，交付前核取 `meta.gen == pending_gen`
10. 三新 Tauri 指令是否三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
11. 完成條件與驗收方法是否可量測、可重現（cargo test / node --test / tauri build）
12. repo 是公開的，文件內不得出現參考播放器名稱（執行前 git grep -n -i -e 'a[i]mp' 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
