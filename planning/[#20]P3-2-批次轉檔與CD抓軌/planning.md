<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#20]P3-2-批次轉檔與CD抓軌

## Description
現況一次一首，mp3 flac wav ogg 四種，ConvertRequest 只有 song_id / format / output_dir / bitrate_kbps。本機 ffmpeg 能編碼 mp3、aac、alac、flac、vorbis、opus、wavpack、wma，不能編碼 APE、MPC。需要轉檔佇列、外部編碼器、輸出模式、檔名範本、標籤封面、正規化、取樣率/位元深度/聲道、完成後關機、CD 抓軌實驗。

## User Story
作為使用者，50 首 flac 轉 opus，進度與完成數正確，CPU 有吃滿；結果的標籤與封面都在；整張專輯合併成一個 flac 加 CUE，掃描後跟原本的分軌一致。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 轉檔佇列：多首一起丟進去，平行處理（預設為 CPU 核心數減 1），每首用 ffmpeg -progress pipe:1 回報真實進度。不能用假進度條。可以取消；失敗的逐一列出；
- 格式：mp3、m4a（AAC）、ALAC、flac、wav、ogg、opus、WavPack、wma。位元率或品質選項依格式顯示：無損格式不顯示位元率，flac 顯示壓縮等級；
- APE、MPC 等：用「外部命令列編碼器」設定（使用者自己指定 mac.exe、mpcenc.exe 的路徑與參數範本）。沒設定時這些格式不出現在選單；
- 輸出模式：一對一；同一個資料夾合併成一個檔案並產生 CUE；全部合併成一個檔案並產生 CUE；
- 輸出檔名範本：同第 1 項的變數，撞名時列出來，不能覆蓋；
- 保留標籤與封面：-map_metadata 0，封面另外處理（各容器寫法不同）；
- 轉換時正規化：寫入 ReplayGain 標籤，或用 loudnorm 直接改音量。兩者分開選，預設是寫標籤；
- 改變格式：取樣率、位元深度、聲道（例如轉成 44.1k／16bit／立體聲）；
- 完成後關機：用 shutdown /s /t 60，畫面上可以取消；預設關；
- CD 抓軌：先做小實驗。Windows 上讀 CDDA 要用 IOCTL_CDROM_RAW_READ，ffmpeg 在 Windows 不能直接讀 CD。CD 資料查詢（MusicBrainz）要連網，預設關，屬於 P4 的連網原則。做不到就寫明不做，不能畫一個抓不了的按鈕。

## Acceptance Criteria
- [ ] 50 首 flac 轉 opus，進度與完成數正確，CPU 有吃滿；
- [ ] 結果的標籤與封面都在；
- [ ] 整張專輯合併成一個 flac 加 CUE，掃描後跟原本的分軌一致；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/extras.rs`、`src-tauri/src/commands.rs`、`src/player/Tools.tsx`
- 設計來源：planning/P3-tools/Design.md 第 2 項
- 第 1 項的 ReplayGain 計算與第 2 項的轉檔正規化都用 P1-1 的讀寫格式；第 2 項的「合併成一檔並產生 CUE」與第 1 項的章節編輯用 P1-4 的 CUE 解析；第 6 項延伸格式用 P0-2 的 ffmpeg 解碼路由
