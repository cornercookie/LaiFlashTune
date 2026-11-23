# P0 Review

審查者：Codex（OpenAI，`codex exec --sandbox read-only`，簡稱 C）。預審與 Spec 第 1 輪另有 OpenCode + Nvidia Nemotron 3 Ultra（簡稱 O）；之後 OpenCode 只負責實作。作者不審自己的文件。

狀態：**已修**｜**成立**（待改 Design.md）｜**不成立**（附核對依據）｜**已涵蓋**（Design 已有）｜**待核對**（還沒拿原始資料驗證）｜**待決定**（需要使用者選）

## Design 審查

### 預審（2026-11-22，對象是改名前的 planning.md，Spec 與 Code 混在一起）

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C#1 | 阻擋 | 搜尋「已修」不成立：`write_tags` 還在寫舊索引的 `id` 欄，改標籤會「檔案已寫入卻回報失敗」，批次也會把已寫入的歌列為失敗 | 重現成立；修正後新增 `extras::tests::write_tags_succeeds_and_search_follows`，14 項測試全過 | 已修 |
| 2 | C#11 | 阻擋 | `node src/player/*.test.mjs` 只會跑第一個檔案；cargo 沒指定工作目錄 | 實測 `node --test "src/player/*.test.mjs"` 跑完 4 個檔 | 已修（AGENTS.md、ROADMAP.md） |
| 3 | C#2 | 阻擋 | AIFF 已經能解碼（symphonia 有開 `aiff` feature），只是沒進掃描；格式表要分成播放、掃描、標籤三欄 | `Cargo.toml` 的 symphonia features 含 `aiff`、`pcm` | 已修（v2：格式矩陣分播放、中繼資料兩欄） |
| 4 | C#3 | 阻擋 | lofty 0.9 不支援 MPC；TTA、DSF/DFF、TAK 等格式沒有後備的中繼資料路由 | 本機 `lofty-0.9.0/src/` 只有 aac、ape、flac、id3、iff、mp4、mpeg、ogg、wavpack，沒有 mpc 與 asf | 已修（v2：lofty 不支援的走 ffprobe） |
| 5 | C#4 | 阻擋 | OFR 列進 ffmpeg 路由，但本機 ffmpeg 沒有 OptimFROG 解碼器 | `ffmpeg -decoders` 確實沒有 | 已修（v2：OFR 列為受阻） |
| 6 | C#5 | 阻擋 | 直接播預載的歌會繞過佇列與「播完就停」；佇列或模式改了，已排入的音源沒有撤回規則 | — | 已修（v2：核對 `Shell.tsx` 成立；改由 `decideNext` 統一決定，加上 `set_next`／`clear_next`） |
| 7 | C#12 | 重要 | `sink.len()` 是音源數量，不是換曲事件；`advanced_to` 分不出同一首循環的第幾次；P1 的曲間靜音也會讓音源數變動 | — | 已修（v2：核對 rodio `sound_count` 成立；改用每軌 `seq`） |
| 8 | C#13 | 重要 | 每首 append 都新建 `DspSource`，EQ、殘響、變速的狀態會在交界處重置，無縫不等於沒有接點 | `player.rs` 的 `append_pcm` 每次都 `DspSource::new` | 已修（v2：同格式走 `TrackChain` 共用 DSP 狀態） |
| 9 | C#14 | 重要 | 交叉淡化直接依樣本索引混合，沒處理不同取樣率或聲道；新的 ffmpeg 格式全部會變成串流，因此不能交叉淡化 | `mix_crossfade` 沒有重取樣 | 已修（v2：ffmpeg 格式不參與交叉淡化，補齊移到 P1-7） |
| 10 | C#15 | 重要 | 「跟電台一樣」回收子程序不可靠：stderr 沒有持續排空，阻塞讀 stdout 時停止旗標不會被處理 | `play_url` 只在完全沒收到音訊時才讀 stderr | 已修（v2：stdout 與 stderr 各用一條執行緒，Drop 時 kill 並 wait） |
| 11 | C#16 | 重要 | 依賴 ffprobe 卻只規劃了 ffmpeg 的查找；「把 exe 旁的 ffmpeg 改名就應該失敗」跟 bin 與 PATH 的後備互相矛盾 | — | 已修（v2：`tools::locate` 同時找 ffmpeg 與 ffprobe；測試改成注入候選位置） |
| 12 | C#31 | 重要 | 格式盤點沒有逐格式的驗收矩陣（M4B、AA3/AT3/OMA、RMI、MTM、MDZ 等沒有列） | 對照官方格式清單 | 已修（v2：逐格式矩陣，用本機 ffmpeg 核對過） |
| 13 | C#35 | 次要 | 中性化清單漏了 `PRODUCT.md:15`、`PLAN.md:128`；`git grep` 不掃未追蹤的檔案 | grep 兩處確實有 | 已修（v2：清單補上，grep 加 `--untracked`） |
| 14 | O#1 | 阻擋 | ffmpeg 帶 `-ar -ac` 會強制重取樣；DSD64/128/256 要對應不同的 PCM 取樣率 | Design 是用 ffprobe 讀到的原生值帶入 `-ar -ac`，所以不會重取樣；DSD 只寫了 88.2k，這點成立 | 已修（v2：DSD 取樣率對照） |
| 15 | O#4 | 重要 | rodio `Sink::append` 取樣率不同時會 panic | `append` 裡確實沒有 assert，「會 panic」不成立。**更正**：原本引用的 `queue.rs` 測試其實是停用的（`#[ignore] // FIXME: samples rate and channel not updated immediately after transition`），交界處的格式切換並不可靠（Design Code 審查 C1#4 指出） | 不成立（panic）；交界格式問題成立，v6 改為不使用 rodio 佇列切換格式 |
| 16 | O#10、#30 | 重要 | 程式與文件裡還有參考播放器的名稱 | 這就是 P0-0 要做的工作本身，不是設計缺陷 | 不成立 |
| 17 | O#19 | 次要 | `switch_output` 沒有處理已排入的下一首 | Design P0-3 的完成條件已要求切換裝置時清掉並重新排入 | 已涵蓋 |
| 18 | O#26 | 次要 | 搜尋的現況描述過時 | Design 先寫原因再寫修法，描述正確 | 不成立 |
| 19 | O#34 | 次要 | 沒有 ffmpeg 時應直接回傳自訂錯誤，不先試 symphonia | Design P0-2 第 6 點已規定 | 已涵蓋 |
| 20 | O#40 | 次要 | `.impeccable/` 不應納入檢查 | `.gitignore:30` 已排除 `.impeccable/`，不會被推上去 | 已修（v2） |
| 21 | O#12 | 次要 | lofty 0.9 支援 ASF 標籤 | 本機 lofty 0.9.0 原始碼沒有 asf 模組 | 不成立 |

### Spec

#### 第 1 輪（對象 v2）

