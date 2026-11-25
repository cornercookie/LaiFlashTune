# SonicLedger 功能路線圖

這份取代原本的差距清單。比對對象一律稱「參考播放器」，**repo 內的文件、程式註解、畫面文字都不寫參考播放器的名稱**（規則見 P0 第 0 項）。要串接的服務（例如聽歌紀錄網站）照實寫名稱。

## 範圍

- 參考播放器有、SonicLedger 沒有的功能都要做（使用者 2026-11-22 決定）。
- 仍不做：內建電台目錄。電台只收使用者自己貼的網址，參考播放器也沒有官方目錄。
- 連網功能一律**預設關閉**，帳號、權杖、快取都寫在執行檔旁的 `data/`，不寫 `%APPDATA%`。
- 舊規則照舊：控制項必須接到引擎才畫；沒有的能力明說沒有；SQLite `journal_mode=DELETE`；介面繁體中文；10 萬首要虛擬捲動。

## 分期

| 期 | 資料夾 | 主題 | 狀態 |
|---|---|---|---|
| P0 | `P0-daily-blockers/` | 名稱中性化、搜尋、格式、無縫播放 | `[#2]P0-SE7-lofty解析器限制` 已完成（Design v8 審查通過，實作 + cargo test 全過）；`[#3]P0-Imp1Imp3-長檔TrackResampler與DSP鏈` 已完成（Design v10，Spec/Code/Security 三階段審查通過，cargo test 54/54 全過） |
| P1 | `P1-playback-quality/` | ReplayGain、音量曲線、淡入淡出、CUE、獨佔輸出、20 段等化器 | `[#5]P1-1-ReplayGain` 已完成（Design v6，Spec/Code/Security 三階段審查通過，cargo test 全過）；`[#6]P1-2-音量曲線與響度補償` 已完成（Design v6，Spec/Code/Security 三階段審查通過，cargo test 全過） |
| P2 | `P2-library-playlists/` | 歌單分組與顯示、歌單工具、書籤、評分、智慧歌單編輯、快捷鍵 | 待做 |
| P3 | `P3-tools/` | 標籤編輯補齊、批次轉檔、電台錄音、鬧鐘排程、頻譜視覺化、延伸格式 | 待做 |
| P4 | （尚未開資料夾） | 連網與架構項目 | 排在 P3 之後 |

每期資料夾內有 `Design.md`（設計與實作步驟、圖、完成條件）與 `Review.md`（分階段審查紀錄），流程見根目錄 `AGENTS.md`。期內順序以各 Design.md 開頭寫的為準；跨期依賴寫在項目裡。

## P4 待辦（尚未展開）

| 項目 | 備註 |
|---|---|
| 聽歌紀錄上傳 | Last.fm、Libre.fm、ListenBrainz；離線先排隊，上線再送 |
| 雲端與網路磁碟 | 先做 WebDAV 與網路路徑，各家 OAuth 之後再說 |
| Podcast | 只收使用者貼的 RSS，不做目錄 |
| 外掛 | 要先定介面 |
| 換膚 | 封底曲目維持預設，換膚是額外主題 |
| 多語系 | 字串抽成語系檔，繁中為預設 |
| 多使用者 | `data/` 底下分使用者資料夾 |
| 高 DPI 縮放選項 | 檢查 900×600 最小尺寸在 200% 下還能用 |

桌面歌詞不在參考播放器的官方功能清單，所以沒列。

## 每期都要做完的事

1. 關掉正在跑的 `src-tauri/target/release/sonicledger.exe`，執行 `npx tauri build`，確認新的 `dist/assets/index-*.js` 檔名在 exe 裡。
2. `cargo test --release --manifest-path src-tauri/Cargo.toml`、`node --test "src/player/*.test.mjs"` 全過。
3. 更新 README 的功能與格式表，不寫做不到的事。
4. `git grep -n -i -e 'a[i]mp'` 結果必須是空的。
