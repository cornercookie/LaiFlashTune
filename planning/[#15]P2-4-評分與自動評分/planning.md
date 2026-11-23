<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#15]P2-4-評分與自動評分

## Description
需要加入評分與自動評分：songs 加 rating（0–5，NULL 表示沒評過）、skip_count；清單與正在播放區可點星星；自動評分依播放次數、播完率與略過次數計算；手動評分永遠優先。

## User Story
作為使用者，評分關掉重開還在；常常聽完的歌自動評分會上升，常被略過的會下降；手動評過的不被自動值蓋掉。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- songs 加 rating（0–5，NULL 表示沒評過）、skip_count；
- 清單評分欄與正在播放區都可以直接點星星；
- 自動評分（預設開）：依播放次數、播完率與略過次數計算。略過的定義是播不到 30% 就換下一首。手動評分永遠優先，自動值另存在 auto_rating；
- 智慧歌單可以用 rating、skip_count 當條件（加進欄位白名單）；
- 寫回檔案標籤（選用，預設關）：ID3 POPM、Vorbis RATING。寫失敗的檔案逐一列出。

## Acceptance Criteria
- [ ] 評分關掉重開還在；
- [ ] 常常聽完的歌自動評分會上升，常被略過的會下降；
- [ ] 手動評過的不被自動值蓋掉；
- [ ] 寫失敗的檔案逐一列出；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/player.rs`、`src/player/Shell.tsx`
- 設計來源：planning/P2-library-playlists/Design.md 第 4 項
- 第 4 項評分的「略過次數」靠 P0-3 無縫播放裡的換曲事件
