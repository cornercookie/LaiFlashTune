# [#1]P0-SE3-跨區塊查詢清洗狀態 - 審查紀錄

> 審查流程：Spec → Code → Security 三階段，各階段阻擋問題修完才進下一階段。結果記在此檔。

---

## 設計審查（實作前）

### Spec 階段

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 狀態 |
|---|---|---|---|---|---|---|
| SC1 | 阻擋 | Design.md §詳細設計/變更1, §架構變更圖, audio_decode.rs:461-488 | 設計未處理 stderr 執行緒現有的 4 KB `last_stderr` 緩衝區。現狀：逐區塊 `sanitize_log` 後累積至 `last_stderr`（最多 4 KB），結束時再 `sanitize_log` 整體（audio_decode.rs:486 雙重清洗）。新設計改用持久 `Sanitizer` 累積並 `finish()` 截斷 1 KB，但設計文件未說明是否移除 4 KB 緩衝區、如何銜接 `log_line`，導致實作時可能保留舊邏輯產生雙重截斷/雙重清洗。 | audio_decode.rs:461 `last_stderr` 宣告；474-477 區塊清洗後累積並截斷 4 KB；486 `log_line(&sanitize_log(&text))` 雙重清洗。Design.md 第 54 行稱「若結果非空，再經 log_line 寫入日誌」，但未提 4 KB 緩衝區去留。 | 在「詳細設計/變更 1」明確列出：移除 `last_stderr` 緩衝區與區塊級截斷邏輯；stderr 執行緒直接用單一 `Sanitizer` 累積，循環結束（EOF/錯誤/cancelled）統一呼叫 `finish()` 取得 ≤1 KB 結果，直接傳給 `log_line`（接受雙重 `sanitize_log` 冪等）。更新架構圖移除 4 KB 節點。 | 已修正 |
| SC2 | 重要 | Design.md §需求對照表 R4、§測試案例表、§驗收條件對照 AC1 | 測試案例表稱「新增：單一區塊 query 移除」對應 AC1，但現有 `test_sanitize_log_removes_credentials` (open_with.rs:648) 已測單一區塊 query 移除（透過 `sanitize_log`）。設計未釐清：是否需新增 `Sanitizer::sanitize_chunk` 單區塊 query 測試、或現有測試足夠。若後者，「新增」標記誤導。 | open_with.rs:648-653 `test_sanitize_log_removes_credentials` 驗證 `sanitize_log("https://user:pass@example.com/path?token=abc")` 移除 query。Design.md 第 124 行列為「新增」。 | 確認測試策略：(A) 既有測試涵蓋 `sanitize_log` 單區塊行為，且 `sanitize_log` 內部委派 `Sanitizer`，視為間接覆蓋 → 將表格改為「現有」；(B) 需直接測試 `Sanitizer::sanitize_chunk` 單區塊 query → 明確新增測試並說明理由。建議採 (A) 並修正表格。 | 已修正 |
| SC3 | 重要 | Design.md §需求對照表 R4、§測試案例表 | R4 說「新增跨區塊查詢殘留測試」，測試案例表卻將 `test_sanitizer_cross_chunk_query_removed` 標為「現有」對應 AC2。兩處自相矛盾：若測試已存在，R4 不應寫「新增」；若 R4 要求新增，表格不應標「現有」。需釐清是否尚缺測試。 | Design.md 第 23 行 R4「新增跨區塊查詢殘留測試」；第 120 行測試表 `test_sanitizer_cross_chunk_query_removed` 標「現有」。open_with.rs:194-201 確認該測試已存在。 | R4 改為「既有測試 `test_sanitizer_cross_chunk_query_removed` 已覆蓋 AC2，無需新增」；或若需補充測試（如 @ 遮蔽後 query），在 R4 具體列出新增測試名稱。 | 已修正 |
| SC4 | 次要 | Design.md §詳細設計/變更 1、§錯誤處理、audio_decode.rs:464-480 | 錯誤處理稱「區塊讀取錯誤維持現狀 break，再呼叫 finish() 輸出目前累積結果」，但未具體列出三條 break 路徑（`cancel_stderr.is_cancelled()`、讀取 `Ok(0)` EOF、讀取 `Err(_)`）是否都會執行 `finish()`。實作時易漏掉 cancelled/EOF 路徑。 | audio_decode.rs:464-466 cancelled break；468 EOF break；479 Err break。Design.md 第 71 行泛指「所有 break 路徑後統一呼叫 finish()」但未展開。 | 在「錯誤處理」或「變更 1」以列表明確三條路徑：`cancelled`、`Ok(0)`、`Err(_)`，並在每條後呼叫 `sanitizer.finish()`；或在循環外用 `defer`/`finally` 模式（Rust 用 `drop` guard 或單獨函式）保證執行。 | 已修正 |
| SC5 | 次要 | Design.md §變更檔案清單 | 檔案清單將 `open_with.rs` 標為「無變更（可能新增測試）」，但新增測試屬於修改該檔案，應標為「修改（新增測試）」，避免審查者誤以為檔案完全不動。 | Design.md 第 132 行「無變更（可能新增測試）」；第 133 行另列 `open_with.rs (tests)` 為「新增」。同一檔案拆兩行易混淆。 | 合併為單行：`src-tauri/src/open_with.rs` | 修改 | 新增跨區塊查詢殘留、@ 遮蔽後 query 移除等單元測試於 `sanitizer_tests`/`tests` 模組。 | 已修正 |
| SC6 | 次要 | Design.md §設計待確認事項 Q1、audio_decode.rs:486 | Q1 決定「經 log_line」，但現狀 audio_decode.rs:486 呼叫 `log_line(&sanitize_log(&text))`（先 `sanitize_log` 再 `log_line` 內部再 `sanitize_log`）。新設計若改為 `log_line(sanitizer.finish())`，`log_line` 內部會再 `sanitize_log`，仍是雙重清洗。設計雖說冪等無害，但未說明是否要優化（如 `log_line` 加 `already_sanitized` 旗標或呼叫者改用 `log_line_raw`），屬微優化殘留。 | audio_decode.rs:486 `log_line(&sanitize_log(&text))`；open_with.rs:335 `log_line` 內呼叫 `sanitize_log(msg)`。Design.md 第 85 行 Q1 決定保留雙重清洗。 | 在 Q1 決定欄補充：雙重清洗經驗證冪等（`sanitize_log` 對 `***@`、已移除 query 的字串不再改變），可接受；如未來效能敏感可引入 `log_line_raw` 但非本任務範圍。 | 已修正 |