結論：Codex「不通過」（1 阻擋）、OpenCode「有條件通過」（4 阻擋，核對後 2 項不成立）。SC = Codex，SO = OpenCode。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | SC#1 | 阻擋 | 「結束後不播下一首」寫成無條件回傳 null，會連手動下一首一起擋住 | `Shell.tsx:637` 只在 `reason === 'ended'` 套用；`Agents/business-logic.md` 規定手動下一首一定換歌 | 已修（v3） |
| 2 | SC#2 | 重要 | 排入的歌解碼失敗後怎麼越過、全部失敗何時停，沒有規則 | Design v2 只寫「改排下一首」 | 已修（v3：失敗集合、移出佇列、連續失敗上限） |
| 3 | SC#3 | 重要 | 驗收只看「P0」列，漏了 AIFF 掃描與 M4B | v2 矩陣期別欄寫「AIFF 補掃描」「M4B 補」 | 已修（v3：明定本期驗收範圍） |
| 4 | SC#4 | 重要 | 外部樣本不是正弦波，不能套 1kHz 驗收 | — | 已修（v3：合成樣本與外部樣本分開判定） |
| 5 | SC#5 | 重要 | 「切歌 50 次後 ffmpeg 為 0」沒有先停止，串流中保留程序是正常的 | — | 已修（v3：具體步驟、停止後等 3 秒、只數本程序的子程序） |
| 6 | SC#6 | 重要 | 中繼資料全失敗、缺工具掃描、右鍵開啟受阻格式沒有完成條件 | — | 已修（v3） |
| 7 | SC#7 | 次要 | 時序圖把不同格式分支的換曲通知也畫成由 TrackChain 發出 | — | 已修（v3） |
| 8 | SO#1 | 次要 | 受阻、P3、P4 格式沒有驗收 | — | 已修（v3） |
| 9 | SO#2 | 阻擋 | 工具查找測試注入候選位置，跟正式行為不一致 | v2 已寫 `locate_in` 可注入，正式順序固定；只是描述不夠清楚 | 不成立（v3 補一句說明） |
| 10 | SO#3 | 阻擋 | ffmpeg 格式一律串流，永遠不能交叉淡化 | P1-7 會在 TrackChain 做串流交叉淡化，「永遠」不成立；但一律串流會失去峰值正規化，這點成立 | 部分成立，已修（v3：15 分鐘內讀進記憶體） |
| 11 | SO#4 | 阻擋 | 連續 `set_next` 時舊的排入沒有取消 | — | 已修（v3） |
| 12 | SO#5 | 重要 | 電台接進 TrackChain 會被套用變速 | 電台走 `play_url`（`player.rs:419`），不經 `set_next` | 不成立（v3 明寫電台不進 TrackChain） |
| 13 | SO#6 | 重要 | 手動切歌時隨機預選沒有作廢 | — | 已修（v3） |
| 14 | SO#7 | 重要 | `Agents/business-logic.md` 還有參考播放器名稱 | grep 沒有命中；該文件用的是「參考播放器」與行為描述 | 不成立 |
| 15 | SO#8 | 重要 | 矩陣期別不一致，P3/P4 格式可能混進掃描 | rmi 本來就在 MIDI 列；但掃描範圍確實沒寫清楚 | 部分成立，已修（v3） |
| 16 | SO#9 | 重要 | 交界測試只驗連續性，沒驗 DSP 狀態延續 | — | 已修（v3） |
| 17 | SO#10 | 重要 | ffprobe 只讀 `format.tags` | — | 已修（v3：兩邊都讀） |
| 18 | SO#11 | 次要 | 搜尋一節的狀態標示不清 | — | 已修（v3） |
| 19 | SO#12 | 次要 | 時序圖沒表示 `set_next` 是非同步 | — | 已修（v3） |
| 20 | SO#13 | 次要 | AAC 的 44 ms priming 吃掉 ±50 ms 容差 | `Agents/business-logic.md` | 已修（v3：AAC 系 +100／−50 ms） |
| 21 | SO#14 | 次要 | grep 要排除 `.impeccable/` | `.gitignore:30` 已忽略，`git grep --untracked` 預設不掃被忽略的檔 | 不成立 |
| 22 | SO#15 | 次要 | P0-3 提到 P1 的曲間停頓是範圍外洩 | 那一句是 `seq` 介面的約束（靜音段不算一首），不是要在 P0 實作 | 不成立 |

#### 第 2 輪（對象 v3）

結論：Codex「不通過」（1 阻擋、5 重要）；未推翻第 1 輪的「不成立」判定。S2 = 第 2 輪 Codex。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | S2#1 | 阻擋 | 失敗上限「取清單長度」跟佇列續播矛盾：清單只有 A、佇列 [壞 B, 好 C] 時 B 失敗一次就停 | `Shell.tsx:597` 會用單曲清單播放，佇列獨立（`Shell.tsx:873–878`） | 已修（v4：停止排入只看 `decideNext` 回 null 或連續 20 次；目前這首照常播完） |
| 2 | S2#2 | 重要 | `clear_next` 沒涵蓋背景準備中的下一首 | v3 只寫「已排入」 | 已修（v4：排入世代計數；新增驗收） |
| 3 | S2#3 | 重要 | ffmpeg 短檔改成記憶體軌後，P0-3、路由圖、P1-7 沒同步 | v3 D:211、P1 Design 第 7 節 | 已修（v4） |
| 4 | S2#4 | 重要 | 期別「P0（要實測）」不在本期範圍的列舉裡 | — | 已修（v4：期別改 P0，要實測寫進後端欄） |
| 5 | S2#5 | 重要 | 驗收樣本都 ≤15 分鐘，串流分支沒有被驗到 | — | 已修（v4：16 分鐘 APE、WavPack 樣本，ffprobe 長度空值單元測試） |
| 6 | S2#6 | 重要 | 殘響驗收無法分辨上一軌的尾音與本軌的效果 | `dsp.rs:377` 殘響開啟時也改變乾聲增益 | 已修（v4：第二軌開頭 300 ms 靜音，跟重置狀態的對照組比較） |

#### 第 3 輪（對象 v4）

結論：Codex「有條件通過」，沒有阻擋；未推翻前兩輪的「不成立」判定。兩項重要問題已在 v5 修正，**Spec 階段通過**，進入 Code 階段（Code 審查對象是 v5，會一併看到這兩項修正）。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | S3#1 | 重要 | 「長檔＋立刻取消」不保證取消發生在背景準備中 | v4 長檔只先開串流，準備時間很短 | 已修（v5：測試用閘門控制準備時序） |
| 2 | S3#2 | 重要 | 16 分鐘 APE 合成樣本沒有可重現的產生方式 | 本機 `ffmpeg -encoders` 沒有 APE 編碼器（已核對） | 已修（v5：APE 用 Monkey's Audio `MAC.exe`；子程序測試改用 WavPack） |

> 從第 2 輪起審查只由 Codex 做；OpenCode 改為負責實作（使用者 2026-11-22 決定）。

### Code

#### 第 1 輪（對象 v5）

