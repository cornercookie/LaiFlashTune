# Hermes #1～#7 缺陷修復 Review

| 基線 | 審查規則 |
| --- | --- |
| `be94cf4`（`v0.2.2`） | 依使用者於 2026-11-27 的指示，Claude CLI 與 AGY CLI 擔任獨立審查者，OpenCode CLI 擔任實作代理。Codex 彙整設計與證據。 |

## 1. 設計審查

### 2026-11-27，v0.1，Claude CLI（claude-sonnet-5-5）

審查對象 SHA-256 `4BA1EEA0291EDBDF7721AC954E02D6DA466C8035C558A59DCAFE2F90B261544D`。唯讀靜態檢查，退出碼 0。

| 批次 | Spec | Code | Security | 主要阻擋 |
| --- | --- | --- | --- | --- |
| A | 需修正 | 受阻 | 受阻 | 快照鍵未全數對齊、未定義後端公開數值、載入路徑遺漏。清洗需補 EOF、中斷與截斷邊界。 |
| B | 需修正 | 受阻 | 受阻 | 缺少 #3 處置表、長檔剪靜音的期限及工作生命週期未定。 |
| C | 需修正 | 受阻 | 受阻 | 需逐格式列出配置前檢查點、圖片略過、同一檔案 handle 及相容證據。 |

共同缺口為現況定位、完成條件、風險及收尾項目。A 批另一次同版窄審亦確認後端發布和增益語義兩項阻擋。兩份原始報告在隔離副本同層的 `SL-fixes-evidence`，檔名為 `design-v01-claude.json` 與 `design-v01-A-claude.json`。

### v0.2 進度

A 批已補前述設計條款，待獨立複審。B、C 仍須依 v0.1 發現補設計。

### 2026-11-27，v0.2 A，Claude CLI（claude-sonnet-5-5）
 
審查對象 SHA-256 `38AB4F0C81B164FC1DCC64C85354FDE1BFF44AA48755412407384CA7FDDBFAB7`。退出碼 0。原始報告 `design-v02-A-claude.json`。
 
| Spec | Code | Security |
| --- | --- | --- |
| 通過 | 需修正（C1～C4） | 需修正（S1、S2） |
 
v0.3 已補 `EqShared` 單一來源、長短檔當前軌資料、Seek 拆軌前擷取、手動 Load 終態識別、EOF 中斷與超長 authority 的處置，待獨立複審。B 批次同步補上 #3 處置及 #4、#7 的驗收。


### 2026-11-27，v0.3，Claude CLI（claude-sonnet-5-5）

審查對象 SHA-256 `62EE5F2E33557A29A1733C03E4E0501B94E95E09EA1A7625BD9A1EA9DD7222F8`。兩批各退出碼 0。原始報告 `design-v03-A-claude.json`、`design-v03-B-claude.json`。

| 範圍 | Spec | Code | Security | 處置 |
| --- | --- | --- | --- | --- |
| #1 日誌 | 通過 | 通過 | 通過 | 交 OpenCode 實作。 |
| #5 ReplayGain §2 第 2～5 點 | 通過 | 通過 | 通過 | 交 OpenCode 實作，只對齊 ReplayGain 快照鍵。 |
| #5 快照 §2 第 1、6 點 | 需修正 | 需修正 | 通過 | C4-a～c 為手動載入終態、一次性副作用及失敗基準。已修訂至 v0.4 待確認。 |
| B | 需修正 | 需修正 | 需修正 | S1/S2、C1～C7、SE1。已補 400 MiB 轉串流、短檔原始時間、非阻塞交接及待播回執、來源游標、播放意圖，待獨立複審。 |

### v0.4 進度

設計 SHA-256 `883054C5D1E37AAF688E8D64C815AD9115D5E6BF99565A3E51133E45B7A687D8`。快照、B、C 均另送獨立審查。C 分成 C1 檔案游標修復與 C2 配置前限制，分別判定。


### 2026-11-27，v0.4／v0.5，Claude CLI

v0.4 的 A 快照、B、C 均有原始 JSON 報告。v0.5 A 快照與 C1 窄版三階段通過，C1 保存既有外部拒絕，不把超限轉進 FFmpeg。v0.5 B 的 PP3、短檔偏移與總長、PP6 三個可獨立子範圍均通過。整體 B 仍有三個阻擋，新增到 v0.6：同一播放序號 Seek 的無候選記號保留、拒絕前已觀察的容量世代作為基準、Seek 至剪除尾端作為自然結束。

v0.4 B 安全原項目（供複審對帳）：

- SE-a：播放／Seek 保存原路徑，重開必須重新驗證。最終路徑只作子程序 input，不能放寬 UNC 禁止。
- SE-b：400 MiB 需同時限制向量容量，不可透過倍增或複製整首繞過上限，剪靜音要原地處理。
- SE-c：直接載入與預載都走有界 PCM，FFmpeg stderr 經有界清洗並保留本機 file,pipe 協定限制。
- SE-d：Windows 讀取 handle 可能暫擋標籤寫回或改名，須明示已被使用，不能更改既有 UNC 執行邊界。

C2 仍須核對配置前限制的鎖定套件原始碼，以及沒有合規後備路徑的格式。界線調整未等同通過。


### 2026-11-27，v0.6 B 及 v0.7～v0.9 C2，Claude CLI

| 版本與範圍 | 判定 | Claude CLI session |
| --- | --- | --- |
| v0.6 B §3 | Spec、Code、Security 通過，包含無候選保留、重送容量世代及剪除終點處置。 | 3bb3e29d-2f61-4596-8bd9-7629c9c0e1ad |
| v0.7 C2a | 三階段通過，可先實作第 11 次累計錯誤終止。 | 73315d93-ca7d-4e50-9c16-01d9c62edb3a |
| v0.8 C2b | 三階段有阻擋，待補首包及 EOF 分類、配置拒絕鎖定、與 B 的回退及預載終態契約。 | 見原始 design-v08-C2b-claude.json。 |
| v0.9 B 步驟 3、5 與 C2b | 三階段設計通過。允許隔離實作 guard，C2b 整合交付須完成 B 型別化終態等依賴。只涵蓋 MP4 的樣本緩衝區。 | c96de7ec-3577-4805-8d7a-bfbd8ce5388f |

v0.9 Design SHA-256 為 `693B50C5CB197A6C3D1EAA726D9C152C351A92DCE3813E87DD278EA23AFA3AD1`。C2c～e 待設計及審查，掃描器與非 MP4 格式的配置限制仍未結案。

## 2. 實作審查

已將 v0.3 通過的 #1 日誌及 ReplayGain §2 第 2～5 點交付 OpenCode，實作及獨立審查進行中。基線另在隔離副本驗證前端建置及原有測試（Rust 63/63，Node 5/5）。此結果僅作回歸基線。
### 2026-11-27，C1、C2a、PP3 第一輪實作審查

