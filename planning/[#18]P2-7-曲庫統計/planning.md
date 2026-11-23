<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#18]P2-7-曲庫統計

## Description
需要一個統計檢視，顯示總曲數、總長度、總容量、格式分布、最常播的歌手與專輯、每月播放量（從 play_history 統計）。只讀，不新增資料表。

## User Story
作為使用者，統計數字跟用 SQL 直接查的結果一致。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 統計檢視：顯示總曲數、總長度、總容量、格式分布、最常播的歌手與專輯、每月播放量（從 play_history 統計）；
- 只讀，不新增資料表。

## Acceptance Criteria
- [ ] 統計數字跟用 SQL 直接查的結果一致；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/commands.rs`、`src/player/Shell.tsx`
- 設計來源：planning/P2-library-playlists/Design.md 第 7 項
