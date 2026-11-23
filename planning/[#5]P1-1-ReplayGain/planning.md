<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#5]P1-1-ReplayGain

## Description
現況只有峰值正規化（player.rs 的 memory_track 算 norm_gain），長檔串流不套用。需要加入 ReplayGain 讀取與套用，支援曲目／專輯模式，並與峰值正規化互斥。

## User Story
作為使用者，播放音量明顯不同的兩首歌時，開啟 ReplayGain 後聽起來音量接近；狀態列顯示目前套用的增益；沒有標籤的檔案套用「無 ReplayGain 前級」。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 掃描時讀 REPLAYGAIN_TRACK_GAIN / _PEAK、REPLAYGAIN_ALBUM_GAIN / _PEAK（ID3 TXXX、Vorbis、APE、MP4 freeform、Opus R128_TRACK_GAIN）；
- `songs` 加 4 欄（缺欄才 ALTER）；
- 模式：關閉／曲目／專輯／依播放方式自動選；
- 有 ReplayGain 的檔案一個前級（預設 0 dB），沒有的另一個前級（預設 −6 dB）；
- 防削波：用峰值限制增益，避免超過 1.0；
- 與峰值正規化互斥，設定裡二選一；
- 長檔串流也能套用（不需要先掃整首）；
- 增益接進 EqProcessor 現有的 gain_target（已有平滑），不另外開一條路。

## Acceptance Criteria
- [ ] 掃描讀取 ReplayGain 標籤並寫入 songs 欄位；
- [ ] 曲目／專輯／自動模式切換正確套用增益；
- [ ] 有 ReplayGain 標籤的兩首歌音量明顯不同；切到曲目模式後聽起來接近；
- [ ] 狀態列顯示目前套用的增益；
- [ ] 沒有標籤的檔案套用「無 ReplayGain 前級」；
- [ ] 與峰值正規化互斥；
- [ ] 長檔串流也能套用 ReplayGain；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/scanner.rs`、`src-tauri/src/eq.rs`、`src-tauri/src/player.rs`
- 設計來源：planning/P1-playback-quality/Design.md 第 1 項
- ReplayGain 計算歸 P3-1（ebur128 crate），這裡只做讀取與套用