結論：Codex「不通過」（3 阻擋、8 重要）。C1 = Code 第 1 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C1#1 | 阻擋 | 長度改存 NULL 與資料表 `NOT NULL`、`i64` 型別衝突 | `db.rs:65`、`models.rs:17、47`、`types.ts:10` | 已修（v6：維持 0 代表長度不明，不改資料表） |
| 2 | C1#2 | 阻擋 | `TrackChain` 的輸入交界不等於 DSP 輸出交界；外部歸零會被下一次 fill 蓋回去 | `dsp.rs:655–690` 單一累計的 `played_ms` | 已修（v6：交界紀錄＋輸出位置換算每軌進度；鏈的 total 為 None） |
| 3 | C1#3 | 阻擋 | Sink 沒有只撤回下一個音源的 API | `sink.rs:269–286`（已核對） | 已修（v6：Sink 只放一條長駐的鏈，下一首放自管的待播槽；`take()` 是唯一判定點） |
| 4 | C1#4 | 重要 | 引用的 rodio 格式切換測試是停用的 | `queue.rs:261` `#[ignore]`（已核對） | 已修（v6：格式不同時建新鏈，不靠 rodio 佇列切換）；預審 O#4 的核對依據一併更正 |
| 5 | C1#5 | 重要 | 共用 DSP 後下一軌沿用第一軌的正規化增益 | `eq.rs:239、264` 只在建構時傳入 | 已修（v6：`set_norm_gain` 在交界套用，走增益平滑） |
| 6 | C1#6 | 重要 | 只寫 Drop 時 kill+wait 不保證取消完成 | `player.rs:219` 阻塞 `recv()`；std `SyncSender` 沒有 `send_timeout` | 已修（v6：`CancelHandle`、try_send／recv_timeout 迴圈、回收執行緒） |
| 7 | C1#7 | 重要 | 15 分鐘只是路由條件，不是記憶體上限 | `player.rs:127–129` 無上限 extend | 已修（v6：每軌 400 MB 上限，超過改串流） |
| 8 | C1#8 | 重要 | 準備結果的所有權與晚到的處理沒定 | `player.rs:22–35` `Stream(PathBuf)`；`dsp.rs` 讀到 None 就永久結束 | 已修（v6：`PreparedTrack`、完成通道、引擎在鏈結束後用槽建新鏈） |
| 9 | C1#9 | 重要 | 前端副作用會重複或漏執行；沒處理上一首 | `Shell.tsx:570、630–634、711–714` | 已修（v6：副作用改在新 `play_seq` 時做，列出要移除的分支）；`src/player/api.ts` 實際是 `src/lib/api.ts` |
| 10 | C1#10 | 重要 | 逐檔 ffprobe 的並行成本與錯誤回傳被低估 | `scanner.rs:107–125` rayon；`ScanProgress` 沒有錯誤清單 | 已修（v6：只對 lofty 讀不了的檔、上限 4、逾時 10 秒、`errors` 欄位） |
| 11 | C1#11 | 重要 | 交叉淡化分支條件跟 `maybe_crossfade` 不一致 | `player.rs:512–516` | 已修（v6：`crossfade_applies` 用同一套條件） |

#### 第 2 輪（對象 v6，第 1 次複查）

結論：Codex「不通過」（2 阻擋、4 重要；阻擋 3 → 2）。C2 = Code 第 2 輪。依 `dev-workflow`「只修阻塞」，v7 只修兩個阻擋；重要項目交給使用者。

使用者 2026-11-22 決定：C2#3–6 全部納入 P0（見上表）。C2#3 用 rubato 重取樣到裝置格式、不動原始檔。

#### 第 2 輪複查結果（對象 v7，第 2 次複查）

**阻擋已修**：C2#1、C2#2（見第 1 次複查）。
**重要已納入**：C2#3–6（見上表）。
**剩餘**：無阻擋。

> 依 dev-workflow：複查只確認上一輪的阻塞是否關閉、本次修改是否引入新阻塞。已納入的重要項目在下一輪複查範圍內。

#### 第 3 輪（對象 v7）

結論：Codex「不通過」（1 阻擋、2 重要）。C3 = Code 第 3 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C3#1 | 阻擋 | rubato 在 TrackChain 內部，鏈結束仍向同一 Sink append，rodio queue 仍會插 mono/44100 靜音 | `sink.rs:73–82`、`queue.rs:235–248`、`source/uniform.rs:59–108` | 已修（v8：`FormatUnifier` 在 TrackChain 外層、DSP 之前，整條鏈固定輸出格式；Sink 永遠只有一個 DspSource，不進出佇列） |
| 2 | C3#2 | 重要 | PreparedTrack 沒有路徑與解碼後端，換軌後 seek/換裝置無法重建 | `player.rs:267–293、315–350`；`audio_decode.rs:23–37` | 已修（v8：PreparedTrack 帶 path、decoder；引擎原子交換 TrackChain） |
| 3 | C3#3 | 重要 | 準備工作上限 1 仍是逐次 spawn，世代只在完成後檢查 | `player.rs:127–129`、`audio_decode.rs:122–178` | 已修（v8：單一 worker + 待處理槽；取消檢查點在解碼迴圈；等待不在引擎迴圈與槽鎖內） |

#### 第 4 輪（對象 v8）

結論：Codex「不通過」（2 阻擋、2 重要）。C4 = Code 第 4 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C4#1 | 阻擋 | 固定格式外層與「鏈結束、Sink 空了再建新鏈」並存：若送 None 到 DSP 仍回到 rodio 空佇列靜音；若外層長駐，Sink.empty() 永遠不成立 | `dsp.rs:659–673`、`queue.rs:235–248`、`sink.rs:309–319` | 已修（v9：FormatUnifier 狀態機 Playing/WaitingForNext/Finished；WaitingForNext 輸出靜音繞過 DSP、不送 None；只有 Finished 時 DSP 收到 None） |
| 2 | C4#2 | 阻擋 | 換輸出裝置不能只換 TrackChain：Sink 綁定舊 mixer，新裝置取樣率不同時「固定格式」與「不重建」衝突 | `player.rs:311–326`、`sink.rs:73–76`、`stream.rs:15–24`、`dsp.rs:623–630` | 已修（v9：換裝置明確重建 OutputStream/Sink/DspSource<FormatUnifier>，固定格式 = 新裝置取樣率/雙聲道；TrackChain 移入新 FormatUnifier；DSP 濾波器/殘響/節奏/重取樣器狀態保留；play_seq/now_token/seek_origin_ms/format_state 保留） |
| 3 | C4#3 | 重要 | seek 只換 TrackChain，不清舊 DSP 緩衝、變速緩衝、交界紀錄 | `dsp.rs:599–605、701–705、500–580`、`player.rs:267–271` | 已修（v9：seek 觸發 Playing→Playing(new_chain_at_seek)，同步清空 FormatUnifier 與 DSP 所有緩衝、重取樣器、交界紀錄；seek_origin_ms 歸零、位置基準 = seek 目標） |
| 4 | C4#4 | 重要 | 交叉淡化先混音後重取樣：兩軌不同取樣率逐樣本相加，A 的 1kHz 被解讀為 1088Hz | `player.rs:183–202` | 已修（v9：交叉淡化先各自重取樣到固定格式，再在共同時間軸混音，最後送入長駐 DSP） |

#### 第 5 輪（對象 v9）

結論：Codex「不通過」（2 阻擋、1 重要）。C5 = Code 第 5 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C5#1 | 阻擋 | Waiting 狀態輸出靜音仍經過 DSP（變速、EQ、殘響），無法達成凍結；仍等 Sink.empty() | `dsp.rs:659–690`、`player.rs:651–655`、rodio `done.rs:54–60` | 已修（v10：DSP 內部固定 48kHz；WaitingForNext 時 DSP 凍結、緩衝/濾波/增益完全不動；TrackResampler 輸出 48kHz 靜音繞過 DSP 處理邏輯；引擎依狀態機與準備完成通知接軌，不等 Sink.empty()） |
| 2 | C5#2 | 阻擋 | 換取樣率不能沿用舊 DSP 緩衝：48kHz 1kHz 變 918.75Hz；v9 要求清空又保留，互衝 | `dsp.rs:248–256、457–467、628–630`、`eq.rs:228、267`、rodio `sink.rs:104–162` | 已修（v10：DeviceConverter 在 DSP 之後，換裝置只重建這一層；DSP 完全不動、48kHz 內部取樣率固定；播放身分、位置、控制參數全部保留） |
| 3 | C5#3 | 重要 | 交叉淡化仍指定走現有混音路徑（stop/clear/append），會重置 DSP 違反長駐管線 | `player.rs:530–542、158–175` | 已修（v10：混音結果直接送入現有 TrackResampler → DSP，不執行 stop/clear/append、不建新 DspSource） |

#### 第 6 輪（對象 v10）

