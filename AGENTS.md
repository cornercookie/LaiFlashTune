# AGENTS.md

SonicLedger：Tauri v2＋Rust＋React 的可攜式 Windows 音樂播放器。這份只放每個任務都適用的規則；專案知識在 `Agents/`，用到哪一塊就讀哪一份。

## 專案知識（`Agents/`）

- 新增或修改 command、事件、執行緒、外部程序 → [`Agents/system-architecture.md`](Agents/system-architecture.md)
- 判斷功能該放前端還是後端、找某個功能在哪個檔案 → [`Agents/service-architecture.md`](Agents/service-architecture.md)
- 動到資料表、欄位、索引、`app_config` 的 key → [`Agents/database.md`](Agents/database.md)
- 改播放、歌單、曲庫、搜尋、音效、工具的行為規則 → [`Agents/business-logic.md`](Agents/business-logic.md)
- 碰到檔案系統、登錄檔、子程序、網路、Tauri capability、系統管理員權限 → [`Agents/permissions.md`](Agents/permissions.md)

其他文件：產品原則 `PRODUCT.md`；視覺設計規範 `DESIGN-SYSTEM.md`（根目錄這份是畫面風格，不是任務的 Design.md）；分期路線圖 `planning/ROADMAP.md`；舊的段落式計畫 `PLAN.md`（已完成，只供查歷史）。

## 開發流程

每個任務照這個順序走，前一步沒完成就不進下一步。

1. **寫 Design.md**：放在 `planning/<任務>/Design.md`，一個任務一份。設計與實作步驟寫在同一份，不另外開 Implementation.md。內容：
   - 目標與範圍
   - 圖：依任務挑流程圖、時序圖或架構圖（Mermaid），用得上的才畫
   - 現況（附檔案與行號）
   - 做法與實作步驟（要改哪些檔、依什麼順序）
   - 資料變更
   - 完成條件（可量測、可重現）
   - 風險
2. **Design 審查**：依序做 Spec → Code → Security 三階段，每階段的阻擋問題修完才進下一階段。
3. **實作**：由 OpenCode（`opencode run`）照 Design.md 的步驟做；做法有變更就先改 Design.md，再改程式。主導的 agent 負責拆工作、核對結果、跑測試與建置。
4. **實作審查**：同樣依序做 Spec → Code → Security。
5. **收尾**：更新受影響的 `Agents/` 文件與 `planning/ROADMAP.md` 的狀態，重建 release exe。

### 審查

- 審查一律由 Codex 做：`codex exec --sandbox read-only -c model_reasoning_effort=high`。作者（寫 Design 或實作的 agent）不能自己審；OpenCode 負責實作，所以也不擔任審查者。審查時只讀，不能改 repo。
- 三個階段要審的東西：
  - **Spec**：需求、產品原則、完成條件是否對得上，有沒有缺漏。
  - **Code**：Design 階段審做法在現有程式與套件上是否可行；實作階段審正確性、測試、可維護性。
  - **Security**：檔案路徑、子程序參數、登錄檔、本機連接埠、網路、Tauri capability、權限提升。
- 審查結果記在同一個資料夾的 `Review.md`，依「Design／實作 × Spec／Code／Security」分節。每一條都標成已修、不修（附理由）或待確認。
- 審查者的說法要先拿原始資料（程式碼、官方文件、實際執行結果）核對過才採用；核對不了的標成待確認。

## 通用規則

- 介面與文件用繁體中文。
- 控制項要能驅動引擎才可以畫；做不到的功能明確寫做不到。
- 資料只寫在執行檔旁的 `data/`。連網功能預設關閉。
- **repo 是公開的**：文件、註解、畫面文字一律用「參考播放器」稱呼比對對象，功能用通用名稱。提交前執行 `git grep -n -i -e 'a[i]mp'`，結果要是空的。已推上去的歷史紀錄不改寫。
- Tauri command 的參數用 camelCase；巢狀結構照 Rust 欄位名稱，或在 Rust 端加 `rename_all`。
- **完成的定義**：
  - `cargo test --release --manifest-path src-tauri/Cargo.toml` 與 `node --test "src/player/*.test.mjs"`（glob 要加引號，交給 node 展開）全過。
  - 關掉正在跑的 `src-tauri/target/release/sonicledger.exe`，執行 `npx tauri build`，確認新的 `dist/assets/index-*.js` 檔名在 exe 裡。安裝程式步驟出現缺 `.ico` 的錯誤不算 exe 建置失敗。
  - 不留著正在跑的 `npm run dev`、vite 或測試用程序。
