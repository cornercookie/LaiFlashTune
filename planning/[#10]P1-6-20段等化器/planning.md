<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#10]P1-6-20段等化器

## Description
現況 18 段半八度（eq.rs EQ_HZ、eq.ts）。參考播放器新版官方寫 20 段。需要擴充到 20 段，並處理舊檔遷移與超過奈奎斯特頻率的頻段停用。

## User Story
作為使用者，舊的 18 段預設載入後曲線形狀一樣；20 段在 44.1k／48k／96k 都通過中心頻率測試；22.6k 那段在 44.1k 下顯示停用。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 開工前先確認參考播放器新版的 20 個頻率點（公開資料推測是 31.25 × 2^(n/2)）；
- 超過 0.48 × 取樣率的頻段（44.1k／48k 下的 22.6k）在引擎裡停用，推桿標成「此取樣率不適用」；
- 18 段存檔（session、預設、情境）依對數頻率換算成 20 段，沿用 migrateBands 作法；
- design_gains 的干擾補償矩陣改成 20×20；
- eq.rs 與 eq.ts 的頻率表一起改，測試同步更新（每段中心頻率的誤差要在 0.6 dB 內）。

## Acceptance Criteria
- [ ] 舊的 18 段預設載入後曲線形狀一樣；
- [ ] 20 段在 44.1k／48k／96k 都通過中心頻率測試；
- [ ] 22.6k 那段在 44.1k 下顯示停用；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/eq.rs`、`src/player/eq.ts`
- 設計來源：planning/P1-playback-quality/Design.md 第 6 項