結論：Codex「不通過」（1 阻擋、2 重要）。C6 = Code 第 6 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C6#1 | 阻擋 | 只重建 DeviceConverter 保留同一 Sink 不成立：Sink 綁定舊 mixer，無法重新綁定 | rodio `sink.rs:73–76`、`stream.rs:15–24、79–85`、現有 `player.rs:311–326` | 已修（v11：重建 OutputStream/Sink/DeviceConverter、DspStateSnapshot 移交 EQ 濾波器、殘響/節奏緩衝、增益、TrackResampler 狀態；新 DspSource 用快照初始化不重置；TrackResampler/TrackChain 繼續用原實例） |
| 2 | C6#2 | 重要 | 正文仍有等 Sink.empty()、走現有混音路徑、異格式結束鏈等過時互斥指示 | v10 正文與新架構衝突 | 已修（v11：刪除等 Sink.empty()、走現有混音路徑、異格式結束鏈、等待靜音進 DSP 等過時描述；同步正文、時序圖） |
| 3 | C6#3 | 重要 | TrackResampler 缺少原生→48kHz frame 邊界換算契約：44.1k 一秒落在 918.75ms 導致換曲提前 | Design 252 記錄 TrackChain 輸入位置、255–260 直接對照 DSP 檢查點 | 已修（v11：TrackResampler 負責把原生輸入 frame 邊界轉成 48kHz 輸出 frame 邊界，含重取樣延遲、尾端補樣、聲道轉換；引擎用 48kHz 邊界更新 now_seq、played_ms、正規化增益） |

#### 第 7 輪（對象 v11）

結論：Codex「不通過」（1 阻擋、2 重要）。C7 = Code 第 7 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C7#1 | 阻擋 | 換裝置取消與移交衝突：取消後串流回 None 不可用；不取消又 clear() 會卡住 | rodio `sink.rs:104–162`、`queue.rs:227–248`、`dsp.rs:589–605` | 已修（v12：暫停拉取→快照→重建管線→移交→恢復；`pause_flag` 暫停音訊回呼，`DspStateSnapshot` 完整移交所有狀態；不取消 ffmpeg、不重開解碼） |
| 2 | C7#2 | 重要 | 過時路徑仍在正文與測試：時序圖等 Sink.empty()、異格式重置 DSP、走現有混音路徑 | `player.rs:530–542、158–175`、`dsp.rs:659–673`、rodio `queue.rs:235–248` | 已修（v12：時序圖、架構決定、第 4 點、測試統一為固定 48kHz 長駐管線；刪除等 Sink.empty()、異格式重置 DSP、走現有混音路徑） |
| 3 | C7#3 | 重要 | TrackResampler 邊界換算未落入實作契約：仍記 TrackChain 輸入 frame、直接對照 DSP 檢查點 | Design 252 記 TrackChain 輸入、255–261 直接對照 DSP、333 又說先轉 48k | 已修（v12：TrackResampler 單獨負責原生→48kHz frame 邊界換算，含延遲、尾端補樣、聲道轉換；引擎只用 48kHz 邊界；44.1k→48k 交接不再提前 81.25ms） |

#### 第 8 輪（對象 v12）

結論：Codex「不通過」（1 阻擋、2 重要）。C8 = Code 第 8 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C8#1 | 阻擋 | 暫停與移交仍衝突：設旗標不代表 next() 已退出；clear() 等待回呼與暫停矛盾 | rodio `sink.rs:269–273、303–305`、`queue.rs:227–229`、`dsp.rs:659–663` | 已修（v13：暫停舊 Sink→快照→新管線→換用→丟棄舊，**不呼叫 clear()**，無死結） |
| 2 | C8#2 | 重要 | 過時路徑仍在正文與測試：第 224、267、268 行等 | v12 正文與新架構衝突 | 已修（v13：徹底清理第 224、267、268 行等所有過時路徑） |
| 3 | C8#3 | 重要 | 48kHz 邊界換算仍未接入正文演算法：仍產生 TrackChain 原生 frame、直接對照 DSP | Design 256 產生 TrackChain 原生、259–260 直接對照 DSP | 已修（v13：第 4 點演算法明確寫入 48kHz 邊界換算公式、單位（DSP 輸入端 48kHz frame）、累計方式；引擎只用 48kHz 統一座標） |

#### 第 9 輪（對象 v13）

結論：Codex「不通過」（2 阻擋、1 重要）。C9 = Code 第 9 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C9#1 | 阻擋 | 換裝置交接缺乏安全同步點：pause() 返回不代表回呼已停止使用來源 | rodio `sink.rs:254–255`、`dsp.rs:659–663`、`cpal wasapi/stream.rs:159–164` | 已修（v14：請求交接→等待回呼確認→快照→新管線→換用→丟棄舊；handoff_requested/handoff_ack 明確同步點） |
| 2 | C9#2 | 阻擋 | 48kHz 座標仍不一致：交界在 DSP 輸入時發布、進度用輸出 frame 減輸入邊界，變速時必錯 | `dsp.rs:659–670、701–705`、WSOLA 節奏 0.8 產生 60k 輸出 frame | 已修（v14：交界在 DSP 輸出端發布、進度從輸出 frame 映射回輸入座標、變速由檢查點映射自動處理） |
| 3 | C9#3 | 重要 | 軌交接仍有兩套互斥指示：引擎看到槽交換 vs TrackChain 讀完時 take() | Design 233、237、283 vs 267、270–271、359 | 已修（v14：唯一判定點是 TrackChain 讀完時 take()；引擎不主動交換、異格式走同一條路徑） |

#### 第 10 輪（對象 v14）

結論：Codex「不通過」（2 阻擋、1 重要）。C10 = Code 第 10 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C10#1 | 阻擋 | 交接確認依賴下一次 fill()，暫停時無人設定 ack，引擎無限等待 | rodio `sink.rs:122–149`、`pausable.rs:76–87`、`dsp.rs:701–702` | 已修（v15：共享 PlaybackCore + 原子交換 OutputProxy，無需回呼配合、暫停時也能切換） |
| 2 | C10#2 | 阻擋 | 仍在 DSP 輸入端發布交界，進度用輸出 frame 減輸入邊界，變速時必錯 | `dsp.rs:499–526、659–690`、WSOLA 節奏 0.8 | 已修（v15：唯一座標 = DSP 輸出端 48kHz frame，交界在輸出時發布、進度從輸出 frame 直接計算、變速自動正確） |
| 3 | C10#3 | 重要 | 軌交接仍有兩套指示：引擎看到槽交換 vs TrackChain 讀完時 take() | Design 233、237、266–270、367–368 | 已修（v15：唯一判定點是 TrackChain.take()，引擎永不主動交換、異格式走同一條路徑） |

#### 第 11 輪（對象 v15）

進行中（Codex）。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | C2#1 | 阻擋 | 交界在 `fill` 時就發布，沒扣掉 `self.out` 還沒送出的部分；WSOLA 一次可產生約 50 ms，「約 23 ms」不成立 | `dsp.rs:655–705`（已核對：先整批產生，`next()` 才逐樣本送出） | 已修（v7：檢查點內插，`next()` 送到交界才發布） |
| 2 | C2#2 | 阻擋 | 準備中改成不續播時，前端比對快照的 `next_token`（仍是空的），不會送 `clear_next` | `commands.rs` 只送 mpsc 就回傳；Design v6 第 7 點 | 已修（v7：比對「已要求的 token」，加 `next_clear_epoch`） |
| 3 | C2#3 | 重要 | 新鏈仍經過同一條 rodio queue，空檔時插 mono／44100 靜音，格式切換的問題沒有真正避開 | `queue.rs:125–168、198–209`；`source/uniform.rs:59–108` | **待使用者決定**（見對話） |
| 4 | C2#4 | 重要 | 換軌後引擎的 `current`、`seek_origin_ms` 沒有接好，seek 或換裝置可能重開上一首 | `player.rs:267–293、315–350`；`SymStream` 沒存路徑 | **待使用者決定** |
| 5 | C2#5 | 重要 | 「合計最多三份」沒有機制：每次 `set_next` 都開新執行緒 | `player.rs:110–130`、`audio_decode.rs:121–178` 沒有取消檢查 | **待使用者決定** |
| 6 | C2#6 | 重要 | 工作階段恢復的 `player_prepare` 沒有 token，恢復後的播放紀錄可能漏記或重複 | `Shell.tsx:433–451`、`commands.rs:123–130` | **待使用者決定** |

