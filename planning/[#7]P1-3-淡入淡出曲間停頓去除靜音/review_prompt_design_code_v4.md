# P1-3 Design v18 — Design 代碼審查請求 (Code 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Code 階段），確認做法在現有程式與套件上是否可行；正確性、測試、可維護性。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v18，2026-11-25)

## v18 修正摘要（針對 v17 審查 #1–#4）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 串流安裝通知形成循環依賴，可能持續停在 WaitingForNext | D:107、181、185、249 | 命令迴圈 tick 直接消費 `stream_install_ready`、核對 `meta.gen == stream_gen`、執行 `install_stream_source()` 並完成狀態轉移，移除「先收到安裝完成通知才能安裝」的循環依賴 |
| #2 來源世代拆分未完成，仍無一致的過期結果判定基準 | D:96、106、143、179、182、187、247 | 統一來源請求世代：開流交付時設 `stream_gen`，安裝前與 `meta.gen` 比對，安裝後不歸零；`SetNext`/`ClearNext` 遞增 `stream_gen` 並清空 `pending_stream_source/meta`，僅撤銷下一首來源，不影響淡段動作世代 |
| #3 完整串流初始化要求未落入正文與實作步驟 | D:181、247、310 | `install_stream_source()` 說明與實作步驟補充重建 RateStage、依新軌格式重設 TrackResampler、清除舊 TrackChain，保留目前音效參數 |
| #4 獨立取消控制的所有權仍未接清 | D:179、187、227、249 | 新開流來源持有獨立取消控制；過期結果、清槽及換軌皆將完整來源移出共享鎖後交回收執行緒，負責取消與程序回收 |

## 程式基準（與 Design.md 一致）
`src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`src-tauri/src/models.rs`、`src-tauri/src/eq.rs`、`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/main.rs`、`src-tauri/build.rs`、`src-tauri/capabilities/default.json`、`src-tauri/src/db.rs`、`src-tauri/src/scanner.rs`、`src-tauri/src/audio_decode.rs`、`src/types.ts`、`src/lib/api.ts`、`src/player/Shell.tsx`、`src/player/nextTrack.ts`

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