Claude CLI session `803129e0-cc84-46a9-8123-0a32bbad3b8f` 審閱隔離快照差異及 68 項 Rust 測試結果。

| 範圍 | Spec | Code | Security |
| --- | --- | --- | --- |
| C1 檔案游標 | 通過 | 通過 | 通過（所示入口） |
| C2a 解碼錯誤門檻 | 待補 | 待補 | 未見新增風險 |
| PP3 交界紀錄上限 | 通過 | 通過 | 通過 |

C2a 第一次補測快照有 69 項 Rust 測試通過。Claude CLI session `484b3bad-20cc-4f5a-93ac-bff00993e5ad` 同意累計計數接縫，仍要求以字面 10 及 11 鎖定門檻並驗證實際 refill 流程。補測進行中。

### 2026-11-27，日誌、掃描路徑、PCM 與 MP4 子範圍

- v0.10 A1 複審指出切點案例必須比較同一收尾模式。v0.11 補齊完整字串和 stderr 收尾及短讀語義，Claude CLI session `c906ac0b-987a-4e62-a035-5c46d9ac6d10` 的 Spec、Code、Security 三階段通過。Design SHA-256 為 `3164D22D9DC50766A87DC18FF2C52C528C7681625BB2F988F0036CA70F22D9ED`。
- A1 第一輪實作審查要求恢復非網址查詢清除。已恢復、維持 ASCII 輸出，並追加跨區塊測試。

| 實作範圍 | 獨立判定 | 本機證據 | Claude CLI session |
| --- | --- | --- | --- |
| A1 日誌清洗與 XF2 掃描錯誤路徑 | 三階段通過，既有使用者目錄遮蔽範圍保留。 | 實際兩個 stderr 入口與掃描遮蔽測試。隔離快照 81 項 Rust 測試全過。實際電台、子程序逾時及取消仍待執行期驗證。 | 744c5929-5fb8-480a-b609-175ea1af6ee8 |
| PP9 容量收集器及原地剪除 | 三階段通過，限共用收集器及裁切。短檔 FFmpeg stdout、預載及超限轉串流仍須接通。 | 隔離快照 80 項 Rust 測試全過，含等於上限、一筆超限、容量及原地修剪。 | 99ca6c6a-c8ef-41f9-95ef-a00b815bccaf |
| C2b MP4 套件樣本配置檢查 | 三階段通過，限鎖定的 Symphonia isomp4 樣本緩衝區。B 橋接及 App 建置待驗證。 | 套件單獨離線 2 項測試全過，同一 Symphonia core 0.5.5。尚在隔離實作資料夾。 | 93e9dc96-2ecc-4317-8509-a8c2a6412c20 |
| B1 記憶體曲間交接 | 退回。暫停後的停頓狀態、Seek／Stop／Load 撤銷、身分發布及回呼內的工作生命週期待修。 | 隔離快照 86 項 Rust 測試全過，仍未覆蓋複審指出的交界情境。 | 467d8189-3718-4f1e-9796-364529b9f822 |

Node 既有測試 5/5 通過，前端中間版本 TypeScript 及 Vite 建置成功。各快照非相同樹，正式整合、前端資源核對及實機驗收另有關卡。

### 2026-11-27，解碼錯誤門檻與已審範圍的整合驗證

- C2a 已補真實 `SymStream::refill()` 的回歸測試，使用一般 PCM WAV 與只在測試中置換的解碼器回傳結果。第 10 次可恢復、第 11 次終止、成功不重置均有驗證。Claude CLI session `9c12c38d-92ed-4d79-b10c-a3b53c941dbb` 三階段通過。
- 已審整合快照涵蓋 A1、C1、C2a、PP3、PP9 容量與原地修剪及 XF2。完成 Rust 83/83、Node 5/5、TypeScript 和 Vite 建置。Tauri build 成功產生 Windows exe、MSI 及 NSIS 安裝程式，exe 內精確比對到 `index-BDQGuyub.js`。來源雜湊見 `reviewed-checkpoint-content-manifest.txt`。封裝前後比對時，發現隔離修復目錄的日誌檔有一份草稿差異，已保存差異並恢復為已審版本，再逐檔核對。封裝快照的日誌檔一直符合已審雜湊。
- A2b 第一輪實作複審 session `8bfcb1da-d72c-433d-8dec-ccb3cab5e8fc` 仍要求單一 EqShared 模式來源、曲目 Peak 增益寫入及暫停與 EQ 前級測試。修正尚在隔離實作資料夾。
- C2e v0.12 設計複審要求所有退出路徑保留配額直到回收。v0.13 以先建探測工作再啟子程序處理所有權，Claude CLI session `2bc6dd3c-bd74-49ed-892c-dc3a535f9987` 三階段設計通過。合成普通 WAV、WMA 的原五欄位與投影值相同，WMA 掃描模式仍讀到標題。應用管線實作待驗證。

### 2026-11-27，整合快照第三方複審

Claude CLI session `132b88b2-a153-4680-b9ee-54a25e16650d` 的 Spec、Code、Security 判定有條件通過，重點事項如下。

| 項目 | 狀態 | 核對結果 |
| --- | --- | --- |
| I-1／S-5 短檔前置解碼入口 | 待修 | 游標修正後，本來可能進 FFmpeg 的正常檔案能進 Symphonia。預載短檔分支目前仍有無界 `collect()`，15 分鐘的門檻無法代替 PCM 容量限制。已安排兩條 Symphonia 短檔路徑的界線與轉串流測試。 |
| I-2 日誌回傳值 | 已核對 | 正式入口 `sanitize_log`、共用 stderr、電台 stderr 均從收尾取得結果，未使用 `sanitize_chunk` 回傳值。 |
| I-3／I-4 不完整與含糊網址 | 不修 | 保守收尾及格式含糊時捨棄符合 Design v0.11。實際診斷文字可用性仍列執行期驗收。 |
| I-5／I-6 PCM 容量口徑 | 待確認 | 已測向量容量與原地裁切。系統分配器搬移期間的峰值、其他緩衝及來源並行需另量測。 |
| I-7 掃描錯誤路徑讀者 | 已核對 | 前端用掃描進度回傳的錯誤列表顯示路徑與原因；列表使用索引鍵，未回送當作曲庫路徑。 |
| I-8 同檔案 handle 並行 | 待確認 | 交接及重開路徑在 B／C2b 整合時驗證。 |
| S-1 `file://` | 已核對 | 基準版本嘗試辨識的位置位於 `h` 字元的分支內。設計及實作審查範圍為 HTTP(S)。 |
| S-3 本機路徑及錯誤理由 | 待確認 | XF2 沿用既有使用者目錄遮蔽，其他路徑形式及錯誤理由維持分項查驗。 |
| R-1 版本 | 已核對 | 原 repo 的 `v0.2.2` 指向 `be94cf4`。同一提交的 Tauri、Cargo、package 版本均已是 `0.1.1`。封裝沿用基準版本，用於隔離測試。 |

