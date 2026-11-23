<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#14]P2-3-書籤

## Description
需要新增書籤功能：在目前位置加書籤、側欄書籤檢視、點書籤跳到位置播放、缺檔保留、刪除歌曲時一起刪掉書籤。

## User Story
作為使用者，在一首 2 小時錄音的 1:23:45 加書籤，關掉重開後點書籤，會從 1:23:45 開始播。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 新表 bookmarks(id, song_id, position_ms, label, created_at)；
- 快捷鍵與右鍵選單「在目前位置加書籤」；標籤預設是 `曲名 mm:ss`，可以改名；
- 側欄「書籤」檢視：點一下就播放並跳到那個位置（用 player_load 加 seek，或 player_prepare 再播放）；
- 歌曲標成缺檔時書籤保留；歌曲真的刪除時一起刪掉。

## Acceptance Criteria
- [ ] 在一首 2 小時錄音的 1:23:45 加書籤，關掉重開後點書籤，會從 1:23:45 開始播；
- [ ] 歌曲標成缺檔時書籤保留；
- [ ] 歌曲刪除時一起刪掉書籤；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/commands.rs`、`src/player/Shell.tsx`
- 設計來源：planning/P2-library-playlists/Design.md 第 3 項
