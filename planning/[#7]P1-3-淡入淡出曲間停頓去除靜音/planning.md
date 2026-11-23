<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#7]P1-3-淡入淡出曲間停頓去除靜音

## Description
現況暫停、停止、拖進度條都是直接切斷（sink.pause()、sink.stop()）。需要加入淡入淡出包絡、曲間停頓、去除靜音，並讓睡眠計時暫停也走淡出。

## User Story
作為使用者，暫停與繼續沒有爆音；拖進度條沒有喀聲；開了去除靜音後，開頭有 3 秒空白的檔案會直接從聲音開始播，進度條從 0:03 起跳（時間仍然對應原檔）。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- DspSource 加淡入淡出包絡，引擎指令改成「先淡出 N ms 再暫停」；
- 各情境分開設定時間，0 為關：
  - 開始播放、暫停、繼續播放、停止：預設 150 ms
  - 拖進度條：預設 30 ms（只為了去掉爆音）
  - 手動切歌：預設 300 ms
- 曲間停頓：0–10 秒，在無縫排入之前插入指定長度的靜音；跟交叉淡化互斥；
- 去除靜音：記憶體軌在解碼後找出開頭與結尾低於 −60 dBFS 的段落，播放時跳過；長檔串流只處理開頭；預設關；
- 睡眠計時暫停時也走淡出。

## Acceptance Criteria
- [ ] 暫停與繼續沒有爆音；
- [ ] 拖進度條沒有喀聲；
- [ ] 開頭有 3 秒空白的檔案，開了去除靜音後直接從聲音開始播，進度條從 0:03 起跳；
- [ ] 曲間停頓 0–10 秒可設定，跟交叉淡化互斥；
- [ ] 長檔串流去除靜音只處理開頭；
- [ ] 睡眠計時暫停走淡出；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/dsp.rs`、`src-tauri/src/player.rs`、`src-tauri/src/commands.rs`
- 設計來源：planning/P1-playback-quality/Design.md 第 3 項
- 無縫排入依賴 P0-3
