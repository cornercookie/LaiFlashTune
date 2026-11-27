# symphonia-bundle-flac 0.5.5

源碼保留 MPL-2.0，全文見同資料夾的 `LICENSE`。Registry 封包 SHA-256 為 `c91565e180aea25d9b80a910c546802526ffd0072d0b8974e3ebe59b686c9976`。所有來源由上游 rev `6d533f26150953a882a6a111ebd13f0abf7129d5` 發行。[上游原始碼](https://github.com/pdeljanov/Symphonia/tree/6d533f26150953a882a6a111ebd13f0abf7129d5/symphonia-bundle-flac)

`src/parser.rs` 在讀取、累積與合併前限制原生音訊封包，僅保留固定長度的下一幀標頭前瞻。`src/demuxer.rs` 跳過播放不需要的應用、註解、cue 及圖片區塊。 音訊封包上限為 256 KiB。替換新版時重新比對配置、型別化拒絕及標籤路徑，再更新 Cargo path override 與鎖定資料。
