# P1-3 Design v24 — Design 安全審查請求 (Security 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Security 階段），確認威脅面與防護措施在設計契約層級是否完整，並符合現有 security review patterns（見 hermes skill `sonicledger` 的 Security 防禦模式）。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v24，2026-11-25)

## v24 修正摘要（針對 Security v4 SE1-a，已落入正文）
| 原重要 | 修正位置 | 核心修正 |
|--------|----------|----------|
| SE1-a 電台進度更新仍同時指定 `fill()` 與實際取樣出口 | D:138、210、235、263 | 電台路徑由 `next()`（實際逐樣本取樣出口）而非 `fill()` 推進進度與完成通知，刪除 `fill()` 更新淡段狀態的指示；§1.5、§6、§8 資料變更、§13 實作步驟第 4 點同步調整；`Arc<FadeGate>` 全部統一為 `Arc<Mutex<FadeGate>>` |

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

## 新增威脅面（本任務特有，已在 v21–v24 補齊契約）
- FadeGate `stream_gen` / `pending_stream_source` / `pending_stream_meta` / `stream_install_ready` 為共享狀態，由命令執行緒寫、音訊回呼讀，已定義 `Arc<Mutex<FadeGate>>` 同步與單次消費規則。
- 串流來源持有獨立取消控制，過期交付與清槽時的來源交換須不持鎖等待 ffmpeg 程序退出。
- `stream_install_ready` 由 `next()`（音訊回呼執行緒）設定，由命令執行緒 tick 消費，已定義單一消費者與世代比對失效規則。
- 背景開流／掃描綁定獨立執行期限與監督者，開流前取得配額，回收中仍占配額。
- 本機與電台兩條輸出路徑均由 `next()`（實際逐樣本取樣出口）推進 FadeGate 進度，已定義本地計數與合併責任。

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