### Security

#### 第 1 輪（對象 v25）

結論：Codex「需修正」（2 阻擋、9 重要、1 待核對、2 待決定、2 不成立）。Sec = Security 第 1 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | Sec#1 | 阻擋 | **本機磁碟檢查不能阻止符號連結轉往網路。** `C:\Music\link\song.flac` 可以經符號連結指向 UNC；檢查原始磁碟類型後，`metadata` 就可能存取遠端。應在跟隨重解析點前處理其目標，明定拒絕或安全解析方式；事後才 canonicalize 無法保證「拒絕前不連網」。 | D:210–214；Microsoft 明載[符號連結可指向 UNC](https://learn.microsoft.com/en-us/windows/win32/fileio/creating-symbolic-links)。 | 成立 |
| 2 | Sec#2 | 阻擋 | **禁網驗證的入口覆蓋不完整。** 設計只明列播放／預載／探測共用驗證，但 argv、拖放分類及掃描會更早呼叫 `is_file`／`is_dir`／`metadata`。即使最後拒絕播放，前面的檔案檢查仍可能接觸網路。應明列這些入口在第一次檔案系統存取前共用驗證，並要求一般檔案，不能僅確認 metadata 成功。 | D:41、210–227；`src-tauri/src/open_with.rs:12–13,72–79`；`src-tauri/src/extras.rs:274–281`；`src-tauri/src/scanner.rs:29–40,64`。 | 成立 |
| 3 | Sec#3 | 重要 | **電台路徑的協定限制仍未定義。** 「另行驗證」沒有列出允許的協定及巢狀資源規則；現有程式只檢查網址前綴，ffmpeg 沒有白名單。惡意串流清單可能引導額外資源存取。應定義電台專用白名單、重導向／巢狀輸入限制及連線逾時；是否允許區網電台需明確保留或限制，不能自行假定。 | D:213–214、219；`src-tauri/src/commands.rs:500–505`；`src-tauri/src/player.rs:440–443`；[FFmpeg 協定規則](https://ffmpeg.org/ffmpeg-protocols.html)。 | 成立 |
| 4 | Sec#4 | 重要 | **S2 的清洗順序仍互相矛盾。** 先保留 stderr 最後 4 KB，已可能切掉 `https://` 等辨識前綴；之後「清洗再截成 1 KB」仍會留下憑證片段。反過來累積「完整 stderr」又失去記憶體上限。應採有界、可跨讀取區塊保留辨識狀態的清洗，再保存清洗後尾端；無法辨識的片段不寫出。 | D:221、229–232；S2。 | 成立 |
| 5 | Sec#5 | 重要 | **S2 未涵蓋其他日誌入口的敏感資訊。** 原始 argv、TCP 本體與完整路徑另有直接寫入日誌的路徑，繞過 stderr 清洗；可能留下使用者名稱、私人目錄、網址權杖及偽造換行。應將清洗與單筆長度限制放到共用 logger，明定完整路徑是否保留。ASCII 過濾不等於個資遮蔽。 | D:45、229–233；`src-tauri/src/open_with.rs:26–45,78,147`；`src-tauri/src/commands.rs:620,627`。 | 成立 |
| 6 | Sec#6 | 重要 | **S4 通道容量計算錯誤。** `4096 × sizeof(f32)` 是 **16 KiB**，256 區塊是 **4 MiB**，不是 8 KB／2 MB。若照標示的 8 KB 計費，上限會失準。應統一 MB／MiB，依實際樣本數乘 4 計費；2 MiB 對應最多 128 個此大小區塊。 | D:52、346。 | 成立 |
| 7 | Sec#7 | 重要 | **S4 的限制尚未涵蓋解析器內部配置。** ffprobe stdout 64 KB、文字標籤上限及最終 PCM 上限，不能限制 ffprobe 本身的工作記憶體，也不能限制 lofty 解析標籤／內嵌封面或 Symphonia 單封包配置。應補述這些配置的限制或明列殘餘風險，不能以目前數字代表整個解碼流程已有記憶體保障。 | D:224–225、357–360；`src-tauri/src/scanner.rs:163–184,232–234`；`src-tauri/src/audio_decode.rs:138–153`。 | 成立 |
| 8 | Sec#8 | 重要 | **S5 的逾時回收會丟失追蹤能力。** 超時後 `drop()` 執行緒 handle 不會終止執行緒；若 `kill()` 失敗，丟掉 child handle 也不會終止程序。此時又要求「確認結束前持續佔額」，可能永久凍結準備工作。應保留監管 handle、處理 kill 失敗，並定義不可取消的解碼工作如何恢復。 | D:55、353；Rust 官方 [JoinHandle](https://doc.rust-lang.org/std/thread/struct.JoinHandle.html)、[Child](https://doc.rust-lang.org/std/process/struct.Child.html) 行為。 | 成立 |
| 9 | Sec#9 | 重要 | **S5 非阻塞承諾未涵蓋核心鎖。** `next()` 仍用阻塞式 `core.lock.lock()`；換裝置又在同一把鎖內建立 OutputStream／Sink。裝置初始化延遲時，音訊回呼仍會卡住，`try_recv()` 無法解決。應把可能阻塞的建立工作移出核心鎖，鎖內只進行必要交換。 | D:348、381–395；S5。 | 成立 |
| 10 | Sec#10 | 重要 | **S6 還不足以形成可驗收的權限政策。** 「CSP 非 null」未指定允許來源；`AppManifest::commands` 也未列出 command 與 capability 的授權對照。應定義 main 視窗可呼叫的命令、遠端來源政策，以及 script／connect／image 等必要 CSP 指令；同步列出拒絕案例。未確認存在可利用 XSS。 | D:57–58；`src-tauri/build.rs:1–3`；`src-tauri/capabilities/default.json:5–23`；`src-tauri/tauri.conf.json:23–24`；[Tauri capabilities](https://v2.tauri.app/security/capabilities/)、[CSP](https://v2.tauri.app/security/csp/)。 | 成立 |
| 11 | Sec#11 | 重要 | **S3 仍允許從網路候選位置執行工具。** 排除空白與相對 PATH 並未排除 UNC、網路磁碟機或連到遠端的候選位置；啟動時跑 `-version` 已是在執行該工具。應先驗證候選位置為受信任本機位置，再執行，並明列工具來源的信任責任；版本日誌不是完整性驗證。 | D:48、209。 | 成立 |
| 12 | Sec#12 | 重要 | **ffprobe 的資源限制是否涵蓋全部呼叫點不明。** 10 秒與最多 4 個的文字位於「掃描成本控制」，但播放／預載也會探測。應明定所有探測共用限制、並行配額是否為程序全域，以及 stdout 超限時立即停止／回收，避免實作者先用無界 `output()` 收完才檢查。 | D:216、224–225、314；S4、S5。 | 待核對 |
| 13 | Sec#13 | 重要 | **S7 保留了尚未決定的外部副作用。** 現況不只建立專用 ProgID，還每次啟動以 `/f` 改寫多個副檔名的 HKCU 預設關聯。應決定預設不寫入、明確選擇後註冊，或保留自動覆寫並明示；不能把目前文字當作完成的可攜模式政策。 | D:60–61；`src-tauri/src/main.rs:40`；`src-tauri/src/open_with.rs:48–63`。 | 待決定 |
| 14 | Sec#14 | 重要 | **S1 已明示不驗證身分，但權限提升情境仍需界定。** loopback 不等於同一 Windows 使用者；若播放器被提升權限啟動，未驗證的本機發送者仍可要求其開檔。應明定不支援提升權限執行，或重新處理 IPC 身分隔離。設計已將 named pipe／ACL 排到 P1，本次不將該既定延期重新列為阻擋；未證實完整提權利用鏈。 | D:40–42；`src-tauri/src/open_with.rs:105–123,131–157`；`Agents/permissions.md` 的一般使用者權限說明。 | 待決定 |
| 15 | Sec#15 | 次要 | **S3 所列既有 Cargo 相依未鎖定的疑慮不成立。** Cargo.lock 已受 Git 追蹤，且四個版本與設計相符。rubato 尚未加入，需在選版後核對鎖檔與公告；本次未做完整依賴漏洞掃描。 | D:49、209；`src-tauri/Cargo.lock:2514–2517,3759–3762,4351–4354,4638–4641`。 | 不成立 |
| 16 | Sec#16 | 次要 | **未發現現有 ffmpeg 呼叫的 shell 字串注入。** 已讀呼叫使用獨立參數，轉檔格式有白名單、位元率有範圍限制；登錄開啟命令也引用了 exe 與 `%1`。新 ffprobe 呼叫仍應明寫沿用 `Command` 參數陣列。此結論不消除第 1–3 項的路徑／協定風險。 | D:216–217；`src-tauri/src/player.rs:440–441`；`src-tauri/src/extras.rs:205–232`；`src-tauri/src/open_with.rs:51–54`。 | 不成立 |