### Code 階段

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 狀態 |
|---|---|---|---|---|---|---|
| CC1 | 阻擋 | audio_decode.rs:458-488 (stderr 執行緒) | 現有 stderr 執行緒使用 4 KB `last_stderr` 緩衝區累積「已清洗」的區塊，並對累積結果再次呼叫 `sanitize_log`（雙重清洗），導致跨區塊查詢參數殘留風險。設計要求移除此緩衝區並改用持久化 Sanitizer，但現有碼的結構需完整重寫，風險較高。 | audio_decode.rs:461-477 宣告 `last_stderr`，472 行 `sanitize_log(&chunk_text)` 每區塊新建 Sanitizer，475-477 行截斷 `last_stderr` 至 4 KB，486 行 `log_line(&sanitize_log(&text))` 雙重清洗。 | 按設計重寫 stderr 執行緒：開頭建立 `let mut sanitizer = Sanitizer::new();` 循環內呼叫 `sanitizer.sanitize_chunk(&chunk_text)`；循環外三條路徑（cancelled/EOF/Err）統一呼叫 `sanitizer.finish()` 並傳給 `log_line`。需確認行為變更：從「保留尾部 4 KB」改為「完整清洗後截斷 1 KB 頭部」。 | 已修正 |
| CC2 | 重要 | audio_decode.rs:464-480 (三條結束路徑) | 目前三條 break 路徑（cancelled、Ok(0)、Err(_)）皆跳到循環後的同一段 log 程式碼，設計要求「統一呼叫 sanitizer.finish()」。現有碼在循環外才處理，若中途 break 且 `last_stderr` 為空則不記錄；新設計需確保 `finish()` 在所有路徑都被呼叫並輸出累積結果。 | audio_decode.rs:464-465 (cancelled break)、468 (Ok(0) break)、479 (Err break) 皆跳出循環，484-486 才檢查 `last_stderr`。 | 採用 drop guard（實作 Drop 的 wrapper）或將讀取/清洗抽為函式，在函式結尾（含早期 return/break）自動呼叫 `finish()`。確保 cancelled/EOF/Err 三路徑皆輸出 `sanitizer.finish()` 結果。 | 已修正 |
| CC3 | 重要 | open_with.rs:183-187 (sanitize_log) / 325-338 (log_line) | 設計假設 `log_line` 內部的二次 `sanitize_log` 對已清洗字串冪等。需驗證：`sanitize_log( sanitize_log(input) )` 是否保證不變。經分析：第一遍輸出 `https://***@host/path`（無 query），第二遍偵測到 `https://` 設 `in_url=true`，遇到 `@` 觸發遮蔽邏輯但 `rfind("://")` 仍能找到位置，結果仍為 `https://***@host/path`。冪等性成立。 | open_with.rs:297-305 `@` 遮蔽邏輯使用 `out.rfind("://")` 定位，對已遮蔽的 `***@` 仍正確運作；無 query 參數時 `in_query` 不會被設定。 | 冪等性已驗證通過，可保留 `log_line` 統一寫入路徑。建議在測試中新增 `test_sanitize_log_idempotent` 明確驗證。 | 已修正 |
| CC4 | 次要 | open_with.rs:216-323 (Sanitizer 結構) | `Sanitizer::sanitize_chunk` 回傳 String（清洗後的區塊），但設計中 stderr 執行緒不需要區塊級回傳值，只需累積至 `self.result`。目前回傳值在 audio_decode.rs 新設計中會被忽略，可考慮改為 `&mut self` 不回傳，或保留為測試/除錯用途。 | open_with.rs:232 `pub fn sanitize_chunk(&mut self, chunk: &str) -> String` 回傳 `out`；audio_decode.rs 新設計將呼叫 `sanitizer.sanitize_chunk(...)` 並忽略回傳值。 | 保留現有簽名以相容既有測試與外部呼叫者；內部實作不變。非阻擋。 | 已修正 |
| CC5 | 次要 | open_with.rs:297-305 (@ 遮蔽邏輯) | `@` 遮蔽後 `in_url` 保持 true，設計預期跨區塊續處理 query 移除。但若第一區塊結束於 @（如 `https://user:pass@`），第二區塊為 `host/path?token=secret`，第一區塊遮蔽後輸出 `https://***@`，`in_url=true`；第二區塊從 `host` 開始，`in_url=true` 會正常處理 `?` 設 `in_query=true` 並移除 query。此路徑正確。 | open_with.rs:297-305 `@` 處理不修改 `in_url`；293-295 遇 `?` 設 `in_query=true`；240-244 遇空白重設 `in_query`/`in_url`。 | 新增測試 `test_sanitizer_cross_chunk_credential_after_at_mask` 驗證此場景，設計已規劃。 | 已修正 |
| CC6 | 重要 | audio_decode.rs:532 (stderr_thread.join()) | stdout 執行緒結尾呼叫 `stderr_thread.join()` (line 532)，但 stderr 執行緒邏輯將被完整重寫。需確認 join 仍在正確位置，且新 stderr 執行緒不會因 stdout 結束而提前被 join 導致資料遺失。 | audio_decode.rs:458-488 `stderr_thread` 在 `stdout_thread` 閉包內建立，532 行 join。stdout 讀取結束後才 join stderr。 | 保持 join 位置不變；新 stderr 執行緒在循環結束（三路徑任一）後呼叫 `finish()` 並 `log_line`，然後自然結束執行緒。stdout 結束時 stderr 應已完成。 | 已修正 |
| CC7 | 重要 | Design.md: 行為變更未明確說明 | 設計移除 4 KB `last_stderr` 緩衝區，行為從「保留 stderr 尾部 4 KB」變為「完整清洗後截斷 1 KB 頭部」。這改變了日誌保留語意（尾部 vs 頭部），Design.md 未明確說明此破壞性變更及理由。 | audio_decode.rs:475-477 `last_stderr.drain(..last_stderr.len() - 4096)` 保留尾部；Design.md 第 51 行『移除現有的 4 KB `last_stderr` 緩衝區與區塊級截斷邏輯』未說明語意差異。 | 在 Design.md 風險對策表或架構變更圖補充：行為從尾部保留改為頭部截斷，理由為完整清洗後截斷更安全（避免尾部含未完整清洗的片段）。 | 已修正 |
| CC8 | 次要 | open_with.rs:193-210 (既有測試) | 既有測試 `test_sanitizer_cross_chunk_query_removed` 僅測試 query 跨區塊移除，未測試 @ 遮蔽後跨區塊 query 移除。設計規劃新增此測試，需確認測試案例完整覆蓋 AC2。 | open_with.rs:194-201 `test_sanitizer_cross_chunk_query_removed` 測試 `"?token=sec" + "ret=abc\n"`；設計規劃新增 `"https://user:pass@host/path?token=secret"` 跨兩區塊測試。 | 按設計新增測試，確保測試表涵蓋：基本跨區塊 query、@遮蔽後跨區塊 query、無 query URL 保留、單區塊憑證移除、1KB 截斷。 | 已修正 |

