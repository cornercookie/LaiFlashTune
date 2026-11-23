# 產生格式驗收用的音訊樣本（SP7）。
#
# 用法（PowerShell）：
#   .\scripts\make-format-fixtures.ps1
#
# 產出 src-tauri/tests/fixtures/formats/ 下各格式的 3 秒、1 kHz 立體聲正弦波樣本，
# 並寫入固定標籤，供單元/整合測試驗證掃描、解碼與 seek。
#
# 需求：本機 ffmpeg（PATH、exe 旁或 bin\）。APE 樣本需 Monkey's Audio 的 MAC.exe。

$ErrorActionPreference = 'Stop'
$PSScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$outDir = Join-Path $repoRoot 'src-tauri\tests\fixtures\formats'
$seconds = 3
$frequency = 1000
$channels = 2
$sampleRate = 44100

# 尋找 ffmpeg（exe 旁 -> bin\ -> PATH）
function Find-Ffmpeg {
    $candidates = @()
    # exe 旁（repo root / 組織後的執行檔目錄）、bin\、PATH
    $candidates += Join-Path $repoRoot 'ffmpeg.exe'
    $candidates += Join-Path $repoRoot 'src-tauri\target\release\ffmpeg.exe'
    $candidates += Join-Path $repoRoot 'bin\ffmpeg.exe'
    $candidates += 'ffmpeg.exe'
    foreach ($c in $candidates) {
        if (Test-Path $c) { return $c }
    }
    return $null
}

$ffmpeg = Find-Ffmpeg
if (-not $ffmpeg) {
    Write-Error '找不到 ffmpeg，請將 ffmpeg 放到 PATH 或專案 bin\ 目錄。'
    exit 1
}
Write-Host "使用 ffmpeg：$ffmpeg"

if (-not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir | Out-Null }

# 產生一個 3 秒 1 kHz 立體聲 WAV 做為所有編碼的來源（以 48kHz 輸出避免重取樣誤差）
$wavSrc = Join-Path $outDir '_tone.wav'
& $ffmpeg -y -hide_banner -loglevel error -f lavfi -i "sine=frequency=$frequency:sample_rate=$sampleRate:duration=$seconds" -ac $channels -ar $sampleRate $wavSrc

# ffmpeg 本機可編碼的格式（3 秒 1kHz 立體聲，寫入固定標題/藝術家標籤）
$formats = @(
    @{ ext='mp3';  args=@('-codec:a','libmp3lame','-b:a','192k') },
    @{ ext='mp2';  args=@('-codec:a','mp2','-b:a','128k') },
    @{ ext='aac';  args=@('-codec:a','aac','-b:a','128k') },
    @{ ext='m4a';  args=@('-codec:a','aac','-b:a','128k') },
    @{ ext='flac'; args=@('-codec:a','flac') },
    @{ ext='wav';  args=@('-codec:a','pcm_f32le') },
    @{ ext='ogg';  args=@('-codec:a','libvorbis','-b:a','128k') },
    @{ ext='opus'; args=@('-codec:a','libopus','-b:a','128k') },
    @{ ext='spx';  args=@('-codec:a','libspeex','-b:a','128k') },
    @{ ext='wv';   args=@('-codec:a','wavpack') },
    @{ ext='tta';  args=@('-codec:a','tta') },
    @{ ext='wmav2';args=@('-codec:a','wmav2','-b:a','128k') },
    @{ ext='ac3';  args=@('-codec:a','ac3','-b:a','192k') },
)

foreach ($f in $formats) {
    $out = Join-Path $outDir "tone.$($f.ext)"
    & $ffmpeg -y -hide_banner -loglevel error -i $wavSrc @($f.args) -metadata title='Fixtures Tone' -metadata artist='SonicLedger' $out
    if ($LASTEXITCODE -ne 0) { Write-Error "編碼 $($f.ext) 失敗" }
    Write-Host "產出 $out"
}

# APE 例外：本機 ffmpeg 沒有 APE 編碼器，改用 Monkey's Audio MAC.exe
$macCandidates = @(
    Join-Path $repoRoot 'MAC.exe',
    Join-Path $repoRoot 'bin\MAC.exe',
    'MAC.exe'
)
$macPath = $null
foreach ($c in $macCandidates) { if (Test-Path $c) { $macPath = $c; break } }
if ($macPath) {
    $apeOut = Join-Path $outDir 'tone.ape'
    & $macPath $wavSrc $apeOut -c2000
    if ($LASTEXITCODE -ne 0) { Write-Error 'MAC.exe 編碼 APE 失敗' }
    # 用 lofty 寫入標籤（需 dotnet 工具，此處僅產出音訊；標籤由測試補上或另行處理）
    Write-Host "產出 $apeOut（需另用 lofty 寫入標籤）"
} else {
    Write-Warning '找不到 MAC.exe，APE 樣本未產出（SP7：需 Monkey's Audio MAC.exe）。'
}

# 移除臨時 WAV
if (Test-Path $wavSrc) { Remove-Item $wavSrc }

Write-Host '樣本產出完成。'
