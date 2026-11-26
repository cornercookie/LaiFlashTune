# P1-3 Design v21 — Design 安全審查請求 (Security 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Security 階段），確認威脅面與防護措施在設計契約層級是否完整，並符合現有 security review patterns（見 hermes skill `sonicledger` 的 Security 防禦模式）。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v21，2026-11-25)

## v21 修正摘要（針對 Security v1 SE1–SE4，已落入正文）
| 原重要 | 修正位置 | 核心修正 |
|--------|----------|----------|
| SE1 FadeGate 跨執行緒同步方式未定義 | D:99–109、133–150、194–197 | 新增 §1.5「跨執行緒同步契約（FadeGate）」：FadeGate 包裝於 `Arc<Mutex<FadeGate>>`，命令執行緒寫入、回呼與電台路徑唯讀快照；段落與世代在同一同步區段讀寫；完成通知單次消費；鎖順序禁止持鎖等待 I/O／回收；回呼無法取得鎖時使用上一快照繼續輸出 |
| SE2 ready 標記沒有綁定世代，失效操作未同步撤銷 | D:106、181–186 | `stream_install_ready` 改為 `Option<u64>` 攜帶 `stream_gen`，tick 在同一同步區段核對世代、交接狀態及 gap，再取出來源並消費 ready（僅一次） |
| SE3 掃描量上限不能代替實際經過時間的期限 | D:224–232 | §9 串流頭部靜音掃描補充獨立執行期限與監督者：預設 10 秒，期限與「無新區塊進入」任一觸發即 `cancel` + `kill()`；`stdout.read()` 阻塞由監督者以 `recv_timeout` 監控，逾時無新資料視為卡住並終止；Symphonia 卡住以同一監督者隔離並保留資源計數；60 秒／50 MB 僅限制已解碼量，不等同於執行期限 |
| SE4 新增背景開流／掃描未接入完整生命週期配額 | D:179、188、224–232 | §5 補充「開流／掃描配額契約」：啟動前取得配額，過期交付、排隊回收及未終止工作持續佔用；配額耗盡或回收佇列滿時非阻塞處置，完成回收後才釋放；回收中仍占配額，清理完成前阻止新準備 |

## 程式基準（與 Design.md 一致）
`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/player.rs`、`src-tauri/src/audio_decode.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/eq.rs`、`src-tauri/build.rs`、`src-tauri/capabilities/default.json`

## 與 Security 防禦模式對照的設計要點
1. **B1 本機播放不跨入網路**：本任務不新增網路路徑；串流來源為本機長檔（SymStream/FFmpeg 本地檔案），不涉 UNC/URL。ffmpeg/ffprobe 只針對本地路徑。
2. **I1 記憶體上限**：淡段以輸出樣本計數（不累積大緩衝）；gap 以固定樣本數靜音；trim_silence 在解碼後掃描，不額外保留完整原始 PCM；串流來源為迭代器逐塊輸出，不一次讀入整檔。
3. **I2 音訊回呼不持鎖等 I/O**：`set_stream_source()` 不在音訊回呼內調用；`next()` 僅設 `stream_install_ready` 標記，實際安裝由命令執行緒 tick 執行；FadeGate 增益計算不持核心鎖等待。
4. **I3 子程序清理不阻塞**：FFmpeg 開流/安裝後失效與清槽，依「完整來源回收契約」在鎖內取出完整來源，釋鎖後交回收執行緒；回收用 `try_wait()` / `is_finished()` + deadline，逾時 `kill()` + `drop()`，不阻塞 `wait()` / `join()`。
5. **I4 單一實例 socket**：本任務不涉 socket，無需新增。
6. **I5 錯誤日誌遮蔽**：ffmpeg stderr 經現有 `spawn_stderr_sanitizer` 處理；新增指令錯誤以代碼 + 經過清洗的原因回傳前端。
7. **I6 工具搜尋順序 / 依賴鎖定**：本任務不新增外部工具搜尋，沿用既有 `bin\` → PATH 順序。
8. **I7 Socket 總期限**：本任務不涉 socket，無需新增。
9. **SE10 能力權限**：三新指令需加入 `capabilities/default.json` 的 `allow-*` 權限（D:203-205、251-254 已列）。

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