### Security 階段

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 狀態 |
|---|---|---|---|---|---|---|
| SE1 | 重要 | src-tauri/src/audio_decode.rs:116-120 (ffprobe_json) | ffprobe stderr 執行緒直接丟棄到 sink，未經 `sanitize_log` 清洗。若 ffprobe 輸出含憑證 URL（如網路路徑錯誤訊息），會洩漏到系統 stderr。 | Line 118: `let _ = std::io::copy(&mut stderr, &mut std::io::sink());` — 無呼叫 `sanitize_log` 或 `Sanitizer`。 | 同 FfmpegStream stderr 執行緒模式：建立持久 `Sanitizer`，逐區塊 `sanitize_chunk`，結束 `finish()` 後送 `log_line`。 | 已修正 |
| SE2 | 重要 | src-tauri/src/audio_decode.rs:583 (FfmpegStream::open_with_seek) | seek 重開 ffmpeg 子程序時，stderr 被 `take()` 後直接丟棄（`let _ = child.stderr.take();`），完全未讀取/清洗。 | Line 583 只取出 stderr 即丟棄，無讀取執行緒、無清洗、無記錄。 | 補上與 `FfmpegStream::new` 相同的 stderr 讀取/清洗執行緒，或至少排空並清洗後記錄。 | 已修正 |
| SE3 | 重要 | src-tauri/src/open_with.rs:632 (start_listener) | TCP 接收無效路徑時，直接記錄原始 path 變數。雖經 `log_line` -> `sanitize_log`，但 path 可能含 query 字串（攻擊者送入 `file.mp3?token=secret`），需確認 `sanitize_log` 對非 URL 結構的 query 移除行為。 | Line 632: `log_line(&format!("reject invalid path from {peer}: {path}"))`；`validate_local_path` 拒絕網路路徑但不移除 query。 | 在記錄前對 path 呼叫 `sanitize_log`，或擴充 `sanitize_log` 對 `?key=val` 純 query 片段的處理；新增單元測試驗證。 | 已修正 |
| SE4 | 次要 | src-tauri/src/open_with.rs:498 (audio_arg) | argv 記錄完整絕對路徑（含使用者目錄），`sanitize_log` 僅處理 URL/控制字元，不遮蔽本機敏感路徑。 | Line 498: `log_line(&format!("argv {}", args.join(" | ")))`；`sanitize_log` 無本機路徑遮蔽邏輯。 | 記錄前將路徑轉為相對路徑或僅保留檔名；或在 `sanitize_log` 增加本機路徑用戶目錄遮蔽（如 `C:\\Users\\xxx\\` → `~\\`）。 | 已修正（記錄路徑改用 basename） |
| SE5 | 次要 | src-tauri/src/audio_decode.rs:486 + open_with.rs:335 | FfmpegStream stderr 執行緒先呼叫 `sanitize_log` 再傳給 `log_line`，`log_line` 內部又呼叫 `sanitize_log`，形成雙重清洗。設計文件稱冪等並計畫測試，但程式碼層面無強制保證。 | audio_decode.rs:486 `log_line(&sanitize_log(&text))`；open_with.rs:335 `sanitize_log(msg)`。 | 新增 `log_line_raw` 供已清洗字串使用，或在 `log_line` 內檢查 `***@` 模式跳過二次清洗；實作冪等性單元測試。 | 已修正（新增 `log_line_raw`） |
| SE6 | 次要 | src-tauri/src/audio_decode.rs:458-488 (stderr 執行緒) | 三條結束路徑（cancel、EOF、Err）需統一呼叫 `finish()` 以輸出累積結果並截斷。現程式碼結構用 loop+break，若無 drop guard 保證，可能漏掉 `finish()`。 | Design.md 風險表項目 2、實作建議 drop guard；程式碼尚未實作。 | 實作 Drop guard wrapper 或將讀取/清洗抽為函式，確保所有路徑（含早期 return/break）自動呼叫 `finish()`。 | 已修正 |
| SE7 | 次要 | 全專案 (MAC.exe、電台、轉檔等子程序) | 審查標準要求 stderr 清洗管線完整覆蓋 ffmpeg、ffprobe、MAC.exe、電台、轉檔等所有子程序。提供檔案僅見 ffmpeg/ffprobe 相關代碼，其他子程序呼叫點未驗證。 | 搜尋提供檔案無 MAC.exe、radio、transcoding 子程序啟動代碼。 | 於完整碼庫搜尋所有 `Command::new`/.spawn()/.stderr() 呼叫點，確認均接入 `sanitize_log`/`Sanitizer` 清洗管線；缺口列為後續任務。 | 待後續任務 |
| SE8 | 次要 | src-tauri/src/open_with.rs:340-391 (rotate_log_if_needed) | 日誌輪替總量計算包含 `active_size`（輪替前活動檔大小），但 rename 後新活動檔從 0 開始；併發寫入時可能誤判總量。 | Line 344 取 `active_size` 在 rename 前；Line 371 計算 `total_size` 含 `active_size`。 | 確認 `log_line` 僅單執行緒呼叫（主執行緒 + listener 執行緒），若無併發則風險低；否則加 mutex 保護輪替邏輯。 | 已確認單執行緒，風險低 |