目前整合產物的範圍為前述六個子項目，仍需完成 I-1、主播放與第三方實機驗收。正式使用的播放／Seek／預載格式相容性待驗證。

### 2026-11-27，AGY CLI 接續獨立審查

Claude CLI 達當時使用量上限。Grok CLI 在此 Windows 環境回報唯讀沙箱設定未生效，該次回應未採為審查證據。依使用者授權，改以 AGY CLI（`gemini-4-hydrogen-medium`、restricted）讀取隔離且限定的證據。設計 v0.17 SHA-256 為 `039BCCA8301BFD4283F5C26093B63599E9BF51B2CD4723D1933F08B68DE9DFB4`。

| 審查對象 | 判定與處置 | 會話或工作 ID |
| --- | --- | --- |
| C2d-FLAC 與 A 手動載入終態設計 v0.15／v0.16 | 要求明確 4 KiB 鍵分隔與大檔分流、舊 IPC 取代的回執、首次恢復播放等。已在 v0.17 修訂。 | review-mwxvyvlj-ad101202 |
| C2d-FLAC 與 A 手動載入終態設計 v0.17 | 兩範圍的 Spec、Code 可行性、Security 無阻擋，可進實作。原生 FLAC 限範圍掃描。 | review-mwxw45cx-d274a7b9 |
| A 手動載入回執第一版 | 84 項 Rust 測試通過。獨立審查要求淡出後失敗不重播、實際 token 與結果的原子發布、有界錯誤和較新意圖。由 v0.17 補設計，分階段修正實作。 | Claude CLI 610e89f3-9859-45a6-b686-4c7c9e52215 |
| A2b ReplayGain Seek | Claude 複審指出 fallback 無舊 metadata 時用假的 0.0，可能靜音。已改以實際安裝後的新軌。Headless WAV 測試與其餘 84 項測試通過，送實作複審。 | Claude CLI bc070190-31c9-4b11-bae8-f6d1a75d8239、AGY review-mwxw6lxd-3fc99d95 |
| C2e 播放探測欄位 | Spec 與 Security 通過，Code 有條件通過，限輸出欄位。隔離快照 84 項 Rust 測試通過，正常 WAV/WMA CLI 投影與原對應欄位一致。stdout 有界通道與 Child 回收仍待整合。 | Claude CLI f9006ed2-cee3-4651-afe7-646cd1ad59b1 |
| B1 記憶體交界第二版 | 90 項 Rust 測試通過。輪詢／等待仍有跨執行緒的舊世代寫入，abort 未接三個正式指令，故未通過交付。 | review-mwxw2pp6-ca5121d1 |

### 2026-11-27，電台、掃描及手動載入回執

| 審查對象 | 判定與證據 |
| --- | --- |
| ReplayGain 後端與短檔 Symphonia 預載 | AGY review-mwyci5i7-962cf47c 通過。組合快照 91 項 Rust 測試通過，包含短檔預算超限時將同一 WAV handle 重設至檔頭。 |
| FLAC 原生掃描及依賴鎖定 | AGY review-mwyd3bx8-33937f9b、review-mwyd9rwv-7b049b35、review-mwydhkvf-fe474509 通過。Lofty 使用本地 0.9.0 補丁。原生合成 FLAC 由掃描服務入庫，快照 92 項 Rust 測試通過。 |
| 保留區網電台及管理程序 | 設計 v0.20 經 AGY review-mwyc96vc-e45f65f1 通過。實作的取消、自然結束與兩支子程序回收經 review-mwydp0px-5da8317e 複審關閉原阻擋。兩項新增測試經 review-mwydrike-11a47e33 放行，HTTP 空回應及失敗測試經 review-mwye47l1-71f3006b 放行。Windows 六項實測通過，含 0／5／6 跳、換台、滿通道退出及兩個 Windows handle 確認子程序結束。快速連續換台、啟動第二程序失敗時首程序 handle 和實機聆聽待驗證。 |
| 手動載入回執與播放事件 | AGY review-mwyeiy4j-41dbf270 複審關閉五項阻擋。104 項 Rust 測試通過，涵蓋第一次 Play、同歌重播、Seek 不增加播放事件和失敗清空快照。完整 #7 淡出與 B 仍獨立驗收。 |
| 前端快照、手動意圖與 ReplayGain 顯示 | AGY review-mwye5o4s-6113ebad 與 review-mwyedohw-d6e8af4b 放行。TypeScript 檢查通過。需搭配同代後端快照與回執。 |


### 2026-11-27，組合驗證與續修

- 前端交付版尾隨空白調整經 AGY review-mwyekvsw-4b45abb2 通過，與手動載入回執同批整合。組合工作樹通過 5 項 Node 測試、TypeScript、Vite 建置及 105 項 Rust 測試。
- 播放用 ffprobe 最小欄位與有界 stdout 經 AGY review-mwyeifx8-8a80ceb4 及 review-mwyen03p-25f46288 通過。合成 WAV 實際比較掃描與播放探測欄位，兩者對應值一致。已驗證 EOF、精確上限、超限與讀取失敗。配額極端回收故障需繼續測試。
- B1 記憶體轉場 111 項 Rust 測試通過，AGY review-mwyejiu7-40000b83 確認跨世代狀態覆蓋已處理。Stop 完成前的時序、串流來源與安裝仍有依賴，B1 候選保留在隔離驗證目錄。


### 2026-11-27，終端機停止後的工作樹核對

- 已整合的暫停、繼續、停止與 Seek 淡出控制經 AGY review-mwyf59f1-45d05f0c 複審關閉三項阻擋。四行空白經 review-mwyf8vu9-80b3edf6 確認可併入。工作樹 player.rs 與通過版本 SHA-256 同為 31A8342152F767773E9FCDE0139830D1DD710850FF8A8ABADF887AAA362438B0。106 項 Rust 通過屬淡出隔離候選。手動換軌與長檔首靜音仍待驗證。
- 滿通道測試在 Windows 等到最新合成 WAV 準備成功，並核對歌曲與 token，經 AGY review-mwyeyhlh-58502d20 放行。記憶體交界的重取樣候選初版曾通過 111 項 Rust 測試，但加入實際尾音斷言後，最新候選為 111 通過、1 失敗、6 略過。44.1 kHz 案例已滿足 9,600 交錯樣本，失敗點是最後 100 個交錯樣本無預期尾音。依鎖定版 Rubato SincFixedIn 的輸入擺放、初始索引與 sinc 核中心推導，額外丟棄 139 個輸出幀會將最後觀察窗起點從原生第 4,364 幀移至約第 4,492 幀，超過本檔 4,410 幀。下一版應保留有界尾端與 unread 游標修正，先用同一尾音斷言比較有無額外裁切群延遲，再以普通升取樣及降取樣重跑。這是源碼推導，仍需實跑和 AGY 複審，候選保留隔離。
- FFmpeg PCM 回收候選的預設套件測試為 105 通過、8 略過。額外合成 WAV 真工具測試在 100 ms Seek 回報 ChildFailed。對應診斷檔尾有 Immediate exit requested，但目前缺少該筆診斷、超額試次及 Seek 試次的個別程序與退出階段對應。應先讓每次測試可區分收集錯誤、子程序退出狀態與讀取工作結束，再複審回收與名額。候選未合併。
- 預載凍結候選有 107 項 Rust 通過，含保留原世代、待播曲目與 token 的測試。尚待獨立審查與前端的預載終態回執整合。
- MP4 短 PCM 候選通過 106 項 Rust。AGY review-mwyfkwll-ae44d595 確認配置前樣本上限與短 PCM 的型別化拒絕。長串流終態仍待接線，候選未併入。
- 六項 Windows 電台測試通過的 radio_tests.rs 與隔離版 SHA-256 同為 718C9F7BC38C528411ACBC4428E94E89AF8A13A562219D0B5C6BF3109150F22E。既有 Windows 封裝 SHA-256 對應先前檢查點，完整修復及新版執行檔驗證仍待完成。

