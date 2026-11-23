# P2 Review

審查者：Codex（OpenAI，`codex exec --sandbox read-only`，簡稱 C）。預審另有 OpenCode + Nvidia Nemotron 3 Ultra（簡稱 O）；之後 OpenCode 只負責實作。作者不審自己的文件。

狀態：**已修**｜**成立**（待改 Design.md）｜**不成立**（附核對依據）｜**已涵蓋**（Design 已有）｜**待核對**（還沒拿原始資料驗證）｜**待決定**（需要使用者選）

## Design 審查

### 預審（2026-11-22，對象是改名前的 planning.md，Spec 與 Code 混在一起）

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C#26、O#14、O#35 | 重要 | 自動評分沒有公式，也沒有「實際聽了多少」的紀錄；手動切歌、拖進度條都不會經過 `on_empty` | `play_history` 只有 song_id 與時間 | 成立 |
| 2 | C#27 | 重要 | 智慧歌單的 AND/OR、介於、排序、上限需要擴充規則模型與查詢，不是只補畫面 | `models.rs` 規則只有 field/operator/value；`db.rs` 全部用 AND | 成立 |
| 3 | C#28 | 重要 | 同一首歌可以在歌單裡出現多次，但前端用 `song.id → item.id` 的單值映射，分組、多選、去重都會出錯 | `Shell.tsx` `itemIdsRef: Record<number, number>` | 成立 |
| 4 | C#29、O#18 | 重要 | Tauri ACL 的描述錯誤：自訂 command 預設全部允許，只有用 `AppManifest::commands` 宣告後才受 capability 控制；plugin 要 `.plugin(...)` 註冊 | Tauri 官方文件 capabilities 頁 | 成立（`Agents/permissions.md` 已改；Design P2-6 待改） |
| 5 | C#30 | 重要 | 期內順序衝突：快捷鍵排在書籤與評分前面，但它的動作依賴這兩項 | — | 成立 |
| 6 | C#32 | 重要 | 還有功能差距沒排入：搜尋所有已開歌單、使用者自訂分組範本 | 對照官方功能表 | 成立 |
| 7 | C#34 | 重要 | 宣稱 10 萬首卻只驗收 1 萬首 | — | 成立 |
| 8 | O#7 | 重要 | `estimateSize` 只拿得到 index，不能混排兩種高度 | Design P2-1 已攤平成陣列，`estimateSize(index)` 用 index 查列的種類即可 | 不成立 |
| 9 | O#23 | 次要 | 以資料夾為來源需要 `path` 欄位，但白名單沒有 | `db.rs` 白名單確實沒有 `path` | 成立 |
| 10 | O#28 | 次要 | 書籤表需要遷移 | Design 已寫「新表」 | 已涵蓋 |
| 11 | O#31 | 次要 | `playlist_view:<id>` 放在 key-value 表不好維護 | 設計取捨 | 待決定 |
| 12 | O#39 | 次要 | 「顯示或隱藏視窗」沒有對應的 command | — | 待核對 |

### Spec

待進行：預審中「成立」的項目改進 Design.md 之後才開始。

### Code

待進行。

### Security

待進行。

## 實作審查

### Spec

待進行。

### Code

待進行。

### Security

待進行。
