# SonicLedger / 音樂隨身聽

本地音樂播放器桌面應用，對標 AIMP，但更現代、更輕、更漂亮。完全本地、不連網、不登入、不上傳，支援可攜式（Portable），可放在隨身碟帶著走。

## 技術棧

- 語言：Rust
- 桌面框架：Tauri v2
- 前端：React + TypeScript + Tailwind CSS (Vite)
- 虛擬列表：@tanstack/react-virtual（10 萬首歌單即時渲染）
- 資料庫：SQLite（rusqlite bundled）+ FTS5 全文搜尋
- 音訊輸出：rodio；解碼：symphonia
- Tag 讀取：lofty；平行掃描：rayon + walkdir
- 封面縮圖：image（200×200 JPG）

## 專案結構

```
SonicLedger/
├── Cargo.toml                      # Rust 工作區根（前端建置輸出到 dist/）
├── src-tauri/
│   ├── Cargo.toml                  # Rust 依賴與 tauri-build
│   ├── build.rs                    # tauri_build::build()
│   ├── tauri.conf.json             # Tauri 設定（可攜式，plugins: dialog/fs/shell/notification）
│   └── src/
│       ├── main.rs                 # 應用程式入口：開啟 data/app.db、啟動音訊執行緒、註冊命令
│       ├── models.rs               # 資料模型（Song / Playlist / Lyrics / PlayerCommand …）
│       ├── db.rs                   # 資料庫層：init_db / FTS5 / 掃描差比 / 歌單 / 歌詞 / 設定
│       ├── scanner.rs              # 音樂庫掃描（rayon+walkdir、lofty Tag、封面縮圖、拼音）
│       ├── player.rs               # 播放器（rodio Sink + symphonia 解碼、mpsc 指令、無縫預載）
│       └── commands.rs             # 所有 Tauri Command（皆附中文註解）
├── src/                            # Vite 前端
│   ├── main.tsx
│   ├── App.tsx
│   ├── index.css                   # Tailwind + 深色佈景主題
│   ├── types.ts                    # TypeScript 型別
│   ├── lib/api.ts                  # Tauri invoke 封裝與事件監聽
│   └── components/                 # SearchBar / SongList / PlayerBar / LyricsPanel / PlaylistPanel / Settings
├── index.html
├── package.json
├── vite.config.ts
├── tailwind.config.js
├── postcss.config.js
├── tsconfig.json
└── README.md
```

## 核心設計（可攜式，最高優先級）

執行檔旁自動建立 `data/` 資料夾，所有資料皆寫於此，不碰 `%APPDATA%`、不寫登錄檔：

- `<exe>/data/app.db` — SQLite 資料庫（歌曲、FTS5 索引、歌單、歌詞、播放歷史、設定）
- `<exe>/data/covers/` — 封面縮圖（200×200 JPG，以檔案路徑雜湊命名）
- `<exe>/data/config.json` — 使用者設定（音樂資料夾、佈景主題、歌詞偏移）

路徑透過 `std::env::current_exe()` 決定，因此隨身碟盤符變更（D:/E:/F:）不影響。

## 開發與建置

### 前置需求

- Rust (cargo 1.100+)
- Node.js 與 npm
- Windows 10 1803+（內建 WebView2）

### 安裝依賴

```bash
cd SonicLedger
npm install
```

### 開發模式（啟動 Vite dev server + Tauri 視窗）

```bash
npm run dev
# 或
cargo tauri dev
```

首次啟動會在執行檔旁建立 `data/` 與 `data/app.db`，並自動建表。

### 組建發行版（單一 .exe）

```bash
npm run build           # 前端編譯到 dist/
cargo tauri build       # 編譯 Rust，產出 src-tauri/target/release/sonicledger.exe
```

發行預設只交付單一 `.exe`（不需複製整個 `target/release/`）。第一次在目標機器啟動時，`data/` 會在 exe 旁自動建立，因此請將 exe 放在可寫入目錄（桌面、隨身碟），不要放入 `C:\Program Files`。

### 以隨身碟攜帶

1. 將 `src-tauri/target/release/sonicledger.exe` 複製到隨身碟根目錄。
2. 插入任何 Windows 電腦，直接雙擊 `.exe` 即可啟動（需系統 WebView2）。
3. 在設定頁新增音樂資料夾（例如隨身碟上的 `D:\Music`），點「立即掃描音樂庫」。
4. 資料庫與封面皆寫入隨身碟旁的 `data/`，換電腦攜帶即可繼續使用。

備份時，將 exe 旁的 `data/` 資料夾一併複製即可。

## 主要 Tauri Command（後端）

全部命令皆在 `src-tauri/src/commands.rs`，每個皆附中文註解：

- `scan_folders` — 掃描音樂資料夾（平行讀取 Tag、產生封面縮圖、寫入資料庫，進度經由 `scan-progress` 事件回傳）
- `get_songs` / `search_songs_cmd` — 列出歌曲 / FTS5 全文搜尋（中文、拼音、拼音首字母、片段）
- `player_load` / `player_preload` / `player_play` / `player_pause` / `player_toggle` / `player_stop` / `player_next` / `player_prev` / `player_seek` / `player_set_volume` / `player_set_mode`
- `create_playlist_cmd` / `list_playlists_cmd` / `list_playlist_items_cmd` / `add_song_to_playlist_cmd` / `remove_from_playlist_cmd` / `update_playlist_order_cmd`
- `save_lyrics_cmd` / `get_lyrics_cmd` / `set_lyrics_offset_cmd`
- `add_music_folder_cmd` / `list_music_folders_cmd` / `load_config_cmd` / `save_config_cmd`
- `app_paths` — 回傳可攜式資料路徑

## 開發順序（對照需求規格）

1. 掃描器 + DB + FTS5 搜尋（先確保 10 萬首搜尋快速）
2. 播放器 + 播放佇列
3. 歌詞解析 + 同步（LRC 二分搜尋定位、±0.5s 偏移）
4. 歌單 + UI 美化

## 注意事項

- `tauri.conf.json` 中 `plugins` 須開啟 `dialog` / `fs` / `shell` / `notification`；`Cargo.toml` 的 `tauri` 依賴不得硬編碼 `custom-protocol`，請透過 `[features] default = ["custom-protocol"]` 控制。
- SQLite 採用 `journal_mode = DELETE` + `synchronous = FULL`，避免隨身碟拔除不正常卸載損壞 `-wal`/`-shm`。
- `rusqlite` 必須開 `bundled` 特性，讓目標機器無需安裝 SQLite 開發庫。
- 前端 Tailwind 鎖定 v3.x（v4 不相容 `tailwind.config.js`）。

## 授權

MIT
