<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#23]P3-5-頻譜視覺化

## Description
需要頻譜視覺化：在 DspSource 處理完、限幅之後，把樣本複製到一個無鎖環形緩衝區（只在視覺化開啟時寫入）；用 rustfft 2048 點、Hann 窗，合併成 32／64 個對數頻帶加峰值保持；前端 canvas 繪製；預設關閉；暫停時停止計算與繪製；電台也有效；資源預算：開啟時 CPU 增加不超過約 2%；視窗縮到系統匣時停止繪製。

## User Story
作為使用者，播 1kHz 正弦波時，峰值出現在 1k 附近的頻帶；暫停後頻譜歸零、CPU 回到原本的水準。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 取樣點：在 DspSource 處理完、限幅之後，把樣本複製到一個無鎖環形緩衝區（只在視覺化開啟時寫入）；
- FFT：rustfft，2048 點、Hann 窗，合併成 32／64 個對數頻帶加峰值保持。由指令 player_spectrum 以約 30 fps 取得，或用 Tauri 事件推送，選比較省的那個；
- 前端：canvas 繪製，放在右側面板與迷你模式；顏色用現有變數（#e23b2f 等），不另外造一套；
- 預設關閉；暫停時停止計算與繪製；電台也有效，因為電台同樣經過 DspSource；
- 資源預算：開啟時 CPU 增加不超過約 2%；視窗縮到系統匣時停止繪製。

## Acceptance Criteria
- [ ] 播 1kHz 正弦波時，峰值出現在 1k 附近的頻帶；
- [ ] 暫停後頻譜歸零、CPU 回到原本的水準；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/dsp.rs`、`src-tauri/src/player.rs`、`src-tauri/src/commands.rs`、`src/player/Tools.tsx`
- 設計來源：planning/P3-tools/Design.md 第 5 項
