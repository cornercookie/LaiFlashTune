<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#11]P1-7-交叉淡化補齊

## Description
現況 maybe_crossfade 只在兩首都是記憶體軌時才混音；mix_crossfade 直接依樣本索引混合，沒有處理取樣率或聲道不同的情況。P0 之後，15 分鐘以內的檔案是記憶體軌；15 分鐘以上或長度不明的是串流軌，串流軌不會交叉淡化，取樣率或聲道不同的兩首記憶體軌也會混錯。需要把交叉淡化移到 P0-3 的 TrackChain 裡做。

## User Story
作為使用者，不同格式、取樣率、聲道的兩首歌曲切換時，淡化 3 秒，輸出長度正確、中間沒有零值空白，淡化後的頻率是下一首的頻率。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 交叉淡化改在 TrackChain 裡做：下一首開始時，前後兩軌在淡化時間內各自乘上增益曲線後相加；
- 記憶體軌與串流軌都適用；
- 取樣率或聲道不同時，下一首先轉成目前這一軌的格式（線性插值重取樣，單聲道複製成兩聲道、多聲道降混成兩聲道），淡化結束後再切回原生格式並重新開始 DSP 狀態；
- 切換點選在淡化結束的那一刻，不是在兩首重疊的時候；
- 淡化期間 now_seq 在下一首第一個樣本進入混音時更新（跟 P0-3 一致）；
- 跟曲間停頓互斥；任一首是電台時不淡化。

## Acceptance Criteria
- [ ] 單元測試：記憶體軌接串流軌、串流軌接串流軌、44.1k 接 48k、單聲道接立體聲，四種情況淡化 3 秒，輸出長度正確、中間沒有零值空白，淡化後的頻率是下一首的頻率；
- [ ] 實際測試：APE 接 FLAC、長檔接短檔，各聽一次淡化過程；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/player.rs`
- 設計來源：planning/P1-playback-quality/Design.md 第 7 項
- 依賴 P0-2、P0-3
