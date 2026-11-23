# [#1]P0-SE3-跨區塊查詢清洗狀態

> 本期的設計與實作步驟都在這份；審查紀錄在同資料夾的 `Review.md`。流程見根目錄 `AGENTS.md`。

## 文件資訊表

| 項目 | 內容 |
|---|---|
| 版本 | v1 |
| 對應 Planning | `planning/[#1]P0-SE3-跨區塊查詢清洗狀態/planning.md` |
| 設計依據 | Planning Description / Expected behavior / Acceptance Criteria |
| 程式基準 | `src-tauri/src/open_with.rs:183` (`sanitize_log`)、`src-tauri/src/open_with.rs:216` (`Sanitizer`)、`src-tauri/src/audio_decode.rs:458-488` (FfmpegStream stderr 執行緒) |
| 建立日期 | 2026-11-23 |
| 負責 | Hermes Agent |

## 需求對照表（R1…Rn）

| 編號 | Planning 原文 | 現況 (path:line) | 設計決策 |
|---|---|---|---|
| R1 | `sanitize_log`（或取代它的有狀態清洗器）必須移除 URL 查詢參數（`?` 之後直到空白為止） | `Sanitizer::sanitize_chunk` 已在遇 `?` 時設 `in_query=true`，後續至空白前捨棄 (`open_with.rs:293-296`)；但 `sanitize_log` 每次新建 `Sanitizer`，狀態不跨呼叫保存 | 保留 `Sanitizer` 的 `in_query` 跨 `sanitize_chunk` 呼叫；FfmpegStream stderr 執行緒改用單一 `Sanitizer` 實例處理所有區塊，最後呼叫 `finish()` |
| R2 | 跨讀取區塊保留 `in_url` / `in_query` 狀態，避免憑證跨區塊殘留 | `sanitize_log` 每次建立新 `Sanitizer`，狀態不跨呼叫保存 (`open_with.rs:183-187`)；FfmpegStream stderr 逐區塊呼叫 `sanitize_log`，跨區塊憑證可能殘留 (`audio_decode.rs:472`) | FfmpegStream stderr 執行緒建立一個 `Sanitizer` 實例，所有區塊用 `sanitize_chunk`，結束時 `finish()` 截斷 |
| R3 | 清洗完成後再截斷 ≤ 1 KB，避免截斷切掉前綴導致憑證殘留 | `Sanitizer::finish()` 已在 1 KB 後截斷 (`open_with.rs:317-322`)；`sanitize_log` 先 `sanitize_chunk` 再 `finish`，單一輸入已符合 | 保持 `finish()` 截斷；串流場景只在所有區塊處理完後呼叫 `finish()`，不中途截斷 |
| R4 | `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過 | 現有測試已通過 | 既有測試 `test_sanitizer_cross_chunk_query_removed` 已覆蓋 AC2；新增跨區塊 @ 遮蔽後 query 移除測試，確認所有測試綠燈 |

## 架構變更圖

```mermaid
flowchart TD
  FE[ffmpeg/ffprobe 子程序 stderr] --> ST[stderr 讀取執行緒<br/>audio_decode.rs:458]
  ST --> CH[逐區塊讀取 (4 KB buffer)]
  CH --> S1{Sanitizer 實例<br/>是否持久化？}
  S1 -- 否（現狀） --> SAN[每區塊新建 sanitize_log<br/>in_url/in_query 遺失，跨區塊 query 殘留]
  S1 -- 是（設計） --> SAN2[單一 Sanitizer 實例<br/>sanitize_chunk 跨區塊保留 in_url/in_query]
  SAN2 --> FIN[finish() 截斷 ≤ 1 KB]
  FIN --> LOG[log_line 寫入 data/open-with.log]
  LOG --> OUT[前端顯示 / 日誌檔案，憑證不外洩]