> S1 的 loopback 綁定、單連線大小／時間上限，以及 S2 的單檔 5 MB、總量 25 MB 輪替要求，**已在設計明列**，不再列為缺漏。實作仍須驗證限制是在讀取／寫入當下執行，且輪替總量包含目前活動檔案。
>
> 本次為設計與原始碼唯讀審查；未執行惡意媒體、網路連線、登錄寫入、提權或壓力測試。

## 實作審查

### Spec
#### 第 1 輪（對象 f33e18f + 未提交變更）
結論：Codex「需修正」（9 阻擋、4 重要）。Imp = Implementation 第 1 輪。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| 1 | Imp#1 | 阻擋 | **P0-3 長駐播放管線未實作。** 實際仍逐軌建立 `DspSource`；換裝置重新 append，交叉淡化仍 `stop/clear/append`。未找到 `TrackChain`、`TrackResampler`、`PlaybackCore`、`OutputProxy`、`DeviceConverter` 的實作，Cargo 也沒有 rubato。無法滿足固定 48kHz、DSP 狀態延續及輸出交界契約。須完成管線並接入引擎。 | D:292–304、339–360；`player.rs:158`、`:299–341`、`:534–539`；`Cargo.toml:37` | 成立 |
| 2 | Imp#2 | 阻擋 | **P0-3 指令與前端流程未接上。** 仍使用 `player_preload`，缺少帶 token 的 load／prepare、set_next／clear_next 及快照序號。前端仍在載入後記歷史、播放前消耗佇列、每 250ms 偵測 ended 再載入下一首；沒有設計要求的統一決策與取消流程。新增 `NextCoordinator` 尚未成為實際播放路徑。 | D:310–337、384–395；`models.rs:189`、`:243–264`；`api.ts:27`；`Shell.tsx:570`、`:630–634`、`:711–718` | 成立 |
| 3 | Imp#3 | 阻擋 | **P0-2 新解碼路由沒有接入正式播放，串流仍是占位實作。** `decode_to_pcm` 仍是 Symphonia → rodio，沒有依格式矩陣走 ffmpeg。`FfmpegStream::next()` 收到非空資料仍回傳 `None`，seek 明確回傳未實作錯誤。新增格式與長檔播放／seek 不能驗收。 | D:227–241、267–270；`player.rs:110`；`audio_decode.rs:456`、`:483–492` | 成立 |
| 4 | Imp#4 | 阻擋 | **掃描沒有 ffprobe 後備與錯誤清單。** `process_single_file` 仍只呼叫 lofty，失敗直接變成長度 0；矩陣的 `MetaBackend` 未驅動掃描。`ScanProgress` 沒有 `errors`，前端只顯示進度文字。TTA／WMA 等格式無法依設計取得資料或回報逐檔失敗。 | D:242–244、263–265；`scanner.rs:299`；`models.rs:108`；`Shell.tsx:460` | 成立 |
| 5 | Imp#5 | 阻擋 | **S1 共用路徑驗證未覆蓋入口。** 播放直接開檔、拖放直接 `is_dir/is_file`、掃描直接走訪與 `metadata`，argv 也未驗證。驗證函式的 `GetFinalPathNameByHandleW` 傳入 `INVALID_HANDLE_VALUE`，並未取得有效檔案 handle 解析目標。須補齊首次存取前的驗證與有效解析流程。 | D:40–45、515–516；`audio_decode.rs:47`；`extras.rs:269`；`scanner.rs:171`；`open_with.rs:101`、`:328–344` | 成立 |
| 6 | Imp#6 | 阻擋 | **S3 工具查找不符合契約。** PATH 以檔案路徑分隔字元拆解，無法正確解析 Windows 的分號清單；僅 PATH 安裝的工具可能找不到。執行前只驗證目錄，沒有驗證工具檔案本身的遠端連結；重解析檢查同樣使用無效 handle。須修正 PATH 解析並驗證實際執行目標。 | D:56、226、256、517；`tools.rs:74`、`:140–162` | 成立 |
| 7 | Imp#7 | 阻擋 | **S4 資源限制大部分未實作。** ffprobe 使用無界 `.output()`，沒有共用四程序配額、64KB 上限或十秒逾時；PCM 使用無上限 collect／extend；lofty 沒有標籤限制且仍載入封面；Symphonia 沒有 256KB 封包檢查，錯誤計數每次 refill 重設。128 區塊通道不能代替這些限制。 | D:59–65、374–382；`next_track.rs:323`、`:364–367`、`:415–428`；`player.rs:127`；`scanner.rs:305`、`:326`；`audio_decode.rs:131` | 成立 |
| 8 | Imp#8 | 阻擋 | **S5 非阻塞與監管回收未落地。** `FfmpegStream`、電台仍使用阻塞 `recv()`。取消時取走 Child、忽略 kill 結果後即釋放；Drop 沒有把執行緒 handle 移交監管者。未見十秒 deadline、kill 失敗續追、三十秒孤兒記錄或回收期間凍結準備工作的機制。 | D:67–70、363–372；`audio_decode.rs:259`、`:473–485`；`player.rs:215`；`next_track.rs:195` | 成立 |
| 9 | Imp#9 | 阻擋 | **S6 權限與協定政策未實作。** CSP 仍是 `null`，build.rs 沒有 command manifest；電台 ffmpeg 參數沒有協定白名單、重導向限制、十秒逾時或禁止巢狀清單的設定。本機 ffmpeg／ffprobe 呼叫也缺少 `file,pipe` 白名單。 | D:72–75、231–232、523；`tauri.conf.json:23`；`build.rs:1`；`player.rs:444`；`next_track.rs:365`、`:416` | 成立 |
| 10 | Imp#10 | 重要 | **S2 清洗與輪替未完整符合要求。** 清洗函式沒有移除 URL query，例如 `https://host/?token=secret` 會保留權杖；辨識狀態也不跨呼叫保存。stderr 仍先截原始尾端，沒有接入清洗日誌。輪替只計算歷史檔，最多五個歷史檔另加活動檔，未保證含活動檔的五檔／總量限制。 | D:47–53、518；`open_with.rs:131`、`:245–285`；`audio_decode.rs:373` | 成立 |
| 11 | Imp#11 | 重要 | **格式矩陣與入口清單不一致。** mp1／mp2／spx 中繼資料後端與設計不同，額外加入 `.alac`；右鍵另維護硬編碼清單且多出 `.wavpack`。`playback_backend_for("ofr")` 仍回傳 ffmpeg，argv 對 `.ofr/.mid` 則直接忽略，沒有指定的拒絕原因。須統一來源並落實受阻格式處理。 | D:197–221、245、266；`scanner.rs:18`、`:28`、`:36`、`:153–154`；`open_with.rs:18`、`:335–344` | 成立 |
| 12 | Imp#12 | 重要 | **S7 僅完成取消啟動自動註冊。** 註冊函式沒有使用者操作的呼叫鏈，未找到設定頁「設為預設播放器」及取消關聯實作。設計承諾的主動註冊／取消流程尚缺。 | D:78、524；`main.rs:43`；`open_with.rs:304`；全域呼叫點搜尋 | 成立 |
| 13 | Imp#13 | 阻擋 | **必要驗收材料缺失。** 未找到 `scripts/make-format-fixtures.ps1`、`src-tauri/tests/fixtures/formats/`、`nextTrack.test.mjs`；協調器現有測試只驗世代與清槽，未提供設計指定的交界、取消競爭、異格式與狀態延續驗收。現有 Review 的實作三階段也都仍是「待進行」。 | D:255–272、430–509；`next_track.rs:449`；`Review.md:247`；檔案清單核對 | 成立 |

