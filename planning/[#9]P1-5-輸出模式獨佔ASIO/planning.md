<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#9]P1-5-輸出模式獨佔ASIO

## Description
現況 rodio/cpal，WASAPI 共享模式。cpal 不支援獨佔模式；ASIO 要在建置時有 ASIO SDK。需要輸出後端抽象化，支援 WASAPI 獨佔、位元完美、ASIO（另出建置版本）。

## User Story
作為使用者，選了獨佔後，其他程式的聲音被擋掉；48k／96k 的檔案能各自讓裝置切換格式；切回共享後一切照常。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 輸出後端抽象化：OutputBackend trait，現有 rodio 成為「WASAPI 共享」實作；
- WASAPI 獨佔：用 wasapi crate 直接開裝置，依來源取樣率切換裝置格式；裝置不支援時退回最接近的格式並顯示原因；
- 位元完美：音量 100%、等化器與音效全關、沒有 ReplayGain 時跳過所有處理，狀態列顯示「位元完美」；
- ASIO：cpal 的 asio feature，需要 ASIO SDK 與使用者自己裝的驅動。可攜版預設不包含，另出一個建置版本；
- DirectSound：價值低，最後評估；沒有實際需求就寫明不做；
- 換後端時位置不能跳，比照現有的切換輸出裝置。

## Acceptance Criteria
- [ ] 選了獨佔後，其他程式的聲音被擋掉；
- [ ] 48k／96k 的檔案能各自讓裝置切換格式；
- [ ] 切回共享後一切照常；
- [ ] 位元完美條件滿足時狀態列顯示「位元完美」；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`Cargo.toml`
- 設計來源：planning/P1-playback-quality/Design.md 第 5 項
- 每一步都先做小實驗，做不到就不畫選項
