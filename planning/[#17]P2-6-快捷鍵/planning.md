<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#17]P2-6-快捷鍵

## Description
現況快捷鍵寫死在 Shell.tsx（空白、Enter、Delete、Alt+←/→、Ctrl+F），不能自訂。需要按鍵表、本機快捷鍵、全域快捷鍵（需 tauri-plugin-global-shortcut）、衝突提示。

## User Story
作為使用者，改一個本機快捷鍵後立刻生效，關掉重開還在；全域快捷鍵在視窗縮到系統匣時也能用；設定成跟已有的快捷鍵相同時會提示衝突。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 按鍵表：動作 → 按鍵，存在 app_config 的 hotkeys；設定頁提供「按下要設定的組合鍵」與衝突提示；
- 本機快捷鍵：視窗在前景時生效；
- 全域快捷鍵：要加 tauri-plugin-global-shortcut（crate、npm 套件、capability 都要加，而且不能只放 core:default，否則其他指令會被擋）。預設全部不綁；
- 不綁 MediaPlayPause 等媒體鍵：已經由 SMTC 處理，搶走會跟其他播放器衝突（沿用現有規則）；
- 可以設定的動作：播放／暫停、停止、上一首、下一首、音量加減、靜音、快轉與倒退 N 秒、切換播放模式、加書籤、評分 1–5、開關等化器、開關迷你模式、顯示或隱藏視窗。

## Acceptance Criteria
- [ ] 改一個本機快捷鍵後立刻生效，關掉重開還在；
- [ ] 全域快捷鍵在視窗縮到系統匣時也能用；
- [ ] 設定成跟已有的快捷鍵相同時會提示衝突；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src/player/Shell.tsx`、`src/lib/api.ts`、`src-tauri/src/main.rs`、`src-tauri/build.rs`
- 設計來源：planning/P2-library-playlists/Design.md 第 6 項