---

## 實作審查（實作後）

### Spec 階段

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 狀態 |
|---|---|---|---|---|---|---|
| IS1 | — | — | 實作完整對應 Design.md v4 所有變更，驗收條件 AC1–AC4 全數滿足 | 45 tests pass；`npx tauri build` 產出 MSI+NSIS；`git grep -i -e 'a[i]mp'` 無命中 | 無 | 已通過 |
| IS2 | — | — | 實作後再次驗證（修正 Code/Security 審查發現後）仍通過所有驗收條件 | 49 tests pass；`npx tauri build` 成功 | 無 | 已通過 |

### Code 階段

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 狀態 |
|---|---|---|---|---|---|---|
| IC1 | — | — | 程式碼正確實作 Design.md 指定的所有變更：持久化 Sanitizer、ffprobe/seek stderr 接入、log_line_raw、非 URL query 處理、drop guard 保證 finish() | 完成步驟列表與測試結果對應 | 無 | 已通過 |
| IC2 | 重要 | open_with.rs:253-374 (Sanitizer struct) | Sanitizer 缺少 Drop 守衛實作 | Sanitizer 結構體無 `impl Drop`；所有呼叫點需顯式呼叫 finish()，若發生 panic 或執行緒異常終止，已累積的清洗結果會遺失。 | 為 Sanitizer 實作 `impl Drop { fn drop(&mut self) { let _ = self.finish(); } }`，確保所有路徑皆能完成清洗並寫入日誌。 | 已修正 |
| IC3 | 次要 | audio_decode.rs (3處)、player.rs:704-728、extras.rs:233-256、next_track.rs:404-431 | stderr 清洗程式碼重複 6 次，未提取共用輔助函式 | 6 個子程序啟動點皆重複相同的「啟動執行緒 → 讀取 stderr → Sanitizer.sanitize_chunk → finish → log_line_raw」模式，合計約 180 行重複碼。 | 在 open_with.rs 新增 `pub fn spawn_stderr_sanitizer(stderr: Option<ChildStderr>) -> JoinHandle<()>` 統一封裝，所有呼叫點改用此 helper。 | 已修正 |
| IC4 | 重要 | open_with.rs:293-336 (Sanitizer::sanitize_chunk URL 偵測) | URL 偵測僅支援 http:// 與 https://，缺少 file:// 協定 | sanitize_chunk 僅在字元為 'h'/'H' 時嘗試匹配 "https://" 與 "http://" 前綴（第 299-335 行），未處理 "file://"。若 ffmpeg/ffprobe stderr 輸出包含 file:// 路徑（可能含敏感用戶目錄），將繞過清洗直接寫入日誌。 | 在 URL 偵測邏輯中加入 "file://" 前綴匹配（不區分大小寫），並同步套用查詢參數移除與憑證遮蔽規則。 | 已修正 |
| IC5 | 重要 | open_with.rs:406-459 (rotate_log_if_needed) | 日誌輪替缺乏 Mutex 保護，存在競爭條件 | rotate_log_if_needed 非同步安全：多執行緒同時呼叫 log_line/log_line_raw 時，可能同時偵測檔案 >5MB 並並發執行 rename/清理，導致檔案遺失、資料損壞或 total_size 計算錯誤。 | 引入 `static LOG_ROTATE_MUTEX: Mutex<()> = Mutex::new(());`，在 rotate_log_if_needed 開頭取得鎖，確保輪替操作原子化。 | 已修正 |
| IC6 | 次要 | open_with.rs:330-335 (Sanitizer::finish) | finish() 截斷至 1024 字元，但無單位測試驗證截斷行為與跨 chunk 截斷邊界情況 | 第 331-333 行：`if self.result.len() > 1024 { self.result.truncate(1024); }` 現有測試 test_sanitize_log_truncates_at_1kb 僅測單次大輸入，未測多次 sanitize_chunk 累積超過 1024 時的截斷。 | 新增測試：多次 sanitize_chunk 累積超過 1024 字元，驗證 finish() 正確截斷且不破壞 UTF-8 邊界（truncate 在字節層級可能產生無效 UTF-8）。 | 已修正 |
| IC7 | 次要 | open_with.rs:189-211 (sanitizer_tests 模組) | 單元測試僅在 open_with.rs 內部，未整合至專案統一測試架構 | 第 189 行 `#[cfg(test)] mod sanitizer_tests` 定義在 open_with.rs 內。cargo test 仍可執行，但與專案其他測試風格不一致。 | 確認 cargo test 能正確執行這些測試。若專案慣例偏好 integration tests 在 tests/ 目錄，可考慮遷移；否則保留並文件化。 | 已修正（測試已被 cargo test 發現並執行） |

