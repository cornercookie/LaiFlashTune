# P1-3 Design v16 — Design 代碼審查請求 (Code 階段)

## 背景
SonicLedger：Tauri v2＋Rust＋React 可攜 Windows 音樂播放器。本輪審查 Design 文件（Code 階段），確認做法在現有程式與套件上是否可行；正確性、測試、可維護性。審查時只讀，不得改 repo。

## 當前設計
檔案：`planning/[#7]P1-3-淡入淡出曲間停頓去除靜音/Design.md` (v16，2026-11-25)

## v16 修正摘要（針對 v15 審查 #1–#6）
| 原阻擋/重要 | 修正位置 | 核心修正 |
|-------------|----------|----------|
| #1 更換下一首未使舊開流失效 | D:83、181、247 | SetNext/SetLoad/Seek/ClearNext 遞增 pending_gen 並清空 pending_stream_source/meta，重設 install_gen = 0，舊串流不再被安裝 |
| #2 音訊回呼同步觸發 FFmpeg 回收 | D:181、247 | set_stream_source() 不在音訊回呼內調用，舊來源移交回收執行緒析構 |
| #3 串流開啟只指定 SymStream，無法涵蓋 FFmpeg | D:179、247 | 依後端分流：Symphonia 用 SymStream::open()，FFmpeg 用 open_with_seek(position_ms)，補齊 FFmpeg 偏移開流與取消接線 |
| #4 串流交接只重設 TrackResampler，失去音效 | D:181、247 | 串流安裝時重建 RateStage，沿用現有載入路徑，保留當前音效設定 |
| #5 同一串流同時留在協調器槽與待安裝來源 | D:179–185、249 | 開流取得所有權時消費對應協調器 slot，區分「等待來源」與「已安裝、等待通知處理」 |
| #6 程式基準中的建置腳本路徑錯誤 | D:11 | 修正為 src-tauri/build.rs |

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
