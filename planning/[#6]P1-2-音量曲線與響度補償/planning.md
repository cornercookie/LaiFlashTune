<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#6]P1-2-音量曲線與響度補償

## Description
現況 sink.set_volume(volume) 是線性，滑桿下半段幾乎聽不出差別，上半段一動就很大聲。需要加入對數曲線與響度補償。

## User Story
作為使用者，滑桿 25%、50%、75% 聽起來音量差距平均；開啟響度補償後，小音量時能聽得出低頻與高頻補回，100% 時不作用。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 對數曲線：滑桿 0–100% 對應 −60 dB 到 0 dB（0% 就是靜音），換算在引擎內做，前端滑桿值不變；
- 設定裡可以切回線性；
- 響度補償（預設關）：音量越小，自動在 DspSource 加低頻與高頻的擱架增益（最多約 +10 dB 與 +6 dB）；
- 狀態列的音量顯示加上 dB 值。

## Acceptance Criteria
- [ ] 滑桿 0–100% 對應 −60 dB 到 0 dB 的對數曲線；
- [ ] 可在設定裡切回線性曲線；
- [ ] 響度補償開關預設關；
- [ ] 小音量時開啟響度補償聽得出差別，100% 時不作用；
- [ ] 狀態列音量顯示加上 dB 值；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/player.rs`、`src-tauri/src/dsp.rs`、`src/player/Shell.tsx`
- 設計來源：planning/P1-playback-quality/Design.md 第 2 項
