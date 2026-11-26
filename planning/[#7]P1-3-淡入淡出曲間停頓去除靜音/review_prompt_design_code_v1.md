# P1-3 Design v15 — Design 代碼審查請求 (Code 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Code 階段），確認做法在現有程式與套件上是否可行；正確性、測試、可維護性。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v15，2026-11-25)

## v15 修正摘要
- 記憶體軌安裝時設 `install_gen = pending_gen`（遞增後），建立 `Some((StreamInstalled, install_gen))`；命令執行緒以通知世代核對後恢復 `Playing`
- 實作步驟第 3 點同步正文：串流進入 WaitingForNext 時一次性啟動 gap；耗盡後直接向 `NextCoordinator.take_next()` 取 Memory、建立 Memory TrackChain、重設重取樣器及通知世代
- R1–R6 涵蓋功能驗收，測試與建置要求另列於完成條件
- gap 的固定 48 kHz 雙聲道換算正確，2000 ms 為 192,000 個交錯樣本
- 九個設定 key、互斥持久化、三處指令註冊均有安排
- Design 前後 SHA-256 一致，`git grep -n -i -e 'a[i]mp'` 與未追蹤 Design 補查均無命中

## 程式基準（與 Design.md 一致）
`src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/models.rs`、`src-tauri/src/eq.rs`、`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/main.rs`、`src-tauri/src/build.rs`、`src-tauri/capabilities/default.json`、`src-tauri/src/db.rs`、`src/types.ts`、`src/lib/api.ts`、`src/player/Shell.tsx`、`src/player/nextTrack.ts`

## 審查重點
1. 做法在現有程式與套件上是否可行（threading/cancellation races、音訊回呼鎖、子程序清理、記憶體上限）
2. 正確性、測試、可維護性
3. 新增三 Tauri 指令（camelCase 引數）三處註冊（main.rs handler、build.rs COMMANDS、capabilities/default.json 權限）
4. `cargo test --release --manifest-path src-tauri/Cargo.toml` 與 `node --test "src/player/*.test.mjs"` 是否可全過
5. `npx tauri build` 是否可成功
6. repo 公開規則：文件內不得出現參考播放器名稱（`git grep -n -i -e 'a[i]mp'` 結果需為空）

## 輸出格式要求
`# | 嚴重度 | 位置 | 問題 | 證據 | 建議 | 結論` markdown 表，嚴重度為 阻擋／重要／次要。
最後一行：`結論：通過 / 有條件通過 / 不通過`