```

## 詳細設計

### 現狀問題

1. `sanitize_log(input: &str) -> String` (`open_with.rs:183-187`) 每次呼叫都 `Sanitizer::new()`，是無狀態的。單一完整訊息清洗正確，但串流逐區塊呼叫時，`in_url`/`in_query` 不會跨區塊保留。
2. `FfmpegStream` 的 stderr 讀取執行緒 (`audio_decode.rs:458-488`) 逐區塊呼叫 `sanitize_log(&chunk_text)` (`audio_decode.rs:472`)。第一區塊若以 `?token=` 結束，第二區塊的 `secret\n` 會以新 `Sanitizer` 處理，`in_query` 為 false，查詢參數殘留於日誌。
3. `log_line` (`open_with.rs:335`) 對單一訊息呼叫 `sanitize_log`，屬單次輸入，無跨區塊需求，保持現狀即可。

### 做法

**變更 1：`audio_decode.rs` FfmpegStream stderr 執行緒改用持久化 `Sanitizer`**

- **移除現有的 4 KB `last_stderr` 緩衝區與區塊級截斷邏輯**（`audio_decode.rs:461-477`：`last_stderr` 宣告、區塊清洗後累積並截斷 4 KB 保留尾部、雙重 `sanitize_log`）。**行為變更：從「保留 stderr 尾部 4 KB」改為「完整清洗後截斷 1 KB 頭部」，理由為完整清洗後截斷更安全（避免尾部含未完整清洗的片段）。**
- 在 stderr 讀取執行緒開頭建立 `let mut sanitizer = Sanitizer::new();`
- 每個區塊呼叫 `sanitizer.sanitize_chunk(&chunk_text)`，取代 `sanitize_log(&chunk_text)`；`sanitize_chunk` 會將清洗後結果累積至 `self.result` 並保留 `in_url`/`in_query` 狀態
- **確保三條結束路徑（`cancel_stderr.is_cancelled()`、`Ok(0)` EOF、`Err(_)`）均呼叫 `sanitizer.finish()`**：建議以 `drop guard`（實作 `Drop` 的 wrapper）或將讀取/清洗抽為函式，在函式結尾（含早期 return/break）自動呼叫 `finish()`，保證所有路徑輸出累積結果。
- 若結果非空，直接傳給 `log_line` 寫入日誌（`log_line` 內部會再次呼叫 `sanitize_log`，對已清洗字串為冪等，不會引入新憑證；保留統一寫日誌路徑與輪替邏輯）

**變更 1b：`audio_decode.rs` ffprobe stderr（`ffprobe_json`）與 seek 重開（`open_with_seek`）同步接入清洗管線**

- `ffprobe_json`（`audio_decode.rs:116-120`）目前將 stderr 直接丟棄到 `std::io::sink()`，**改為建立持久 `Sanitizer` 逐區塊 `sanitize_chunk`，結束 `finish()` 後送 `log_line`**。
- `open_with_seek`（`audio_decode.rs:583`）seek 重開 ffmpeg 時 `child.stderr.take()` 後直接丟棄，**補上與 `FfmpegStream::new` 相同的 stderr 讀取/清洗執行緒**。

**變更 2：保留 `sanitize_log` 作為單次訊息便捷函式**

- `sanitize_log` 保持不變，供 `log_line`、`extras.rs`、`player.rs` 等單次訊息呼叫者使用
- 不擴散為跨呼叫狀態機，避免呼叫者誤用

**變更 3：單元測試**

- 新增 `test_sanitizer_cross_chunk_query_removed_via_stderr`：模擬兩區塊（第一個以 `?token=` 結束，第二個為 `secret\n`），驗證 `finish()` 結果不含 `token`
- 新增 `test_sanitizer_cross_chunk_credential_after_at_mask`：`https://user:pass@host/path?token=secret` 跨兩區塊，驗證 `@` 遮蔽後 `***@` 與 query 皆被正確處理
- 新增 `test_sanitize_log_idempotent`：驗證 `sanitize_log(sanitize_log(input)) == sanitize_log(input)`，確認 `log_line` 二次清洗冪等
- 新增 `test_sanitize_log_non_url_query`：驗證非 URL 的 `?key=val` 片段（如 `file.mp3?token=secret`）被移除，確認 `open_with.rs` socket 記錄路徑安全
- 現有 `test_sanitizer_cross_chunk_query_removed`、`test_sanitizer_cross_chunk_url_preserved` 已涵蓋基本跨區塊，保留

**變更 4：`log_line` 引入 `log_line_raw` 避免雙重清洗**

- 新增 `log_line_raw(msg: &str)` 直接寫入日誌（不經 `sanitize_log`），供 `audio_decode.rs` stderr 執行緒、TCP listener 等已清洗字串使用
- `log_line` 保持呼叫 `sanitize_log`，供未清洗入口（`argv`、工具定位、命令日誌等）使用
- 更新 `audio_decode.rs`、`open_with.rs` listener 等呼叫點改用 `log_line_raw`

### 錯誤處理

