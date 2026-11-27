# SonicLedger / 音樂隨身聽

Windows 本地音樂播放器，對標 參考播放器 的日常播放能力。完全離線：不連網、不登入、不上傳。可攜式：曲庫與設定都寫在執行檔旁的 `data/`，整包丟進隨身碟換電腦直接用。

## 下載

到 [Releases](https://github.com/cornercookie/SonicLedger/releases/latest) 下載：

- `SonicLedger_<版本>_x64.exe`：可攜版，免安裝。放到可寫入的資料夾（桌面、隨身碟）直接執行，不要放進 `C:\Program Files`。
- `SonicLedger_<版本>_x64-setup.exe`：安裝版。

需求：Windows 10 1803+，系統已有 WebView2（Windows 10/11 通常已內建）。

升級可攜版：直接覆蓋舊的 exe，保留旁邊的 `data/`。

## 功能

**曲庫與清單**

- 掃描音樂資料夾（平行讀 Tag、產生封面縮圖），10 萬首清單虛擬捲動
- FTS5 全文搜尋：中文、拼音、拼音首字母、片段
- 依歌手／專輯／曲風瀏覽；專輯依碟號、音軌排序；最近播放依播放時間排序
- 播放清單頁籤（參考播放器 式工作區）：建立、改名、刪除、拖曳排序；關頁籤不會刪清單
- 智慧歌單：播放次數最多、最近加入、指定曲風等規則，開頁籤時重算
- 播放佇列：拖曳重排、存成歌單
- m3u 匯入／匯出
- 找不到的檔案標為「缺檔」留在曲庫，不會被刪掉

**播放**

- 播放模式：順序／全部循環／隨機／單曲循環
- 等化器（前級增益 −12～+12 dB、可存自訂預設）、音量標準化、交叉淡入淡出
- 切換輸出裝置，從目前位置續播
- 歌詞：自動讀同資料夾同檔名的 `.lrc`、可貼上歌詞、偏移調整
- 睡眠計時：N 分鐘後暫停，或這首播完就停
- 關閉時記住上次的歌曲、位置、音量、模式、等化器、佇列；下次開啟停在原處，不會自己出聲

**桌面整合**

- 右鍵「開啟檔案」或雙擊音訊檔直接播放（啟動時在 HKCU 註冊檔案關聯，不需系統管理員）
- 把檔案或資料夾拖進視窗：單檔直接播，資料夾加入曲庫並掃描，拖進歌單頁籤就加入該歌單
- 系統匣、媒體鍵、Windows 媒體控制（SMTC）
- 迷你模式：同一個視窗縮成 480×200 並置頂

**工具**

- 標籤編輯；批次修改歌手／專輯／曲風／年份，失敗的檔案逐一列出
- 格式轉換：mp3（128/192/320 kbps）、flac、wav、ogg，**需要本機 PATH 上有 ffmpeg**
- 網路電台可儲存使用者貼入的 HTTP／HTTPS 串流網址，包含內網電台。需要 Windows 系統 curl 與本機 FFmpeg。

## 支援格式

| 格式 | 播放 | 曲庫掃描 |
| --- | --- | --- |
| mp3、flac、wav、ogg (Vorbis) | ✓ | ✓ |
| m4a（AAC、ALAC）、aac (ADTS) | ✓ | ✓ |
| mp4 音訊、aiff / aif | ✓ | ✗（用開啟檔案或拖進視窗） |
| ape、wma、opus、wv、tta | ✗ 沒有解碼器 | — |

超過 15 分鐘的檔案（長錄音、DJ mix）邊讀邊播，不整首解進記憶體，拖曳進度條即時跳轉。這類檔案不套用音量標準化與交叉淡入淡出，因為兩者都需要事先掃過整首。

## 資料位置

全部在執行檔旁，路徑由 `std::env::current_exe()` 決定，隨身碟盤符改變不影響：

- `data/app.db`：SQLite（歌曲、FTS5 索引、歌單、智慧歌單規則、歌詞、播放歷史、電台、設定與工作階段）
- `data/covers/`：封面縮圖（200×200 JPG）
- `data/open-with.log`：開啟檔案與載入的診斷紀錄

不寫 `%APPDATA%`。唯一寫入登錄檔的是 `HKCU\Software\Classes` 的音訊檔案關聯；每次啟動重新註冊，搬動 exe 後會自動指向新位置。

備份：複製 exe 旁的 `data/` 資料夾即可。

## 技術棧

- 桌面：Tauri v2（Rust）＋ WebView2
- 前端：React 18 + TypeScript + Tailwind CSS 3（Vite），清單用 `@tanstack/react-virtual`
- 資料庫：SQLite（`rusqlite` bundled）＋ FTS5；`journal_mode=DELETE`、`synchronous=FULL`，隨身碟被直接拔掉也不會留下 `-wal`／`-shm`
- 音訊：rodio 輸出；symphonia 0.5 直接解碼（`audio_decode.rs`）
- Tag：lofty；掃描：rayon + walkdir；拼音：pinyin
- 媒體鍵／SMTC：souvlaki
- 轉檔與電台串流：外部 ffmpeg（PATH 上有才啟用）

## 專案結構

```
SonicLedger/
├── src/                        # 前端
│   ├── main.tsx                # 入口（必須 import ./index.css）
│   ├── App.tsx                 # 轉出 player/Shell
│   ├── index.css
│   ├── types.ts
│   ├── lib/api.ts              # Tauri invoke 封裝（參數一律 camelCase）
│   └── player/
│       ├── Shell.tsx           # 主畫面：側欄、頁籤、清單、底部控制列
│       ├── TrackTable.tsx      # 虛擬捲動的多欄曲目表
│       ├── Tools.tsx           # 等化器、標籤、轉檔、電台
│       ├── session.ts          # 工作階段存取
│       ├── openWithRestore.ts  # 開啟檔案與恢復上次進度的先後順序
│       └── *.test.mjs          # node --test 單元測試
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   └── src/
│       ├── main.rs             # 入口：開 data/app.db、檔案關聯、音訊執行緒、註冊命令
│       ├── commands.rs         # 所有 Tauri command
│       ├── db.rs               # 資料表、FTS5、歌單、設定
│       ├── scanner.rs          # 曲庫掃描
│       ├── player.rs           # 音訊引擎：等化器、標準化、交叉淡化、輸出裝置、電台
│       ├── audio_decode.rs     # symphonia 串流解碼器
│       ├── extras.rs           # 標籤、轉檔、電台、拖放
│       ├── open_with.rs        # argv 開檔、單一實例轉送、檔案關聯
│       ├── transport.rs        # 系統匣與媒體鍵
│       └── models.rs
├── PRODUCT.md                  # 產品定位與原則
└── PLAN.md                     # 實作計畫
```

## 開發與建置

需求：Rust（stable）、Node.js、Windows 10 1803+。

```bash
npm install
npx tauri dev                     # Vite + Tauri 視窗（npm run dev 只起 Vite，沒有後端）
npx tauri build                   # 產出 release exe 與安裝檔
node --test src/player/*.test.mjs # 前端單元測試
```

- 建置前先關掉正在跑的 `sonicledger.exe`，否則 linker 無法覆蓋。
- `cargo check` 與 `npx tauri dev` 不會更新 release exe；要看發行版行為，必須重跑 `npx tauri build`。
- 可攜版是 `src-tauri/target/release/sonicledger.exe` 單一檔案；安裝檔在 `src-tauri/target/release/bundle/nsis/` 與 `bundle/msi/`。

## 開發注意事項

- Tauri v2 command 參數是 camelCase（`songId`、`positionMs`）；送 snake_case 會被拒絕，畫面會載不出曲庫。
- `tauri.conf.json` 雖然列了 dialog／fs／shell／notification，但沒有安裝對應的 plugin crate 與 npm 套件，不要呼叫。選資料夾用 `browse_folder_cmd`。
- 不要用 `rodio::Decoder` 開檔：rodio 0.19 的 `ReadSeekSource` 不回報檔案長度，開 MP4 會直接 panic。解碼一律走 `audio_decode::SymStream`。
- `Cargo.toml` 的 `tauri` 依賴不要硬寫 `custom-protocol`，由 `[features] default = ["custom-protocol"]` 控制。
- Tailwind 鎖在 v3（v4 不相容 `tailwind.config.js`）。

## 授權

MIT
