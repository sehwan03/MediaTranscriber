<p align="center"><img src="assets/icon-256.png" width="96" alt="MediaTranscriber"></p>

# MediaTranscriber

동영상·음성 파일에서 말소리를 텍스트(TXT)로 추출하는 Windows 데스크톱 프로그램입니다.
Rust + egui 로 만들었고, **내 PC에서(Local)** 또는 **인터넷 서비스로(Cloud)** 인식할 수 있습니다.

## 주요 기능

- **입력**: MP4 · MKV · AVI · MOV · WEBM · MP3 · WAV · M4A · FLAC · AAC · OGG (확장자가 아니라 실제 내용을 검사)
- **URL 변환**: 강의·영상 페이지 · m3u8(HLS) · MP4 직접 주소 등 [yt-dlp](https://github.com/yt-dlp/yt-dlp) 지원 사이트에서
  영상을 찾아 목록으로 보여주고, **사용자가 고른 영상만** 오디오를 받아 변환 (영상 전체를 저장하지 않음).
  yt-dlp 가 모르는 대학 강의 플레이어(Xinics 커먼즈) 페이지도 플레이어 설정에서 영상을 찾아 처리
- **영상 다운로드** (선택 기능, 설정에서 켜기): URL 변환에서 고른 영상을 최고 화질(가장 좋은 영상 + 가장 좋은 소리, 재인코딩 없음)로 mp4 또는 mkv 저장
- **Local**: [whisper.cpp](https://github.com/ggml-org/whisper.cpp) 기반, 인터넷 없이 동작 (모델 Tiny / Base / Small / Medium)
- **Cloud**: OpenAI (`gpt-transcribe` 등) 또는 Google Gemini (`gemini-3.5-flash-lite` 등), 본인 API Key 사용
- **Auto**: PC 성능(CPU·RAM·GPU·디스크)과 실제 측정 속도를 바탕으로 Local / Cloud 와 모델을 추천
- **일괄 변환**: 여러 파일·폴더·드래그 앤 드롭, 파일별 진행률·취소, 실패해도 다음 파일 계속
- **출력**: UTF-8 TXT (원본 이름 기반, 기존 파일 덮어쓰지 않음)
- 한국어 · 영어 (자동 감지 지원)

## 설치

1. [Releases](../../releases) 에서 `MediaTranscriber-Setup-<버전>.exe` 를 받아 실행합니다.
   관리자 권한 없이 현재 사용자용으로 설치됩니다.
2. 처음 실행하면 필요한 구성 요소를 안내합니다.
   - **FFmpeg**: 「FFmpeg 자동 설치」 (약 170 MB, LGPL 빌드, SHA-256 검증 후 앱 데이터 폴더에 설치)
   - **Whisper 모델**: 설정 → 모델 관리에서 다운로드 (Base 148 MB 권장, SHA-256 검증)
   - **yt-dlp** (URL 변환을 쓸 때만): URL 변환 화면의 「설치」 또는 설정 → URL 변환 구성 요소 (약 23 MB, 공식 릴리스, SHA-256 검증)

설치 프로그램 무결성은 Releases 의 `SHA256SUMS.txt` 와 비교해 확인할 수 있습니다.

```powershell
Get-FileHash .\MediaTranscriber-Setup-0.2.0.exe -Algorithm SHA256
```

## 사용법

1. 「파일 선택」·「폴더 선택」 또는 창으로 끌어다 놓기
2. 모드(Auto / Local / Cloud)·모델·언어 선택
3. 「변환 시작」 → 완료되면 결과 영역에서 「열기」·「폴더에서 보기」

### URL 변환

1. 「URL 변환」(기본 화면)에서 영상 주소(강의 페이지 · `.m3u8` · `.mp4` 등)를 입력 → 「영상 찾기」
2. 로그인이 필요한 페이지는 **로그인**을 지정합니다.
   - 「브라우저」: 브라우저(Edge · Chrome · Firefox · Whale · Brave)에 로그인된 세션을 yt-dlp 가 직접 읽습니다.
     Chrome·Edge 는 실행 중이면 쿠키를 읽지 못할 수 있으니 브라우저를 닫고 시도하거나 Firefox 를 사용하세요.
   - 「쿠키 파일」: 확장 프로그램 등으로 내보낸 Netscape 형식 cookies.txt
3. 찾은 영상(제목 · 길이 · 형식 · 해상도)에서 변환할 영상을 체크 → 「선택한 영상 변환」
   (여러 개가 나와도 자동으로 고르거나 변환하지 않습니다)
4. 오디오만 받아 16 kHz mono WAV 로 바꾼 뒤 기존 Local / Cloud 인식으로 변환하고, TXT 이름은 영상 제목을 씁니다.
   임시 오디오는 성공 · 실패 · 취소 · 종료 시 모두 지워집니다.

본인에게 접근 권한이 있는 영상에만 사용하세요. DRM 으로 보호된 영상은 처리하지 않으며 접근 제한을 우회하지 않습니다.

## 개인정보와 보안

- **API Key** 는 설정 파일·로그·소스 코드에 기록되지 않습니다.
  기본은 메모리에만 두고, 「이 PC에 기억」을 선택한 경우에만 **Windows 자격 증명 관리자**에 저장됩니다.
  설정 화면이나 제거 프로그램에서 삭제할 수 있습니다.
- **Cloud 전송 전에는 매번 동의**를 받습니다 (확인 체크 후에만 전송).
- Cloud 처리 중 인터넷이 끊겨도 다른 방식으로 몰래 전환하지 않습니다.
- **Google Gemini 무료 등급**: 보낸 음성과 결과가 Google 제품 개선에 사용되고 사람이 검토할 수 있습니다.
  또한 무료 등급은 EEA·영국·스위스 사용자 대상 서비스에 사용할 수 없습니다
  ([약관](https://ai.google.dev/gemini-api/terms)). 민감한 내용은 Local 모드를 사용하세요.
- 로그에는 API Key 와 인식 텍스트를 남기지 않습니다.
- **URL 변환**: 쿠키·로그인 정보는 앱이 읽거나 저장하지 않습니다 (브라우저 이름 또는 cookies.txt **경로**만 설정에 저장하고 yt-dlp 에 넘김).
  로그 · 오류 메시지의 주소는 `?` 뒤(토큰·서명)를 지워 기록합니다. 외부 프로그램은 셸 없이 인자를 분리해 실행하고,
  주소는 항상 `--` 뒤에 두어 옵션으로 해석될 수 없게 합니다. http/https 주소만 받습니다.

## 데이터 위치

| 항목 | 경로 |
|---|---|
| 설정 | `%APPDATA%\MediaTranscriber\config\settings.json` |
| 모델 · FFmpeg · yt-dlp · 로그 · 임시 오디오 | `%LOCALAPPDATA%\MediaTranscriber\data\` |
| 기본 출력 폴더 | `문서\MediaTranscriber` |

문제가 생기면 설정 → 「로그 폴더 열기」의 로그(최근 14일)를 확인하세요.

## 소스에서 빌드

필요: Rust (MSVC, stable), Visual Studio Build Tools (C++), CMake, LLVM (libclang), Inno Setup 6 (설치 프로그램 생성 시)

```powershell
winget install Kitware.CMake LLVM.LLVM JRSoftware.InnoSetup
```

```powershell
cargo run --release
```

설치 프로그램 만들기 (`dist\` 에 생성):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
```

- C 런타임은 정적 링크되어 VC++ 재배포 패키지가 필요 없습니다 (`.cargo/config.toml`).
- release 빌드에서 whisper.cpp 최적화가 빠지는 cmake 크레이트 동작을 `.cargo/config.toml` 에서 보정합니다.
- 테스트: `cargo test` (FFmpeg·모델·Windows TTS 가 있으면 실제 인식까지 검증, 없으면 해당 테스트는 건너뜀)

## 프로젝트 구조

```
src/
├── app/             GUI 상태와 화면 조립
├── ui/              파일 · 작업 목록 · 결과 · 설정 패널
├── config/          설정 (settings.json), API Key 보관 (secret.rs)
├── media/           FFmpeg 탐색 · 검사 · 디코딩 · 자동 설치
├── speech/          Local(whisper) · Cloud(OpenAI/Gemini) 엔진, 모델 관리
├── engine_selector/ Auto 모드 성능 분석과 추천
├── system/          CPU · RAM · GPU · 디스크 정보
├── task/            작업 관리자 (Local 1개 / Cloud N개 동시 처리), 파일·URL 입력 준비
├── url/             URL 변환: yt-dlp 탐색 · 오디오 추출 · 자동 설치 · 주소 검증/가리기
├── output/          TXT 저장
└── utils/           오류 · 로그 · 임시 파일 가드
```

## 라이선스

다음 두 라이선스 중 하나를 선택해 사용할 수 있습니다 (Rust 생태계 표준 이중 라이선스).

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

별도로 밝히지 않는 한, 이 프로젝트에 기여한 내용은 위와 같은 이중 라이선스로 제공되는 것으로 간주합니다.

사용한 오픈 소스 구성 요소는 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 를 참고하세요.
URL 변환이 지원하는 영상 출처와 한계는 [docs/url-sources.md](docs/url-sources.md) 에 정리되어 있습니다.
