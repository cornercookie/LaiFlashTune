<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#21]P3-3-電台錄音與播放清單網址

## Description
現況電台只能播（ffmpeg 轉成 PCM 給 LiveSource），不能錄；貼 .pls／.m3u 網址會失敗。需要電台錄音、依曲目分檔、播放清單網址下載解析、錄音由排程啟停。

## User Story
作為使用者，錄一個 mp3 電台 5 分鐘，原樣保存的檔案能直接播，長度大約 5 分鐘；貼一個 .pls 網址能播出聲音。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 錄音：同一個 ffmpeg 程序用 tee 輸出，或另開一個程序：
  - 原樣保存：mp3、aac 串流用 -c copy，不重新編碼；
  - 轉存：flac、ogg、mp3、wav、WavPack；
- 依曲目分檔（選用）：ICY 標題換了就切一個新檔，檔名用標題；沒有標題就照時間切；
- 錄音中狀態列顯示檔名與已錄時間。停止播放、切台、關閉程式時都要把檔案正確收尾；
- 播放清單網址：貼 .pls、.m3u、.m3u8、.asx 時先下載、解析出真正的串流網址。HLS（m3u8）交給 ffmpeg 處理；
- 錄音可以由排程（第 4 項）開始或停止。

## Acceptance Criteria
- [ ] 錄一個 mp3 電台 5 分鐘，原樣保存的檔案能直接播，長度大約 5 分鐘；
- [ ] 貼一個 .pls 網址能播出聲音；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/player.rs`、`src-tauri/src/audio_decode.rs`、`src-tauri/src/commands.rs`
- 設計來源：planning/P3-tools/Design.md 第 3 項
