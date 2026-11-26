# P1-3 Design v20 — Design 代碼審查請求 (Code 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Code 階段），確認做法在現有程式與套件上是否可行；正確性、測試、可維護性。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v20，2026-11-25)

## v20 修正摘要（針對 v19 審查 #4，已完整落入正文）
| 原重要 | 修正位置 | 核心修正 |
|--------|----------|----------|
| #4 清槽與過期來源回收契約未完整落入正文 | D:179、182、184、185、186、310 | §5 串流交接補充「完整來源回收契約」：清槽、安裝前失效、過期交付、換軌舊來源移除皆在鎖內取出完整 `pending_stream_source` / `pending_stream_meta`，釋鎖後交回收執行緒負責取消與程序回收；不得只移交取消控制，不得在鎖內析構來源；回收執行緒使用 `try_wait()` / `is_finished()` + deadline，逾時 `kill()` + `drop()`，不阻塞 `wait()` / `join()`。§5 各處「清空」語句同改為依回收契約執行。 |

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
