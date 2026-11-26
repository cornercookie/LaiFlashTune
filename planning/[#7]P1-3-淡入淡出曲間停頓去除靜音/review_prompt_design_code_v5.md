# P1-3 Design v19 — Design 代碼審查請求 (Code 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Code 階段），確認做法在現有程式與套件上是否可行；正確性、測試、可維護性。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v19，2026-11-25)

## v19 修正摘要（針對 v18 審查 #1–#4，已落入正文）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 串流安裝循環依賴 | D:181、185、249、310 | `next()` 設 `stream_install_ready`，命令執行緒 tick 直接消費 ready、核對 `meta.gen == stream_gen`、呼叫 `install_stream_source()` 完成安裝、再設 `done_target = StreamInstalled` 僅用於 `sink.play()`，移除循環依賴 |
| #2 來源世代歸零與 fade_gen 混用 | D:106、139、140、143、182、187、247 | 移除 `stream_gen = 0` 歸零與 `stream_gen = fade_gen` 混用，`stream_gen` 持續遞增且與 `fade_gen` 分開，Memory 安裝不覆寫來源世代 |
| #3 串流初始化未完全落入正文 | D:181、247、310 | `install_stream_source()` 合約補入「清除舊 TrackChain、重建 RateStage、依新軌格式重設 TrackResampler、保留目前音效參數」 |
| #4 獨立取消控制所有權未接清 | D:179、187、227、249、310 | 新開流來源持有獨立取消控制；過期結果、清槽及換軌將完整來源移出共享鎖後交回收執行緒 |

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