### 尾音與 Seek 的無程序驗證

以鎖定的 Rubato 0.16.2 源碼重建 SincFixedIn 的 2,048 幀區塊、256 點 sinc、256 相位、Blackman-Harris 平方窗、線性插值與補零流程。模型以 JavaScript 數值運算，未執行 Rust 測試。輸入為 100 ms 合成 PCM，最後約 5 ms 的振幅為 0.5。既有 Rust 斷言為 9,600 個立體聲交錯樣本，最後 100 個樣本須有值大於 0.2。

| 輸入取樣率 | 另外捨棄的輸出幀 | 模型輸出交錯樣本數 | 尾端 50 幀最大值 | 尾端 50 幀超過 0.2 的幀數 |
| --- | ---: | ---: | ---: | ---: |
| 44100 Hz | 139 | 9600 | 0.000005 | 0 |
| 44100 Hz | 0 | 9600 | 0.525145 | 50 |
| 96000 Hz | 64 | 9600 | 0.002145 | 0 |
| 96000 Hz | 0 | 9600 | 0.539877 | 50 |

下次實作應保留有界收尾、輸入範圍及未讀游標修正，先移除一般區塊及尾端對 SincFixedIn 輸出的額外裁切。Rust 驗收必須保留尾音斷言，涵蓋 44.1 kHz 升取樣及 96 kHz 降取樣，再交獨立複審。JavaScript 模型的通過結果不替代 Rust 驗證。

FFmpeg 的合成 WAV 為三秒 44.1 kHz 雙聲道。檔頭給出完整收集 264,600 個交錯樣本及 100 ms Seek 後 255,780 個交錯樣本的預期值。相同測試在回收候選之前曾多次通過。最新失敗試次總耗時約 2.05 秒，Seek 路徑允許空輸出，因此可排除十秒收尾期限、四十秒回收移交及起點為零時的空輸出拒絕。下一次執行需用試次、子程序識別與階段區分非零退出、工作執行緒失敗、結果通道中斷與收尾狀態；不從共享 stderr 直接推定 FFmpeg 已被哪個試次取消。


### 前端型別與預載回執核對

- 直接使用工作樹的 TypeScript 5.6.3 編譯器 API 載入專案設定，19 個前端來源檔零診斷。現有五個斷言檔全部通過，涵蓋關窗、EQ、音效、換曲選擇、開檔還原。這次核對未啟動 CLI，也未改動產品程式碼。
- Design 要求由引擎快照回傳帶 token 的 `nextRequestResult`，區分 `accepted`、`ready`、`rejected`、`failed`。現有前端在 `await playerSetNext(...)` 返回後記下請求，並以後續有效播放的 `nowToken` 配對佇列。這五個前端測試未覆蓋預載終態及忙碌後重送，仍須接通回執後實測與送獨立審查。


### FFmpeg PCM 工作回報與回收

- Windows 真實 WAV 用例依序驗證完整解碼、恰好達上限、超出上限及 100 ms Seek。新回收候選前三步未觸發斷言失敗，第四步在 `player.rs:2201` 收到 `ChildFailed`。該用例在完整測試輸出列為 ignored，須獨立執行；舊 v2 候選的成功結果僅適用於舊版本。
- 新候選把子程序非零退出、stdout／stderr 執行緒啟動或 join 失敗，以及收集工作未回報終態都歸為 `ChildFailed`。下次實作先以固定欄位區分這些結果，並在該測試以第幾次解碼關聯紀錄。退出碼及分流標記即可定位，無須記錄原始路徑、網址或 FFmpeg 參數。


### MP4 封包補丁的來源與發行查核

