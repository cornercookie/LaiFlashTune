<!-- 本文件為規劃文件，不完全是實作依據。 -->

# [#22]P3-4-鬧鐘與排程

## Description
現況只有睡眠計時（N 分鐘後暫停，或這首播完就停），只在視窗開著時有效。需要新表 scheduled_tasks，支援一次性／每天／每週／每月／每年／一天內每隔 N 分鐘／事件觸發，動作包括下一首、播放指定曲目或歌單、暫停／繼續、電台錄音啟停、調整音量（可漸變）、播完 N 首後回到前一個歌單、讓電腦休眠或睡眠、關機、關閉程式。鬧鐘播放指定歌單，音量由小漸大。要從睡眠中喚醒電腦，用 SetWaitableTimer 的喚醒旗標；程式關掉時就不會響，畫面上要寫清楚。

## User Story
作為使用者，設一個 2 分鐘後的鬧鐘，讓電腦睡眠，時間到電腦被喚醒，音樂由小聲漸大；設定關掉重開後還在；關機任務執行前有 60 秒可以取消。

## To Reproduce
不適用（非 bug）。

## Expected behavior
- 新表 scheduled_tasks(id, name, enabled, trigger_json, action_json, last_run)；
- 觸發：一次性；每天、每週、每月、每年；一天內每隔 N 分鐘；事件（例如目前歌單再播 N 首後）；
- 動作：下一首；播放指定曲目或歌單；暫停、繼續；開始或停止電台錄音；調整音量（可以漸變）；播完 N 首後回到前一個歌單；讓電腦休眠或睡眠；關機；關閉程式；
- 鬧鐘：播放指定歌單，音量由小漸大。要從睡眠中喚醒電腦，就用 SetWaitableTimer 的喚醒旗標；程式關掉時就不會響，畫面上要寫清楚。要做到「程式關掉也會響」就要用工作排程器，那可能需要權限，先做小實驗；
- 睡眠計時併進排程，成為一種一次性任務；
- 關機、休眠前先寫入工作階段（沿用關閉時的寫入流程）。

## Acceptance Criteria
- [ ] 設一個 2 分鐘後的鬧鐘，讓電腦睡眠，時間到電腦被喚醒，音樂由小聲漸大；
- [ ] 設定關掉重開後還在；
- [ ] 關機任務執行前有 60 秒可以取消；
- [ ] `cargo test --release --manifest-path src-tauri/Cargo.toml` 全過；
- [ ] `npx tauri build` 成功。

## Additional context
- 相關檔案：`src-tauri/src/db.rs`、`src-tauri/src/commands.rs`、`src/player/Shell.tsx`
- 設計來源：planning/P3-tools/Design.md 第 4 項
