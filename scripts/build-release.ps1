# MediaTranscriber 배포 빌드
#
#   powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 [-SkipTests]
#
# 1. (선택) cargo test
# 2. cargo build --release  (CRT 정적 링크, whisper.cpp 최적화 — .cargo\config.toml)
# 3. Inno Setup 으로 설치 프로그램 생성 → dist\MediaTranscriber-Setup-<버전>.exe
#
# 필요: Rust(MSVC), Visual Studio Build Tools, CMake, LLVM(libclang), Inno Setup 6

param([switch]$SkipTests)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# winget 으로 설치한 CMake 등은 새 터미널 전까지 PATH 에 없을 수 있다
$links = Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Links"
if ((Test-Path $links) -and -not ($env:PATH -split ';' -contains $links)) { $env:PATH = "$links;$env:PATH" }

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "MediaTranscriber $version 배포 빌드"

if (-not $SkipTests) {
    cargo test
    if ($LASTEXITCODE -ne 0) { throw "테스트 실패" }
}

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "release 빌드 실패" }

$iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) 를 찾을 수 없습니다. winget install JRSoftware.InnoSetup" }

& $iscc /Q "/DMyAppVersion=$version" "installer\MediaTranscriber.iss"
if ($LASTEXITCODE -ne 0) { throw "설치 프로그램 생성 실패" }

$setup = Join-Path $root "dist\MediaTranscriber-Setup-$version.exe"
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
"$hash  MediaTranscriber-Setup-$version.exe" | Out-File -Encoding ascii (Join-Path $root "dist\SHA256SUMS.txt")
Write-Host ("완료: {0} ({1:N1} MB)" -f $setup, ((Get-Item $setup).Length / 1MB))
Write-Host "SHA-256: $hash"