> 另確認：**P0-0 名稱搜尋無命中**（tracked/untracked 雙重 grep 均無輸出）；**P0-1 原始碼與測試存在**（未執行不能認定通過）；**通道數值已修正**（128 chunks × 16 KiB = 2 MiB）；**NextCoordinator Drop 先 drop sender 再 join**；**main.rs 自動註冊已註解**。

### Code
#### 第 3 輪（對象 f33e18f + v26 + 第四次實作變更）
結論：Codex「需修正」（多項阻擋、重要）。C1、C2、C4、C5 發送／錯誤、C6、C8 已修；其餘仍成立。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| C1 | 阻擋 | **已修**：load／prepare 的 API 包裝及呼叫點已傳 token。原本命令參數解析失敗已修；token 後續被覆蓋見 Imp#2。 | `src/lib/api.ts:27`；`src/player/Shell.tsx:471`、`:586` | 已修 |
| C2 | 阻擋 | **已修**：一般 `C:\...` 路徑已擷取磁碟根目錄再呼叫 `GetDriveTypeW`；原本全面誤拒本機檔案已修。重解析目標驗證見 SE1/SE5。兩個驗證函式將完整路徑傳給 `GetDriveTypeW`，該 API 要求根目錄及尾端反斜線。本機唯讀實測：`C:\\` 回傳 3，專案目錄與 `AGENTS.md` 均回傳 1，程式隨即拒絕。 | `src-tauri/src/open_with.rs:79`、`:82`；`src-tauri/src/tools.rs:50`、`:53` | 成立 |
| C3 | 阻擋 | **已累積完整 BLOCK_FRAMES，DeviceConverter 也會建立 resampler；但換軌時 `flush_tail()` 產出的音訊隨即被 `set_track()` 清空。** 整區塊結束時 `remaining=0` 又直接跳過尾端排出，且未裁掉補零／補償延遲，仍不能保證無縫交界。 | `src-tauri/src/playback_pipeline.rs:419`、`:430`、`:776`、`:812`、`:827`、`:877`、`:998` | 成立 |
| C4 | 阻擋 | **已修**：completion 發送已改為 100ms `send_timeout` 並檢查取消旗標；Drop 關閉 pending sender。原本「completion 滿載導致 send 永久阻塞」已修；準備工作本身的取消／回收另見 SE9。 | `src-tauri/src/playback_pipeline.rs:1329`、`:1424` | 已修 |
| C5 發送／錯誤 | 阻擋 | **已修**：pending 已用 `try_send` 替換舊請求，completion 已攜帶 `Result<PreparedTrack, String>`，失敗會抵達引擎。 | `src-tauri/src/playback_pipeline.rs:1272`、`:1354`；`src-tauri/src/player.rs:860` | 已修 |
| C5 取消 | 阻擋 | **ClearNext 遞增協調器世代，卻未更新 `engine.next_coord_gen`；poll 仍拿舊世代比較。** 若準備在 clear 後完成，舊結果仍會入槽。Load／Seek 也沒有真正清除該協調器。 | `src-tauri/src/player.rs:357`、`:835`、`:840`、`:852`；`src-tauri/src/playback_pipeline.rs:1373`、`:1389` | 成立 |
| C6 | 重要 | **已修**：常數已統一為 2048 frames × 2 channels × 4 bytes = 16 KiB；128 區塊為 2 MiB。 | `src-tauri/src/playback_pipeline.rs:28`；`src-tauri/src/audio_decode.rs:391`、`:464` | 已修 |
| C7 | 重要 | **已測正式協調器且新增非靜音斷言；但交接測試只要求任一樣本非零**，沒有確認兩軌皆輸出、交界時間或 DSP 延續。滿 completion 測試送出一次後立即 Drop，未等待通道滿，也沒有第二筆阻塞發送，不能證明指定情境。 | `src-tauri/src/playback_pipeline.rs:1672`、`:1685`、`:1788` | 成立 |
| C8 | 重要 | **已修**：指定的兩個案例已修（單曲循環跳過 failed 歌曲；全部循環單曲清單回傳該歌），且有對應測試。 | `src/player/nextTrack.ts:103`、`:151`；`src/player/nextTrack.test.mjs:131` | 已修 |
| C8 衍生 | 重要 | **下一首搜尋無條件使用 modulo，回到第一首也未限制 All 模式。** 直接執行現有函式確認：`[A,B]`、目前 B、手動下一首，在 Sequence／Single 都回 A。須恢復「只有全部循環才繞回」規則。 | `src/player/nextTrack.ts:134`、`:152`；`planning/P0-daily-blockers/design.md:318` | 成立 |
| 進度／交界 | 阻擋 | `played_out_frames` 每個 sample 加一，串流又額外加一次，卻直接除以 48000。48kHz 雙聲道記憶體軌約回報兩倍進度，串流約四倍；沒有 DSP checkpoint 映射，身分也在輸入換軌時提前發布。 | `src-tauri/src/playback_pipeline.rs:314`、`:326`、`:435`、`:441`；`src-tauri/src/player.rs:205` | 成立 |
| 交接後 seek | 阻擋 | **自然交接不更新 `current_path/current_decoder`；B 開始後 seek 仍使用 A 的路徑。** restart 又呼叫會配置新 seq 的 load_at，違反 seek 保留播放身分的契約。 | `src-tauri/src/player.rs:437`、`:576`、`:586`、`:350`；`src-tauri/src/playback_pipeline.rs:401` | 成立 |
| 換裝置 | 重要 | **新 Sink 在 active_id 切換前就 append 新 proxy**，音訊回呼可能先拉取並因 ID 不符回 None，使新來源提前結束。須先控制新 Sink 的拉取時機再啟用代理。 | `src-tauri/src/player.rs:304`、`:308`、`:313`；`src-tauri/src/playback_pipeline.rs:1121` | 成立 |
| 軌資料生命週期 | 阻擋 | **TrackChain 每次交接都 push 新軌，從未釋放已播軌的 `Arc<Vec<f32>>`。** 長時間連續播放會累積所有 PCM，違反最多三份的記憶體契約。 | `src-tauri/src/playback_pipeline.rs:542`、`:601`；`planning/P0-daily-blockers/design.md:67` | 成立 |
| 驗證／Rust | 阻擋 | **`cargo test --release`：38 passed, 0 failed**；實測通過，但測試未涵蓋上述正式播放路徑。 | `AGENTS.md:50`；實測結果 | 已修 |
| 驗證／Node | 重要 | **指定 Node 指令實跑 5 passed, 0 failed**；此結果不消除 C7、C8 衍生、進度／交界、交接後 seek、換裝置、軌資料生命週期的驗證缺口。 | `AGENTS.md:50`；實測結果 | 已修 |
### Security
#### 第 3 輪（對象 f33e18f + v26 + 第四次實作變更）
結論：Codex「需修正」（多項阻擋、重要）。SE12 已修；其餘仍成立。

