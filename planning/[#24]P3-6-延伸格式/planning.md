<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#24]P3-6-延伸格式

## Description
現況僅支援主流音訊格式。需要支援模組音樂（mod、xm、it、s3m、mo3、umx 等，需 libopenmpt）、MIDI（需 SoundFont）、音樂 CD 直接播放（.cda）、串流網址（MPEG-DASH 等，需連網，歸 P4，預設關）。每一種先確認本機 ffmpeg 支不支援，不支援就不列進格式表。

## User Story
作為使用者，支援的格式能播能拖進度條；不支援的不會出現在掃描副檔名與格式表裡。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 模組音樂：ffmpeg 編譯時要有 libopenmpt，先確認目前用的版本有沒有；
- MIDI：需要 SoundFont。設定頁可以指定 .sf2；沒有 SoundFont 就明說無法播放；
- 音樂 CD 直接播放（.cda）：跟第 2 項 CD 抓軌一起做小實驗；
- 串流網址（MPEG-DASH 等）：需要連網，歸 P4，預設關。

## Acceptance Criteria
- [ ] 支援的格式能播能拖進度條；
- [ ] 不支援的不會出現在掃描副檔名與格式表裡；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/scanner.rs`、`src-tauri/src/audio_decode.rs`、`src-tauri/src/commands.rs`
- 設計來源：planning/P3-tools/Design.md 第 6 項
- 依賴 P0-2 的 ffmpeg 路由
