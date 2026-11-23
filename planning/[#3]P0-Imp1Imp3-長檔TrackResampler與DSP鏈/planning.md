<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#3]P0-Imp1Imp3-長檔TrackResampler與DSP鏈

## Description
長檔（串流軌）繞過 TrackResampler，直接進入輸出，未經重取樣到固定 48kHz。`apply_dsp()` 僅套用 EQ 與正規化，缺少殘響、節奏、音高、回音等 DSP 鏈。長檔播放時無法走 TrackChain→TrackResampler→DSP→DeviceConverter→Sink 的長駐管線。

## User Story
作為使用者，播放超過 15 分鐘或長度不明的音檔時，樣本應經 TrackResampler 重取樣到 48kHz 後再進入 DSP 與輸出；DSP 鏈應包含除 EQ/正規化外的音效（殘響、節奏、音高、回音等），長檔與短檔走同一條處理路徑。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- `TrackResampler` 在長檔串流路徑中被配置（`set_track` 指定原生取樣率/聲道）；
- `next_playing()` 中串流樣本也經過 `TrackResampler::process()` 重取樣到 48kHz；
- `apply_dps()` 套用 `FxChain`（殘響、節奏、音高、回音等），不只 EQ 與正規化。

## Acceptance Criteria
- [ ] 長檔串流樣本經 TrackResampler 重取樣到 48kHz；
- [ ] `apply_dsp()` 套用 FxChain（含殘響、節奏、音高、回音等）；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/playback_pipeline.rs`、`src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`
- 審查來源：Review.md Imp#1、Imp#3（Spec 第 3 輪實作審查）；Compacted Summary 中「仍待解決」列表
- 設計參考：planning/P0-daily-blockers/design.md Imp#1、Imp#3 章節
