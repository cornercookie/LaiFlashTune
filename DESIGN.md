---
name: SonicLedger
description: 本地可攜播放器，曲目表印在黑托盤卡上。
colors:
  tray: "#12110f"
  ink: "#f4f0e8"
  catalog: "#e23b2f"
  muted: "#8a847a"
  line: "#2a2723"
  row: "#1c1b18"
typography:
  display:
    fontFamily: "Microsoft JhengHei, PingFang TC, Segoe UI, sans-serif"
    fontSize: "32px"
    fontWeight: 700
    lineHeight: 1
  body:
    fontFamily: "Microsoft JhengHei, PingFang TC, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.4
  label:
    fontFamily: "Microsoft JhengHei, PingFang TC, Segoe UI, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    letterSpacing: "0.12em"
rounded:
  control: "17px"
  field: "0px"
spacing:
  rail: "180px"
  margin: "240px"
  header: "96px"
  row: "30px"
components:
  button-transport:
    backgroundColor: "{colors.row}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    width: "34px"
    height: "34px"
  input-field:
    backgroundColor: "{colors.row}"
    textColor: "{colors.ink}"
    rounded: "{rounded.field}"
    padding: "8px 10px"
---

# Design System: SonicLedger

## Overview

**Creative North Star: "封底曲目"**

播放器是一張 CD 封底，不是一排專輯卡片。曲目表佔主區，時間右對齊，目錄號與總時長永遠印在角落。紅色只用來標目錄號、正在播的編號，以及角落的總時長。

密度服務選歌。同一套中文字，清單小，正在播放的歌名只大一級。沒有接上引擎的控制項不出現。

**Key Characteristics:**

- 黑托盤底，白油墨字，紅色目錄號
- 播放清單是主工作區，多欄、可排序、虛擬滾動
- 左側是導覽，右側是歌詞或等化器
- 角落永遠印著目前清單的總時長與曲數

## Colors

黑底白字，紅色是目錄油墨，不是裝飾光暈。

### Primary

- **目錄紅** (#e23b2f): 目錄號、正在播放的編號、角落總時長、等化器讀數。少用，用了就要是資料。

### Neutral

- **托盤黑** (#12110f): 整頁底。
- **白油墨** (#f4f0e8): 歌名與主要文字。
- **油墨灰** (#8a847a): 歌手、時間、未播放列的次要欄。
- **格線** (#2a2723): 欄線與分隔。
- **列底** (#1c1b18): 正在播放的列、按鈕與輸入框。

**The Catalog Ink Rule.** 紅色只標編號、總時長和錯誤。不用來做大面積背景。

## Typography

**Display Font:** Microsoft JhengHei（PingFang TC、Segoe UI 後援）
**Body Font:** 同一套
**Label/Mono Font:** 同一套，數字用 tabular-nums

**Character:** 一套中文字。清單是曲目表，歌名在頂部只大一級，不換另一套展示字體。

### Hierarchy

- **Display** (700, 32px, line-height 1): 正在播放的歌名。
- **Title** (700, 22–26px): 工具頁標題與歌詞歌名。
- **Body** (400, 14px): 曲目列。
- **Label** (400, 13px, tracking 0.12em): 目錄號 SL-000。

**The One Face Rule.** 不為了「播放器感」再加一套英文字體。

## Layout

頂部 96px 是封底上緣：目錄號、歌名、圓形傳輸鈕、細進度線、音量，右上角總時長壓在曲數上。左側 180px 導覽。中間曲目表。右側 240px 歌詞或 10 段等化器。列高 30px。曲庫超過一頁時虛擬滾動，不一次畫完。

## Elevation & Depth

沒有陰影。層次靠格線和正在播放列的深一階底色。

**The Flat Tray Rule.** 封底是平的。不要玻璃、光暈、浮起的卡片。

## Shapes

傳輸鈕是 34px 圓。輸入框直角，1px 格線。進度是 3px 線，拇指 10px 紅點。

## Components

### Buttons

- **Shape:** 圓形傳輸鈕（17px 半徑，34px）
- **Primary:** 列底 #1c1b18，字 #f4f0e8，1px #3a3632 框
- **Hover / Focus:** 框變目錄紅；鍵盤焦點是 1px 紅外框

### Cards / Containers

不用卡片裝歌。正在播放的列只換底色 #1c1b18。

### Inputs / Fields

- **Style:** 直角，底 #1c1b18，框 #3a3632，字 14px
- **Focus:** 1px 目錄紅外框
- **Error:** 紅字寫在欄位下方，說明缺什麼、怎麼補

### Navigation

左側文字列表，選中用白油墨，未選用油墨灰。沒有圖示牆。

### 封底角落

右上角總時長用目錄紅 22px，下面是曲數。這是這套系統的簽名，不是統計卡片。

## Do's and Don'ts

### Do:

- **Do** 讓曲目表佔主區，時間右對齊，編號用目錄紅標出正在播放的那列。
- **Do** 把總時長和曲數印在右上角。
- **Do** 等化器、轉檔、電台若做不到，就寫明做不到，不要留死推桿。

### Don't:

- **Don't** 用專輯封面牆或同一尺寸的卡片當曲庫。
- **Don't** 加第二套展示字體或表情符號當圖示。
- **Don't** 畫 20 段不能動的等化器。實際是 10 段，而且只作用於本地檔案。
