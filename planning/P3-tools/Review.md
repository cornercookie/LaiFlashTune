# P3 Review

審查者：Codex（OpenAI，`codex exec --sandbox read-only`，簡稱 C）。預審另有 OpenCode + Nvidia Nemotron 3 Ultra（簡稱 O）；之後 OpenCode 只負責實作。作者不審自己的文件。

狀態：**已修**｜**成立**（待改 Design.md）｜**不成立**（附核對依據）｜**已涵蓋**（Design 已有）｜**待核對**（還沒拿原始資料驗證）｜**待決定**（需要使用者選）

## Design 審查

### 預審（2026-11-22，對象是改名前的 planning.md，Spec 與 Code 混在一起）

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C#9 | 阻擋 | `shutdown /s /t 60` 隱含 `/f`，會強制關閉其他程式並遺失未存的資料 | Microsoft 官方文件：`/t` 大於 0 時隱含 `/f` | 成立 |
| 2 | C#10 | 阻擋 | 「ffmpeg 在 Windows 不能讀 CD」不成立：ffmpeg 有 libcdio | 本機 `ffmpeg -devices` 列出 libcdio；光碟機實測未做 | 成立 |
| 3 | C#20 | 重要 | ReplayGain 計算規格不足（參考響度、峰值種類、比較基準） | — | 成立 |
| 4 | C#21、O#33 | 重要 | `loudnorm` 要指定線性或動態、單次或雙次；動態模式會升頻到 192k | FFmpeg 官方文件 | 成立 |
| 5 | C#22 | 重要 | 改檔名或搬檔沒有部分失敗的回復；漏了 CUE 與綁定檔案的預設 | — | 成立 |
| 6 | C#23 | 重要 | MIDI 需要能載入 SoundFont 的合成後端，ffmpeg 沒有這個參數 | — | 成立 |
| 7 | C#24、O#21 | 重要 | 「tee」無法同時得到播放用的 PCM 與原樣保存的壓縮串流 | FFmpeg tee 文件 | 成立 |
| 8 | C#25 | 重要 | `SetWaitableTimer` 要用絕對時間；喚醒可能不受支援，卻仍回傳成功 | Microsoft 官方文件 | 成立 |
| 9 | C#32 | 重要 | 轉寫、轉檔時的峰值正規化、錄音成 APE／WMA、編碼成 WMA Pro 都沒排進任何一期 | 對照官方功能表；本機 ffmpeg 有 `wmapro` 解碼器，編碼器待查 | 成立 |
| 10 | C#33 | 重要 | 「做不到就不做」會刪掉功能差距，應改成「受阻」並保留待辦 | — | 成立 |
| 11 | C#34、O#38 | 重要 | 完成條件不可重現（CPU 吃滿、約 2%、BPM 誤差沒有定值） | — | 成立 |
| 12 | O#8 | 重要 | CD 抓軌需要系統管理員權限 | 跟 C#10 的 libcdio 路線矛盾，要等光碟機實測 | 待核對 |
| 13 | O#9 | 重要 | 鬧鐘只在程式執行時有效 | Design 已寫明 | 已涵蓋 |
| 14 | O#16 | 次要 | 頻譜要在區塊層級取樣，不是逐樣本 | Design 的圖已畫在 `DspSource.fill()` | 已涵蓋 |
| 15 | O#25 | 次要 | 要先確認 ffmpeg 有沒有 libopenmpt | 本機 `ffmpeg -demuxers` 有 libopenmpt 與 libmodplug | 成立（現況可更新為有） |
| 16 | O#37 | 次要 | 簡繁轉換需要詞彙級對照（OpenCC），單字對照會出錯 | — | 成立 |

### Spec

待進行：預審中「成立」的項目改進 Design.md 之後才開始。

### Code

待進行。

### Security

待進行。

## 實作審查

### Spec

待進行。

### Code

待進行。

### Security

待進行。