- `Sanitizer::sanitize_chunk` 不回傳 `Result`，內部忽略非 ASCII 圖形字元與控制字元，不失敗
- `finish()` 不失敗，僅截斷
- 區塊讀取結束的三條路徑均在循環外統一呼叫 `sanitizer.finish()` 輸出目前累積結果：
  1. `cancel_stderr.is_cancelled()` → `break`
  2. 讀取 `Ok(0)` (EOF) → `break`
  3. 讀取 `Err(_)` → `break`
- **實作建議：用 `drop guard`（實作 `Drop` 的 wrapper）保證所有路徑（含早期 return/break/panic）自動呼叫 `finish()`，避免漏掉輸出**
- `log_line_raw` 不失敗，僅寫入與輪替

## IPC 指令契約變更

無。本任務僅變更 Rust 內部 stderr 清洗邏輯，不涉及 Tauri command、DTO、前後端介面。

## 前端狀態與 UI 變更

無。本任務僅變更 Rust 內部 stderr 清洗邏輯，不涉及前端狀態或 UI。

## 設計待確認事項

| 編號 | 事項 | 決定 | 理由 |
|---|---|---|---|
| Q1 | stderr 執行緒 `finish()` 後是否仍經 `log_line` 再做一次 `sanitize_log`？ | 已確認：經 `log_line` | `log_line` 的 `sanitize_log` 對已清洗字串冪等（`***@`、已移除 query 的字串不再改變），無副作用；保留統一寫日誌路徑與輪替邏輯。如未來效能敏感可引入 `log_line_raw` 但非本任務範圍。 |
| Q2 | `sanitize_log` 是否應標記 `#[deprecated]` 引導呼叫者改用 `Sanitizer`？ | 未確認：保留 | `log_line` 等單次訊息呼叫者仍需 `sanitize_log`；標記棄用會造成大量改動，非本任務範圍 |

## 驗收條件對照

| AC 編號 | Planning 條件 | 設計對應 | 測試方式 |
|---|---|---|---|
| AC1 | URL 查詢參數在單一區塊內被移除 | `Sanitizer::sanitize_chunk` 遇 `?` 設 `in_query=true`，後續至空白前皆捨棄 | 單元測試 `test_sanitize_log_removes_credentials` 現有（透過 `sanitize_log` 間接覆蓋） |
| AC2 | URL 查詢參數在跨區塊時也被移除（第一區塊結束於 `?token=`，第二區塊是 `secret\n`） | FfmpegStream stderr 用單一 `Sanitizer` 跨區塊呼叫 `sanitize_chunk`，`in_query` 保留至第二區塊 | 既有測試 `test_sanitizer_cross_chunk_query_removed` 驗證此情境 |
| AC3 | 一般 URL 不含 query 時正常保留 | `in_query` 未設定時，URL 字元正常 push 至輸出 | 單元測試 `test_sanitizer_cross_chunk_url_preserved` 現有 |
| AC4 | `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過 | 新增測試不破壞現有測試 | 執行 `cargo test --release --manifest-path src-tauri/Cargo.toml` |

## 風險對策表

| 風險 | 可能性 | 影響 | 對策 |
|---|---|---|---|
| `finish()` 截斷切掉 `https://` 前綴，導致後續非 URL 文字被誤認為繼續 query | 低 | 中 | `finish()` 僅截斷，不重設 `in_url`/`in_query`；但每次 `sanitize_log` 新建實例，單次輸入不受影響；串流場景在 `finish()` 後結束，不再繼續 |
| stderr 執行緒提前 `break`（cancelled / 錯誤）未呼叫 `finish()`，已累積但未截斷的結果丟失 | 中 | 低 | 在所有 `break` 路徑後統一呼叫 `finish()` 輸出累積結果；建議用 `drop guard` 保證 |
| `log_line` 二次 `sanitize_log` 對已清洗字串的 `***@` 產生副作用 | 低 | 低 | `sanitize_log` 對 `***@` 格式不做額外遮蔽，僅再次遍歷，無副作用；新增冪等性測試驗證 |
| 行為變更：stderr 日誌從「保留尾部 4 KB」改為「完整清洗後截斷 1 KB 頭部」 | 低 | 中 | 完整清洗後截斷更安全（避免尾部含未完整清洗片段）；Design.md 已明確說明變更與理由 |

## 實作順序建議

