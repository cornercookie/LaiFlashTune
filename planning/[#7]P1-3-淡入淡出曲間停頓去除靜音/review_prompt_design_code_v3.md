# P1-3 Design v17 — Design 代碼審查請求 (Code 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Code 階段），確認做法在現有程式與套件上是否可行；正確性、測試、可維護性。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v17，2026-11-25)

## v17 修正摘要（針對 v15 審查 #1–#6）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 串流安裝在音訊回呼執行，可能阻塞 FFmpeg 回收 | D:132、179–180、246、282 | `set_stream_source()` 不在音訊回呼內調用，舊來源移交回收執行緒析構，`next()` 只標記 `stream_install_ready`，由命令執行緒 tick 處理 `StreamInstalled` → `sink.play()` |
| #2 SetNext/ClearNext 遞增共用世代，誤取消淡段完成通知 | D:102、134–142、186 | `FadeGate` 新增 `fade_gen` 與 `stream_gen` 分離，`SetNext`/`ClearNext` 僅遞增 `stream_gen` 並清空 `pending_stream_source/meta`，不影響淡段動作世代 |
| #3 FFmpeg 補實作定位錯誤，取消所有權尚未接清 | D:178、224、246–248 | 明確指定修改 `playback_pipeline.rs:1665-1667` 的 `FfmpegStreamHandle::open_with_seek()`，新開流建立獨立取消控制，交代來源持有、過期結果丟棄及回收的所有權 |
| #4 串流交接只重設 TrackResampler，下一軌會失去音效 | D:180、246 | 串流安裝時重建 RateStage，沿用現有載入路徑，保留當前音效設定；實作步驟第 3 點同步列入 |
| #5 同一串流同時留在協調器槽與待安裝來源 | D:179–185、249 | 開流取得所有權時消費對應協調器 slot，區分「等待來源」與「已安裝、等待通知處理」，避免重複取出 |
| #6 程式基準中的建置腳本路徑錯誤 | D:11 | 修正為 `src-tauri/build.rs` |

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
