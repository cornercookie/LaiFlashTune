# P1-3 Design v22 — Design 安全審查請求 (Security 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Security 階段），確認威脅面與防護措施在設計契約層級是否完整，並符合現有 security review patterns（見 hermes skill `sonicledger` 的 Security 防禦模式）。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v22，2026-11-25)

## v22 修正摘要（針對 Security v2 SE1，已落入正文）
| 原重要 | 修正位置 | 核心修正 |
|--------|----------|----------|
| SE1 唯讀快照與回呼更新責任矛盾，跨執行緒同步尚未閉合 | D:113–134、136–145、205–210 | §1.1 程式碼塊明確回呼每個樣本持鎖更新 `consumed_samples`、`base_gain`、`active`、`done_target`，命令執行緒持鎖寫入新段落、世代、ready 標記，兩條路徑均不在鎖內進行阻塞操作；§1.5 回呼每個樣本持鎖讀取並更新可變狀態，命令執行緒持鎖寫入，完成通知單次發布，鎖順序禁止持鎖等待 I/O／回收，回呼遇到競爭使用上一快照並由新快照的 `consumed_samples` 繼續進度；§6 兩條 DSP 路徑的讀寫責任同步調整 |

## 程式基準（與 Design.md 一致）
`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/player.rs`、`src-tauri/src/audio_decode.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/eq.rs`、`src-tauri/build.rs`、`src-tauri/capabilities/default.json`

## 與 Security 防禦模式對照的設計要點
1. **B1 本機播放不跨入網路**：本任務不新增網路路徑；串流來源為本機長檔（SymStream/FFmpeg 本地檔案），不涉 UNC/URL。ffmpeg/ffprobe 只針對本地路徑。
2. **I1 記憶體上限**：淡段以輸出樣本計數（不累積大緩衝）；gap 以固定樣本數靜音；trim_silence 在解碼後掃描，不額外保留完整原始 PCM；串流來源為迭代器逐塊輸出，不一次讀入整檔。
3. **I2 音訊回呼不持鎖等 I/O**：`set_stream_source()` 不在音訊回呼內調用；`next()` 僅設 `stream_install_ready` 標記，實際安裝由命令執行緒 tick 執行；FadeGate 增益計算持鎖操作極短，無阻塞 I/O。
4. **I3 子程序清理不阻塞**：FFmpeg 開流/安裝後失效與清槽，依「完整來源回收契約」在鎖內取出完整來源，釋鎖後交回收執行緒；回收用 `try_wait()` / `is_finished()` + deadline，逾時 `kill()` + `drop()`，不阻塞 `wait()` / `join()`。
5. **I4 單一實例 socket**：本任務不涉 socket，無需新增。
6. **I5 錯誤日誌遮蔽**：ffmpeg stderr 經現有 `spawn_stderr_sanitizer` 處理；新增指令錯誤以代碼 + 經過清洗的原因回傳前端。
7. **I6 工具搜尋順序 / 依賴鎖定**：本任務不新增外部工具搜尋，沿用既有 `bin\` → PATH 順序。
8. **I7 Socket 總期限**：本任務不涉 socket，無需新增。
9. **SE10 能力權限**：三新指令需加入 `capabilities/default.json` 的 `allow-*` 權限（D:203-205、251-254 已列）。

## 新增威脅面（本任務特有，已在 v21–v22 補齊契約）
- FadeGate `stream_gen` / `pending_stream_source` / `pending_stream_meta` / `stream_install_ready` 為共享狀態，由命令執行緒寫、音訊回呼讀，已定義 `Arc<Mutex<FadeGate>>` 同步與單次消費規則。
- 串流來源持有獨立取消控制，過期交付與清槽時的來源交換須不持鎖等待 ffmpeg 程序退出。
- `stream_install_ready` 由 `next()`（音訊回呼執行緒）設定，由命令執行緒 tick 消費，已定義單一消費者與世代比對失效規則。
- 背景開流／掃描綁定獨立執行期限與監督者，開流前取得配額，回收中仍占配額。

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