```mermaid
flowchart TD
  A[修改 audio_decode.rs stderr 執行緒：建立持久 Sanitizer] --> B[所有區塊用 sanitize_chunk，結束呼叫 finish]
  B --> C[新增跨區塊查詢殘留單元測試]
  C --> D[執行 cargo test --release --manifest-path src-tauri/Cargo.toml]
  D --> E[執行 npx tauri build 確認建置正常]
  E --> F[Design 審查 Spec → Code → Security]
```

## 測試案例表

| 測試案例 | 對應 AC | 類型 | 驗證重點 |
|---|---|---|---|
| `test_sanitizer_cross_chunk_query_removed`（現有） | AC2 | 單元 | 第一區塊 `?token=`，第二區塊 `secret\n`，query 跨區塊被移除 |
| `test_sanitizer_cross_chunk_url_preserved`（現有） | AC3 | 單元 | 無 query URL 跨區塊正常保留 |
| `test_sanitize_log_removes_credentials`（現有） | AC1 | 單元 | 單一區塊 `user:pass@host?token=abc`，憑證遮蔽 + query 移除 |
| `test_sanitize_log_truncates_at_1kb`（現有） | R3 | 單元 | 超過 1 KB 被截斷 |
| 新增：跨區塊 @ 遮蔽後 query 移除 | AC2 | 單元 | `https://user:pass@host/path?token=secret` 跨兩區塊，`***@` 後 query 仍被移除 |
| 新增：sanitize_log 冪等性 | AC1/AC2 | 單元 | `sanitize_log(sanitize_log(input)) == sanitize_log(input)`，確認二次清洗無副作用 |
| 新增：非 URL query 片段移除 | AC2 | 單元 | `file.mp3?token=secret` 被移除 query，確認 socket 記錄路徑安全 |

## 變更檔案清單

| 檔案 | 變更類型 | 說明 |
|---|---|---|
| `src-tauri/src/audio_decode.rs` | 修改 | FfmpegStream stderr 執行緒改用持久化 `Sanitizer` 實例，跨區塊呼叫 `sanitize_chunk`，結束 `finish()`；移除 4 KB `last_stderr` 緩衝區與雙重清洗；ffprobe stderr（`ffprobe_json`）與 seek 重開（`open_with_seek`）同步接入清洗管線；stderr 執行緒改用 `log_line_raw` |
| `src-tauri/src/open_with.rs` | 修改 | 新增 `log_line_raw` 函式；新增跨區塊 @ 遮蔽後 query 移除、冪等性、非 URL query 片段移除等單元測試於 `sanitizer_tests`/`tests` 模組；TCP listener 記錄路徑改用 `sanitize_log` 清洗後再 `log_line_raw` |

## 回寫 Agents/ 文件規劃

本任務僅變更內部 stderr 清洗邏輯，不涉及服務架構、系統架構、資料庫、業務邏輯或使用者權限。無需回寫 `Agents/` 任何檔案。

若日後 `sanitize_log` 呼叫點擴散至新的子程序 stderr 場景，請在 `Agents/system-architecture.md` 的子程序管理章節補記「stderr 清洗須用持久 Sanitizer」。

## 審查記錄區

### 設計審查（實作前）

| 階段 | 結論 | 發現 | 修正狀態 |
|---|---|---|---|
| Spec | 有條件通過（已修正） | F1–F6 | 已修正 |
| Code | 有條件通過（已修正） | F1–F8 | 已修正 |
| Security | 有條件通過（已修正） | SE-01–SE-08 | 已修正 |

### 實作審查（實作後）

| 階段 | 結論 | 發現 | 修正狀態 |
|---|---|---|---|
| Spec | 待審 | — | — |
| Code | 待審 | — | — |
| Security | 待審 | — | — |

---

> 修訂紀錄：v1（2026-11-23）：初版，根據 planning.md Expected behavior 與 Acceptance Criteria 撰寫。v2（2026-11-23）：納入 Spec 審查 F1–F6 修正（移除 4 KB 緩衝區、三條 break 路徑明確化、測試表既有/新增釐清、檔案清單合併、Q1 增補冪等驗證）。v3（2026-11-23）：納入 Code 審查 F1–F8 修正（行為變更說明、drop guard 保證 finish()、冪等性測試、行為變更風險補充）。v4（2026-11-23）：納入 Security 審查 SE-01–SE-08 修正（ffprobe/seek stderr 接入清洗管線、log_line_raw 避免雙重清洗、非 URL query 測試、socket 路徑清洗、drop guard 強制 finish()、其他子程序 stderr 列為後續任務、日誌輪替併發確認）。