- 隔離候選鎖定 `symphonia-format-isomp4` v0.5.5。registry 壓縮包 SHA-256 為 `243739585d11f81daf8dac8d9f3d18cc7898f6c09a259675fc364b382c30e0a5`，與 Design 及整合工作樹的 registry 鎖檔一致。候選與 registry 共有的 46 個檔案中僅 `src/demuxer.rs` 內容不同。該檔 SHA-256 為 `7aa7333df614d3ce4a316a3869e1225118bb0e92ff0e69eda0bcf5cb4be82917`，與套件邊界測試的既有雜湊證據及 AGY 審查副本相同。
- 候選 `Cargo.toml` 宣告 MPL-2.0，`src/demuxer.rs` 保留原始授權標頭。registry 壓縮包未帶專案根目錄的授權全文，已定位 [鎖定提交的上游 LICENSE](https://raw.githubusercontent.com/pdeljanov/Symphonia/6d533f26150953a882a6a111ebd13f0abf7129d5/LICENSE)。目前候選沒有 Design 要求的 `UPSTREAM.md`，也沒有上游 LICENSE 副本。待完成來源紀錄及發行告知，與新版 Windows 產物一併核對。
- 套件兩個既有測試覆蓋尺寸上限邊界，隔離 App 紀錄為 106 通過、7 略過。AGY 審查涵蓋開啟與短 PCM 路徑；目前整合工作樹仍使用 registry 版，故不能以此紀錄證明長串流終態或實際發行版。


### 長串流的錯誤終態與撤銷

- MP4 候選 `audio_decode.rs:627–647` 記下 `packet_budget_rejected`，`audio_decode.rs:698–704` 把串流停止折成 `None`。手動播放長檔在 `player.rs:873–876` 將來源轉為 `Box<dyn Iterator<Item = f32>>`，渲染管線在 `playback_pipeline.rs:754–757` 只看樣本是否存在。這條路徑沒有傳遞超限原因。自動預載掃描開頭靜音的 `player.rs:1672–1685` 也沒有在取樣後檢查超限旗標，會繼續嘗試重開。
- 整合工作樹的 `player.rs:1543–1550` 清的是準備協調器世代與前端清除 epoch。`player.rs:1729–1731` 的開流工作完成後才讀取 `stream_gen`，再由 `playback_pipeline.rs:584–593` 寫入來源與安裝通知。已開始的開流工作若跨越清除或換代，完成時可能使用現行世代。四個現有整合、交接、MP4 與預載候選均保留完成時讀取世代的路徑。
- 後續實作接上 Design 的有界來源結果，保留出發時的來源世代與預載 token。渲染端收到型別化失敗時讓命令執行緒結案，待播開流失敗時回覆原 token。一般合成來源可提供樣本後回傳型別化失敗，用來比對正常 EOF。另以可控的開流回傳驗證「送出、清除或換代、舊回傳」順序，預期舊結果不能佔用槽位或發出就緒回執。


### 無視窗 CLI 續做與實機回歸

- OpenCode CLI 使用本機 Qwen3.5-4B-BF16 產生 TrackResampler 修正，候選 SHA-256 為 `c4fcc2a97eb60885ce7af98d3e2fcad1bf38e60fe941a059717ed88445936df9`。透過 Windows Job Object 執行的 44.1／96 kHz 尾音測試通過，候選完整 Rust 測試為 112 通過、6 略過。
- AGY 獨立審查 `bda3f4ed-fea3-40a7-a06c-42a740baac02` 對移除重複延遲修剪的差異判定 Spec、Code、Security 通過。審查明列原有 `output_pos` 與完整 `flush_tail` 為範圍外，後續整合須送完整差異。
- 同一份 FFmpeg 回收候選未修改程式，以已定位的本機 FFmpeg 跑 Windows 真實 WAV 上限與 100 ms Seek，用預設略過測試的精確名稱直接執行，首次及後續三次均通過。舊測試的失敗原因仍未證實。候選仍需獨立審查回收、上限與後備路徑後才能整合。

- 實際整合差異（完整 `flush_tail` 與掃描格式常數）由 AGY `120056b5-b049-4f3b-9a57-a070f4d8f445` 複審，Spec、Code、Security 通過。整合候選原生測試為 107 通過、7 略過，其中 44.1／96 kHz 尾音通過。已將通過審查與測試的兩個檔案原樣併入 `SL-fixes`，重新比對 SHA-256 相同。跨軌交接仍待完成。

- 最新整合底稿加入 FFmpeg 短檔上限、原始 Seek 座標及所有權回收。隔離整合候選原生測試為 107 通過、8 略過，另直接執行略過的真實 WAV 用例通過。AGY `d0f2a48d-bf27-40af-a247-28f584d9c773` 的 Spec、Code、Security 均通過，已依送審雜湊套用到 `SL-fixes`。此輪回歸測試未覆蓋異常回收時的高併發，也未核定共用來源工作者。


### 待播回執快照

- OpenCode 本機模型新增 `nextRequestResult` 與 `nextCapacityEpoch`。Rust 真實快照測試通過，確認 camelCase 鍵名、ready 狀態、token 及舊快照缺欄位時的預設值。
- AGY `9d31e023-b12d-4ca4-bc55-315468b148dc` 通過此序列化差異的 Spec、Code、Security 審查。候選檔案 SHA-256 為 `6512d85882c4c94b69da1c95c8ed47d656ab937fb7ed3a32b95b8e2507cd93f0`。仍在隔離候選，待連接引擎發佈、長串流就緒與前端重試後一併驗證。

- 待播協調器的受理、忙碌拒絕、容量釋出及舊世代／token 保護已通過 AGY `a2b985c6-280c-47b2-9cb8-f6d39b19c442` 的三階段審查，Windows Rust 109 通過、8 略過。回歸包括填滿完成通道後的真實 WAV、忙碌時保留前一個世代與曲目。受理快照與開流／前端尚在接線，整批保留在隔離候選。

- 待播快照已在隔離候選接上協調器。忙碌拒絕保留引擎現行世代與 `nextToken`，快照帶回最新回執及容量 epoch。AGY `90263b4b-c940-47cd-b47f-4688bfda89f4` 通過 Spec、Code、Security，Windows Rust 109 通過、8 略過，三個候選檔案的 SHA-256 與送審版本相同。就緒／失敗訊號與前端重試未完成，因此本輪未併入整合工作樹。


### 待播首幀與晚到開流結果

- OpenCode 本機 Gemma 在隔離候選 `.prefetch-first-frame-mwyi8dn6` 接上首幀檢查、背景工作開始時擷取世代，以及舊待播請求完成後的交付拒絕。記憶體準備可回 ready，開流與準備失敗使用原 token 回 failed。
- Windows Rust 116 通過、8 略過。新增 6 個首幀測試與「清除後才交付」測試。AGY `5cf8d3dd-fef5-411d-803e-4319eb8d1b02` 對此差異的 Spec、Code、Security 均通過。兩個候選檔案雜湊與送審版本一致。
- 實際長串流安裝、已發布後撤銷、非阻塞取樣及就緒後型別化拒絕仍待接線，所以尚未發出長串流 ready，也未併入整合工作樹。


### 曲終交接、撤銷與串流緩衝候選

- `.handoff-integrated-mwyiczes` 整合記憶體曲終交接與命令撤銷。Windows 全套 debug 測試為 124 通過、8 略過，包含 100 ms 的 9,600 個靜音樣本、零停頓、暫停保留停頓、雙軌 EQ／不同樣本、已發布後撤銷，以及複審要求恢復的忙碌世代測試。
- AGY `d4e02a10-743e-4ab6-a675-87033d5033bd` 確認凍結測試已恢復，仍阻擋播放進度及交界發布。這批沒有併入 `SL-fixes`。最新送審的播放管線 SHA-256 為 `9013cf70905ed0251d6a345b206aca292dd30e7e0bb160f89744af0acc2a7a40`。
- OpenCode 直接寫入 `prefetch_stream.rs`，SHA-256 為 `95b09d8d5838fa27aa704640c1a8d2a02a9245f112247da680924414969cb41a`，來源內容與 AGY `8b3dc88e-4741-458a-ab66-142eebee0eef` 獨立預審版本相同。以原生 Windows Rust 單獨編譯該模組，7 個測試全過，包括待資料／EOF／斷線的區分，以及封包拒絕在安裝前與安裝後的不同終態。模組尚未接到實際播放器。

### 換軌快照與 Symphonia 錯誤分類

- AGY `58f01b7f-2664-4cb8-9775-35a5bf97913c` 指出預先安裝下一軌可能讓快照停留在交接狀態，且曲目更新回呼在播放狀態鎖內執行。OpenCode 將發布保護延後到新來源正式提交，並移出回呼。AGY `62b920e1-d3f2-473a-8379-be874c57a90b` 通過 Spec、Code、Security。隔離候選的 Windows Rust 測試通過 126 項，8 項略過；包含暫停在交界時維持已提交身分、進度及開頭靜音位移。
- 串流模組加入完整聲道幀讀取，正式納入 Rust 測試套件。Windows 通過 134 項，8 項略過；緩衝模組的 8 項測試包含不完整雙聲道幀。AGY `b2a41dcb-f3f6-4420-941e-084c0e2de0bd` 通過三項審查。此階段完成模組註冊，播放器的長串流安裝仍待接線。
- AGY `06777c4d-37df-4136-a0e5-93e8cbc41483` 通過型別化讀取終態，AGY `9ae11100-c6d6-4c9c-8155-f980fb0c0aae` 通過開檔、首包及 Seek 分類。兩輪各有 Windows Rust 134 項通過、8 項略過。以正常 WAV、FLAC 及測試替身驗證正常曲終、累計第 11 次解碼失敗與同來源 Seek 保留錯誤。
- 短檔直接載入和預載已接上受檢查的解碼結果。預檢及終態失敗阻止 FFmpeg 後備，已解出的部分 PCM 在讀取失敗時捨棄。AGY `b84e9f9e-7a4b-4513-bc84-fbe7372af155` 通過三項審查。Windows Rust 135 項通過、8 項略過，並確認 PCM 恰好到上限時可正常結束，讀取錯誤仍回報失敗。上述程式碼保留在隔離候選，待長串流實際播放、前端回執、格式防護與 Windows Release 驗證。

### 直接長串流、MP4 防護與工作樹整合

- 直接 Symphonia 載入超過 PCM 上限時，以同一已開啟的 File handle 及相同 Seek 位置改走串流。AGY `bf780c17-c8a2-435a-926d-4047b39a2d24` 通過三項審查，Windows Rust 136 項通過、8 項略過。
- AGY `4e8d8fe9-9d54-4448-849c-06f864a14159` 通過緩衝取樣及錯誤通知。Windows Rust 137 項通過、8 項略過，涵蓋左右聲道靜音成對、進度維持及型別化終態。AGY `e05572aa-1c0b-4b05-98f2-55ddbc5f51a4` 通過直接本機長串流背景解碼及回收。正常 100 ms WAV 經背景解碼完整輸出 8,820 個樣本。AGY `4dc05246-4ce4-4ed2-b1d0-43ef0154a780` 通過來源配額保留到最後一個持有者釋放的修正。
- 本地 `symphonia-format-isomp4 0.5.5` 在樣本緩衝配置前實施 256 KiB 上限，Cargo 鎖定依賴指向本地套件。AGY `8963bbda-9d78-4018-8513-8a28f6cfa5ef` 通過三項審查。套件 2 個單元測試均通過，一般與 fragmented MP4 的 440 Hz AAC 合成素材皆通過實際解碼。保留上游 MPL-2.0 原文與來源校驗碼。
- 將 8 個已複審後端檔案及 48 個套件檔案依原雜湊合入 `SL-fixes`。在完整工作樹以 Windows Cargo `--offline --locked` 執行，139 項通過、9 項預設略過，另外執行其中的 FFmpeg PCM 與 MP4 相容性測試，兩項各自通過。前端 5 個測試檔通過，TypeScript 型別檢查通過。AGY `4905e9ae-76a9-404d-afaa-a51de1590a7a` 通過整合複審。待播長串流交接、前端回執、其餘格式限制與 Windows Release 仍待驗證。

### 明確無待播與前端回執

- 明確無待播適用於已確認的播放序號。音訊尾端排空後轉為 Finished，同軌重定位會保留這個設定，下一個播放序號會使舊設定失效。設定與清除待播可攜帶期望播放序號，既有呼叫省略資料仍可使用。AGY `67efe8e2-4ef7-43c8-ab83-ce39d8a00d00` 通過三項審查。Windows Rust 140 項通過、9 項預設略過。
- 前端以已確認播放序號、佇列、歌單、模式及結束設定重新判斷待播，記錄容量世代並核對回執 token。忙碌後僅在容量前進時重試。前端保留已受理的待播曲，實際播出時才從佇列移除，並在同一情境略過終態失敗的候選。AGY `d94223df-5d8b-4b12-9b24-a7bcdd0e5a30` 通過三項審查。前端 5 個測試檔、TypeScript 型別檢查與 Vite 正式建置均通過。仍待驗證自動長串流交接的實際播放與 Windows Release。

### Symphonia 長串流自動交接與背景回收

- 自動長串流在換曲前建立原生聲道緩衝、重取樣器及速率處理器。AGY `5203d5b0-087c-4e20-8fdb-8a07f5d25b77`、`3a6e2fe3-a664-4c78-b3b6-7bbc5e924817`、`84c01957-f7ba-45e1-9218-ba7721137961` 分輪通過 Spec、Code、Security。實際播放核心的合成 WAV 測試驗證換曲前後樣本的振幅正負不同、播放序號只增一次，以及 0／100 ms 換曲間隔。100 ms 在 48 kHz 雙聲道輸出 9,600 個靜音樣本。
- 音訊回呼把舊來源與處理狀態移交有界回收佇列，背景工作才釋放。待播來源失敗僅撤銷同世代、同 token 候選。AGY `99065646-09b5-44ce-b74e-2926eb1e7d45` 通過三項審查。將三個已複審檔案依 SHA-256 合入 `SL-fixes`，Windows 鎖定 Rust 測試 142 項通過、9 項預設略過。此次整合涵蓋 Symphonia，FFmpeg 長串流自動交接及 Windows Release 仍待驗證。
- 短 WAV 預載於裁掉靜音前計算原檔總長，保留頭尾原生幀。合成音訊回歸確認原長 100 ms、前 441 幀、尾 882 幀及保留聲道內容。AGY `96c6ce5e-3a9c-468e-be76-7278ede8cc65` 通過三項審查。整合工作樹 Windows 鎖定 Rust 測試 143 項通過、9 項預設略過。

### FFmpeg 自動交接、待播撤銷與空讀逾時

- AGY `10093d03-53c1-4c14-84f4-06a8a7bf8ba4` 通過短 PCM 共用啟動程序的重構。原本的 PCM 精確上限及非零 Seek 以實際 FFmpeg 驗證通過。
- 將 FFmpeg 長曲及短 PCM 超限後的長串流接上背景緩衝及自然換曲。AGY `96a7ba9d-486f-4f59-a82e-5ba9226cc5cf` 通過串流傳輸及回收審查，`d0ad8649-fa72-4f00-906c-b8e81b3ecf19` 指出準備中的候選尚缺取消訊號。後續加入按世代撤銷、在首批緩衝前後分開取消、既有取樣率上限，以及在播連續空讀十秒的型別化終態及非阻塞回報。AGY `a4479a74-6aee-457f-80b2-d4a83e78b268` 與 `dc805b43-6148-41e0-a40e-1d6bbbaa816f` 通過三項獨立複審。
- 按複審 SHA-256 合入三個檔案。Windows 重新編譯執行鎖定 Rust 測試，147 項通過、10 項預設略過。另執行合成 WAV 以真實 ffprobe／FFmpeg 驗證 PCM 超限後的 0／100 ms 自動換曲、解碼器識別及新曲聲音，並分開執行短 PCM 上限與非零 Seek，兩項各自通過。短 PCM 收集取消、手動載入及 Seek 背景化、長檔去除開頭靜音、其餘格式防護與 Windows Release 仍待後續驗證。

- 短 PCM 收集與程序收尾可接收待播取消，部分音訊不對外交付，回收完成才釋放配額。AGY `4b0cd72f-5567-4f9f-908d-8350b4fc311f` 通過 Spec、Code、Security。合入後 Windows 鎖定 Rust 測試 149 項通過、10 項預設略過。實際 FFmpeg 自動換曲、精確 PCM 上限與非零 Seek 各自通過。

### 直接 FFmpeg 串流與本機來源準備

- 直接 FFmpeg 的長檔與 PCM 超限路徑共用有界來源、型別化終態及程序回收。AGY `1ba7b07f-2e2d-45ab-a0c3-34b7de7ff45d` 通過 Spec、Code、Security。整合後 Windows 149 項通過、11 項預設略過，另有 3 項實際 FFmpeg 測試各自通過。新增測試強制在原檔 100 ms 轉串流，驗證直到正常曲終為止的樣本完全等於同起點短 PCM。
- 將本機載入的狀態準備、來源解碼及安裝拆成獨立函式，原呼叫順序與 PlaySeq 維持一致。AGY `7cbe430d-a45f-4c27-a8b5-824e28aaff00` 通過 Spec、Code、Security。候選 Windows 鎖定 Rust 149 項及 3 項實際 FFmpeg 測試通過，依複審 SHA-256 合入。背景化命令排程另行驗證。

### 手動背景載入、原檔起點及探測取消

- Load、Prepare、Seek 與淡出後 SwitchLoad 在背景準備。新意圖撤銷上一代，停止及電台切換會捨棄待辦。準備中的 Play、Pause、Toggle 只更新完成時的播放意圖。Seek 保留來源身分及 PlaySeq。候選的調度由 AGY `4879d74d-5339-48ce-8117-cc61d5ac9087` 通過審查，來源工作審查 `79356133-e992-44e2-918c-91f5948404e8` 要求補齊探測取消與錯誤分類。後續 `e9979c58-ebb9-411b-8fdc-d37612e6fffa` 及 `ec44bf94-5f1e-4aa6-8ab2-c82735e12d68` 提出取消等待者的喚醒與手動配額競態。
- 最新手動請求在有來源槽位才開始準備，等待期間仍可接受控制命令。整首收集、超限轉串流及背景回收共用該次預留，避免前一個子程序仍在回收時誤報最新請求解碼失敗。ffprobe 等槽、讀 stdout 或等子程序退出期間可撤銷。釋出槽位時喚醒全部等待者，取消後仍由背景持有槽位至回收完成。初次裁掉開頭靜音以原生幀換算一次起點，手動 Seek 0 回到原檔位置。AGY `82f70857-6e01-413d-ba8a-8a714e5e630a`、`2d0604ae-8a73-42c7-80af-fcd079e16ea1`、`5c06355c-a003-4683-a818-1e5ffacb1043` 通過獨立複審。
- 合入後 Windows 鎖定 Rust 測試 157 項通過、11 項預設略過，另執行真正的 FFmpeg 待播、短 PCM 與 ffprobe 正常欄位比對，各自通過。針對直接 FFmpeg 超限後從 100 ms 開始的實際樣本比對，在相同 SHA 的候選執行通過。跨待播及已安裝來源的統一配額、短檔絕對尾端、長檔靜音及其餘格式防護仍待後續驗證。

### 短檔絕對尾端與原生格式封包上限

- 短檔 Seek 沿用首次裁切的原檔絕對尾端。到達或越過尾端時同一來源進入正常曲終，暫停時維持暫停。AGY `c7c48fe6-3378-4c92-8f8d-a4f20b2006ed` 通過獨立複審。合入後另以 100 ms 合成 WAV 驗證 Seek 0、80、90、95 ms 與原檔長度。
- Ogg、原生 FLAC 及 WAV／AIFF 在封包配置前實施 256 KiB 上限。AGY `8745aea0-6d0e-4543-8d2c-3c26e803274a` 要求核對跨頁 Ogg FLAC 中繼資料，補上合法音檔相容性測試後由 `532b5d43-658d-4350-aa6d-7180d79d2259` 獨立複審通過。原生 FLAC 與 WAV／AIFF 由 `4a13060a-f903-4b7f-883d-54f0fc429778` 和 `756f2131-90b2-4048-8b5f-d47e93becec1` 獨立審查通過。
- 按清冊及複審 SHA-256 合入。Windows 鎖定 Rust 測試 158 項通過、12 項預設略過。另執行 FFmpeg 自動換曲、短 PCM 精確上限及非零 Seek，以及 Symphonia 實際解碼合法 WAV、AIFF、FLAC、Ogg Vorbis 和 Ogg FLAC，三項各自通過。三個原生格式套件的封包上限單元測試皆通過，使用應用程式鎖定依賴。媒體庫掃描的其餘中繼資料上限與 Windows Release 仍待驗證。

### 長曲開頭靜音裁切

- Symphonia 與 FFmpeg 在初次播放時，以完整的原生聲道幀判斷首個聲音，受 60 秒音訊、50 MiB PCM 及 10 秒期限限制。保留首個有聲幀和原檔時長。全靜音與一般探測失敗回到原檔起點，型別化封包或解碼次數超限直接終止。明確 Seek 沿用原檔位置。Symphonia 回退沿用同一已驗證 File handle。FFmpeg 回退先回收程序與管道，再以同一配額開啟來源。
- AGY `753ef86b-008f-46c8-b7ee-cf6f88f37901` 與 `e2e1433e-009e-4545-b135-d219274f154f` 分別通過 Symphonia 與 FFmpeg 的 Spec、Code、Security 獨立複審。依複審內容與 SHA-256 合入後，Windows 鎖定 Rust 測試 160 項通過、13 項預設略過。另獨立執行三項實際 FFmpeg 測試通過。新增項目使用 44.1 kHz 雙聲道合成 WAV，驗證裁切、有聲首幀、全靜音回退、關閉裁切及非零 Seek。

### 串流換入記憶體軌的回收

- 自然交接到記憶體曲目時，把舊串流與舊軌資料交給有界背景回收。回收滿載時保留待播候選。AGY `d215db15-dcf0-4fea-8a3f-acae4a80c1df` 通過 Spec、Code、Security 獨立複審。合入後 Windows 鎖定 Rust 測試 161 項通過、13 項預設略過。合成測試確認新曲取樣與 token 正常生效，舊來源配額在背景消費回收項目後才釋放。

### 共用來源配額與待播完成通知

- 目前軌、待播準備和背景回收共用最多三份配額，來源在受理前取得配額。記憶體曲目安裝後仍持有來源配額，短 PCM 超限轉串流沿用同一份。實際釋放時遞增容量世代，供前端重試。
- AGY `fef67ab4-c782-476a-be58-0b5c386e857b` 指出替換或清除待播時，舊完成結果會滯留並造成誤報忙碌。已使完成通知與取消共用互斥區間，在替換及清除時移除舊的 ready、pending、completion。上代工作或子程序尚在回收時持續占用配額。AGY `334953e0-4cf9-40d2-83b6-9325390ed485` 通過 Spec、Code、Security 複審。
- 按複審 SHA-256 合入。Windows 鎖定 Rust 測試 163 項通過、13 項預設略過。容量回歸覆蓋占滿、釋放後重試、替換或清除舊成品，以及新來源安裝與舊來源背景退場。另執行三項實際 FFmpeg 測試，各自通過。

### 播放器忽略描述性標籤

- Symphonia 播放 MP4 時略過頂層及電影層的描述性標籤。一般與 fragmented AAC 均實際解出音訊，媒體庫入口仍讀得到同一合成檔的標題。外部 ID3v2 只讀固定標頭與適用的 footer，從同一已驗證的檔案按宣告範圍跳過。另以正常 ID3v2.3 與 ID3v2.4 MP3 驗證音訊。
- AGY `8e385ec8-b679-468a-b4c3-3a15a4571b3d` 與 `935426e2-cd4d-457c-88c8-567a63821de8` 通過獨立審查。依已審差異與 SHA-256 合入，兩項實際音檔測試均在 `SL-fixes` 通過。

### Ogg、原生 APE 與 WavPack 媒體庫掃描

- Vorbis、Opus 按頁面與原始封包邊界讀取中繼資料。單欄維持 4 KiB、同檔合計 16 KiB。跨頁封面與超長文字按已驗證範圍略過。AGY `5a74fbba-4c20-49d3-8554-64f134c53a73` 通過獨立複審。合入後以 270 KiB 合法註解驗證 Vorbis、Opus 可正常入庫，保留標題、ReplayGain 與音訊屬性。
- 原生 APE 與 WavPack 以相同 handle 核對 APE 標籤邊界、文字限額與 footer。AGY `2379c70e-f686-429b-8d94-163ce632a0f0` 通過獨立複審。合入後 Windows 鎖定 Rust 測試 163 項通過、16 項預設略過。另以正常 WavPack 驗證保留短文字、略過 8 KiB 註解及維持取樣率、聲道、時長。此批涵蓋原生 APE，帶 ID3v2 前綴的 APE 於後續批次完成。
- 前端 5 個測試檔與 TypeScript 型別檢查在 `SL-fixes` 重跑通過。

### ID3、WAV／AIFF、MP4 與前綴格式掃描

- MP3 的 ID3v2 逐欄讀取，解同步與解壓縮均受預算限制，並與 APE 文字共用總上限。WAV／AIFF 同樣限制封裝 ID3 及容器文字欄位。AGY 分別在 `0851532b-5087-43d3-8c2d-2ff7f7cf44c9`、`a8c30ef4-63bc-4868-99a2-060b894d88ba` 通過 Spec、Code、Security。
- MP4 媒體庫掃描先查 atom 與文字範圍再配置，略過封面，保留音訊屬性、合法短欄位和曲目／碟片號。AGY 審查為 `13ff948d-8132-4dfc-bf34-46f43450aa23`、`7ff9bced-15c9-4616-a736-224d2d12780c` 及 `196a60da-27e8-4754-ad35-fcf7a574568a`。
- 帶 ID3v2 前綴的 ADTS AAC、FLAC、APE 接通同 handle 的受限掃描。AGY `d0486a19-23da-4bb1-b34b-7c8c4580aff2` 通過獨立審查。實際合成 AAC、FLAC 驗證短標籤、取樣率、聲道與時長。
- 依複審快照和 SHA-256 合入。正常 MP3、WAV、AIFF 與含封面的 MP4／AAC 在 Windows Release 格式回歸通過。

### 待播並行、記憶體交接及原生準備逾時

- 讓待播可並行準備，受共用三份來源配額限制。取消舊請求仍待實際退出才釋放配額。記憶體曲目的 DSP 在背景備妥，交接維持同一播放事件及 EQ 狀態。AGY `05b7dd36-33a2-4c15-a1ef-1c9b13a2cfd2`、`728fd1a2-4054-498f-ae0d-7874ead91e85` 分別審查通過。
- 手動載入與待播的原生準備連續 10 秒無進展時回報逾時。有效 PCM 樣本續期。背景工作仍占用來源配額至真正退出，逾時結果不得再裝軌。FFmpeg 沿用自身超時與回收流程。AGY `3da64acd-b5be-4582-8ba7-9fad8fb2cda9` 通過 Spec、Code、Security。三個實作檔案依審查清冊及 SHA-256 合入。
- 操作及維護文件四份均經 AGY `edc727be-8b0e-459a-8a91-d20837273a13` 審查通過，交付檔案與送審快照的 SHA-256 一致。

## 3. Windows Release 驗收

- Windows Rust `--release --offline --locked` 預設測試 167 項通過、0 失敗。將同一 Release 測試執行檔的 20 項預設略過測試全部補跑，20 項通過、0 失敗。包括合成音檔經實際 FFmpeg／ffprobe 解碼與探測，以及六項以本機 HTTP fixture 驗證電台轉址、取消與子程序回收。
- 前端 `node --test src/player/*.test.mjs` 5 項通過，TypeScript 檢查通過。自動驗收使用合成音訊與本機伺服器，#6、#7 的實機聽感驗收保留在原規劃。
- x64 Release 執行檔內含新建置的 index-C20e-Y4d.js，執行檔 SHA-256 與舊產物不同。首次完整封裝遇到 WiX 下載 DNS 錯誤，以無視窗 Tauri 封裝重試取得 MSI 和 NSIS，重試前後主執行檔 SHA-256 相同。三份產物已核對產生時間、檔頭與雜湊。專案內的 Windows 版本欄位維持 0.1.1。
- 自動化驗收涵蓋 Windows Release 建置及測試、產物檔頭和雜湊。安裝與實機聽感未測。

| 產物 | SHA-256 |
| --- | --- |
| sonicledger.exe | 89159502015600E73CD6B47D99FB6664A3DCF3F9048081DABF1E05EEC0AA4CB2 |
| bundle/msi/SonicLedger_0.1.1_x64_en-US.msi | 8248D4D89A8DFC5EB1E70DC48A8F31BAAFCE7F61CE20EEB329217048CA50E2AF |
| bundle/nsis/SonicLedger_0.1.1_x64-setup.exe | EEE2293894C950FC77691F982A57849D15F5BEEE38ACD406E76D53D89B25C2E6 |
