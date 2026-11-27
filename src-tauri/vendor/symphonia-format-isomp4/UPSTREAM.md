# Symphonia MP4 套件來源

- 本地套件來自 `symphonia-format-isomp4 0.5.5`，上游提交為 `6d533f26150953a882a6a111ebd13f0abf7129d5`。[crates.io 發布版](https://crates.io/crates/symphonia-format-isomp4/0.5.5)
- 原始 registry 套件的 SHA-256 為 `243739585d11f81daf8dac8d9f3d18cc7898f6c09a259675fc364b382c30e0a5`。
- 原始碼沿用 Mozilla Public License 2.0，本目錄的 `LICENSE` 保存固定上游版本的授權原文。[上游授權](https://raw.githubusercontent.com/pdeljanov/Symphonia/6d533f26150953a882a6a111ebd13f0abf7129d5/LICENSE)

## 本地補丁

`src/demuxer.rs` 在配置封包樣本緩衝前檢查宣告長度。普通與 fragmented MP4 共用此配置入口。長度超過 256 KiB 時回傳 Symphonia 的 `LimitError`，由應用的讀取層保留終態。MP4 播放讀取器及 `moov` 迭代器略過頂層和電影層的 `meta` 與 `udta`，音訊軌與索引照常解析。媒體庫掃描使用各自的標籤入口。索引表有自己的配置入口。

## 更新方式

1. 從 registry 取得指定版本並核對原始套件 SHA-256。保留該版本授權與來源資訊。
2. 檢查普通與 fragmented MP4 的樣本配置入口是否仍有相同的上限。需要保留補丁時，在配置前套用檢查並保留型別化錯誤。
3. 執行套件測試、應用測試及依賴樹檢查，確認實際使用本地套件。適用的相容性輸入包含一般 MP4 音訊與基本取樣回歸。

```powershell
cargo tree --offline --manifest-path src-tauri/Cargo.toml -i symphonia-format-isomp4
cargo test --offline --manifest-path src-tauri/vendor/symphonia-format-isomp4/Cargo.toml
cargo test --offline --manifest-path src-tauri/Cargo.toml
```