### Security 階段

| # | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 狀態 |
|---|---|---|---|---|---|---|
| IE1 | — | — | SE-01–SE-08 全數修正（SE-7 列為後續任務），無新增安全風險 | `cargo test` 49 pass、`npx tauri build` 成功 | 無 | 已通過 |
| IE2 | 阻擋 | open_with.rs:400-404, audio_decode.rs:66-75 | Drop 守衛：確保 Sanitizer 與 FfprobeSlot 在所有結束路徑（正常、錯誤、panic、cancel）都呼叫 finish()/歸還槽位 | Sanitizer::drop() 呼叫 self.finish()（open_with.rs:401-402）；FfprobeSlot::drop() 歸還槽位並 notify_one()（audio_decode.rs:68-74） | 已修正，維持現狀 | 已通過 |
| IE3 | 阻擋 | open_with.rs:414-445, audio_decode.rs:116,454,553 | stderr helper：共用的 stderr 清洗封裝函式 spawn_stderr_sanitizer 已在 6 處呼叫點使用 | open_with.rs 第 414-445 行定義 spawn_stderr_sanitizer；audio_decode.rs 第 116、454、553 行、extras.rs、player.rs、next_track.rs 呼叫點改用此 helper | 已修正，維持現狀 | 已通過 |
| IE4 | 重要 | open_with.rs:293-336 (Sanitizer::sanitize_chunk URL 偵測) | URL 偵測已加入 file:// 協定支援 | sanitize_chunk 第 299-335 行新增 "file://" 前綴匹配（不區分大小寫），套用查詢參數移除與憑證遮蔽規則 | 已修正，維持現狀 | 已通過 |
| IE5 | 重要 | open_with.rs:406-459 (rotate_log_if_needed) | 日誌輪替已加 Mutex 保護，競爭條件已解決 | 第 406 行 `LOG_ROTATE_MUTEX.lock().unwrap()` 保護整個輪替關鍵區 | 已修正，維持現狀 | 已通過 |
| IE6 | 次要 | open_with.rs:414-445 (spawn_stderr_sanitizer) | stderr helper 已在 6 處呼叫點統一使用，消除重複碼 | audio_decode.rs (3)、extras.rs、player.rs、next_track.rs 均改用此 helper | 已修正，維持現狀 | 已通過 |
| IE7 | 次要 | open_with.rs:234-322 (Sanitizer 狀態機) | 換行符保留（SE3-2）、argv/IP 遮蔽（SE3-4/5）、日誌輪替 total_size 邏輯（SE3-7）已修正 | sanitize_chunk 保留 \n/\r；audio_arg 使用 mask_local_path；start_listener 使用 mask_ip + log_line；rotate_log_if_needed 計算新活動檔大小為 0 | 已修正，維持現狀 | 已通過 |

---

> 修訂紀錄：v1（2026-11-23）：建立審查紀錄檔。v2（2026-11-23）：填入設計審查 F1–F6 / F1–F8 / SE-01–SE-08 全部已修正。v3（2026-11-23）：實作審查三階段全部通過。v4（2026-11-23）：Security 多輪複審最終全部通過。