| # | 來源 | 嚴重度 | 問題 | 核對 | 狀態 |
|---|---|---|---|---|---|
| SE1 | S1／Imp#5 | 阻擋 | **共用驗證未覆蓋入口**：播放與準備已有呼叫，但掃描、拖放仍先 `is_dir/is_file/metadata`；argv 未經驗證。`validate_local_path` 已改成真實 `CreateFileW` handle，此部分已修；仍須補齊首次存取前驗證，並修正 C2。 | `src-tauri/src/open_with.rs:102`、`:388`；`src-tauri/src/scanner.rs:175`、`:302`；`src-tauri/src/extras.rs:274` | 成立 |
| SE2 | S1 | 重要 | Socket 僅設定每次 read 的 2 秒 timeout，沒有總 deadline，且只做兩次 `read()`。TCP 分段可能截斷路徑；兩次等待可超過總 2 秒，也未在超量時完整拒絕整筆訊息。 | `src-tauri/src/open_with.rs:472`、`:476`、`:487`、`:500`；D:46 | 成立 |
| SE3 | S2／Imp#10 | 重要 | **含憑證 URL 仍會洩漏 query**：遮蔽 `@` 時將 `in_url=false`，所以 `https://user:pass@host/path?token=secret` 的 query 不再清除。狀態也不跨呼叫保存。 | `src-tauri/src/open_with.rs:156`、`:224`、`:229`、`:237`、`:253` | 成立 |
| SE4 | S2／Imp#10 | 重要 | **stderr 尚未統一清洗**：FfmpegStream 先保留原始最後 4 KB，日誌寫入仍是註解；電台與轉檔將原始 stderr 截尾直接回前端。須共用清洗後才截斷，前端只顯示整理過的原因。 | `src-tauri/src/audio_decode.rs:341`、`:350`；`src-tauri/src/player.rs:659`、`:664`；`src-tauri/src/extras.rs:230` | 成立 |
| SE5 | S3／Imp#6 | 阻擋 | Windows PATH 已改分號，`-version` 前也已檢查目錄與工具檔；但**受信任路徑檢查仍不正確**：`tools.rs` 仍把 `INVALID_HANDLE_VALUE` 傳給最終路徑解析，會保守拒絕重解析點，不能證明其目標可信。另 FfmpegStream 直接推導同目錄 ffprobe，繞過 locate。 | `src-tauri/src/tools.rs:75`、`:141`、`:157`、`:161`；`src-tauri/src/audio_decode.rs:253` | 成立 |
| SE6 | S4／Imp#7 | 阻擋 | **ffprobe 配額／64 KB／10 秒限制未實作**。播放、準備、FfmpegStream 仍使用無界 `.output()`，沒有共同四程序配額、輸出上限與 deadline。整首 PCM 也仍無界 collect／output，未落實 400 MB 轉串流。 | `src-tauri/src/player.rs:386`、`:458`、`:516`；`src-tauri/src/playback_pipeline.rs:1122`、`:1160`；`src-tauri/src/audio_decode.rs:254` | 成立 |
| SE7 | S4／Imp#7 | 阻擋 | **解析器限制仍缺失**：lofty 無單欄／總標籤上限，仍讀取嵌入封面；取樣率與聲道未驗證指定範圍。Symphonia 無 256 KB packet cap，錯誤計數每次 refill 歸零，不是跨封包累計。 | `src-tauri/src/scanner.rs:307`、`:314`、`:325`、`:328`；`src-tauri/src/audio_decode.rs:132`、`:134`、`:155` | 成立 |
| SE8 | S5／Imp#8 | 阻擋 | **非阻塞消費與核心同步未落地**：FfmpegStream、LiveSource 仍 `recv()`；沒有依 Playing 狀態計時的空讀處理。OutputProxy 只有 ID 檢查與 `None`，沒有核心鎖內再次驗證及取樣；換裝置直接原子改 ID。 | `src-tauri/src/audio_decode.rs:441`；`src-tauri/src/player.rs:934`、`:289`；`src-tauri/src/playback_pipeline.rs:819` | 成立 |
| SE9 | S5／Imp#8 | 阻擋 | **逾時回收仍遺失 handle**：cancel 取走 Child、忽略 kill 結果後釋放；回收只等 stdout 最多十秒便離開，沒有確認 join。Thread 分支直接無期限 join，Child 分支直接 wait；無失敗續追、孤兒紀錄或回收期間準備配額封鎖。 | `src-tauri/src/playback_pipeline.rs:915`；`src-tauri/src/player.rs:153`、`:157`、`:167`；`src-tauri/src/audio_decode.rs:429` | 成立 |
| SE10 | S6／Imp#9 | 阻擋 | **command manifest 只是未使用的常數**。build.rs 沒有呼叫 `AppManifest::commands`；main 只記錄數量，capability 未列自訂命令權限。須接入真正的 Tauri ACL。 | `src-tauri/build.rs:43`、`:47`；`src-tauri/src/main.rs:44`、`:48`；`src-tauri/capabilities/default.json:6` | 成立 |
| SE11 | S6／Imp#9 | 阻擋 | 電台已有協定白名單、redirect／timeout 參數，但**本機 ffmpeg／ffprobe 仍無 `file,pipe` 白名單**；電台未禁止巢狀播放清單，也沒有區網啟用檢查。 | `src-tauri/src/player.rs:459`、`:517`、`:607`；`src-tauri/src/audio_decode.rs:295`；`src-tauri/src/commands.rs:521`；D:75、232 | 成立 |
| SE12 | S6 | 次要 | 「CSP 仍為 null」已修正：目前設定具體 CSP。此項只確認設定，不能替代 command ACL 或執行期驗證。 | `src-tauri/tauri.conf.json:23` | 已修 |
| SE13 | S7／Imp#12 | 重要 | 啟動自動 HKCU 註冊已移除，後端註冊／取消命令與 API 包裝已加入；但**設定頁沒有呼叫鏈**，搜尋只找到包裝定義。且 reg 結果被忽略，command 固定成功。 | `src-tauri/src/main.rs:49`；`src/lib/api.ts:97`；`src-tauri/src/commands.rs:532`；`src-tauri/src/open_with.rs:348` | 成立 |
| SE14 | S7 | 阻擋 | **取消關聯會刪除整個副檔名登錄子樹**：對 `.mp3` 等執行 `reg delete HKCU\\Software\\Classes\\.mp3 /f`，未確認目前關聯是否仍屬 SonicLedger，可能刪除其他程式／使用者設定。 | `src-tauri/src/open_with.rs:371`、`:376`、`:379` | 成立 |

> `npx tauri build` 已成功（exe + MSI + NSIS 皆產出，只有警告無錯誤）。`cargo test --release` 與 `node --test` 均全過，但測試未涵蓋上述正式播放路徑與安全契約。AGENTS.md 的完整完成條件尚未驗證。
