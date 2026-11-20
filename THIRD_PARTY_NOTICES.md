# 제3자 소프트웨어 고지 (Third-Party Notices)

MediaTranscriber 는 아래 오픈 소스 소프트웨어를 사용합니다.

## 실행 파일에 포함 (정적 링크)

| 구성 요소 | 용도 | 라이선스 |
|---|---|---|
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) / ggml (whisper-rs, whisper-rs-sys 경유) | 로컬 음성 인식 | MIT |
| [egui / eframe](https://github.com/emilk/egui) | GUI | MIT OR Apache-2.0 |
| [reqwest](https://github.com/seanmonstar/reqwest), hyper, rustls | HTTP / TLS | MIT OR Apache-2.0 (rustls: Apache-2.0 OR ISC OR MIT) |
| [aws-lc-rs / aws-lc-sys](https://github.com/aws/aws-lc-rs) (rustls 암호화 백엔드) | TLS 암호화 | Apache-2.0 AND ISC (및 OpenSSL 계열 고지 포함) |
| [sysinfo](https://github.com/GuillaumeGomez/sysinfo) | 시스템 정보 | MIT |
| [windows-rs](https://github.com/microsoft/windows-rs) | Win32 API | MIT OR Apache-2.0 |
| [rfd](https://github.com/PolyMeilleur/rfd) | 파일 대화상자 | MIT |
| [serde](https://github.com/serde-rs/serde), serde_json | 설정 저장 | MIT OR Apache-2.0 |
| [tracing](https://github.com/tokio-rs/tracing) 계열 | 로그 | MIT |
| [sha2](https://github.com/RustCrypto/hashes), [base64](https://github.com/marshallpierce/rust-base64), [zeroize](https://github.com/RustCrypto/utils), [zip](https://github.com/zip-rs/zip2) | 해시·인코딩·압축 해제 | MIT OR Apache-2.0 |
| [directories](https://crates.io/crates/directories), [thiserror](https://github.com/dtolnay/thiserror), [anyhow](https://github.com/dtolnay/anyhow) | 유틸리티 | MIT OR Apache-2.0 |

전체 의존성 목록과 정확한 라이선스는 `Cargo.lock` 에 있으며, 다음 명령으로 확인할 수 있습니다.

```powershell
cargo install cargo-about
cargo about generate about.hbs > licenses.html
```

## 실행 중 사용자 요청으로 내려받는 구성 요소 (설치 파일에 포함되지 않음)

### FFmpeg (LGPL)

- 출처: [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) — FFmpeg 공식 다운로드 페이지가 안내하는 Windows 빌드
- 구성: `win64-lgpl` (GPL 라이브러리 미포함), `ffmpeg.exe`, `ffprobe.exe`, `LICENSE.txt` 만 설치
- 라이선스: GNU LGPL v2.1 이상 — 설치 폴더의 `LICENSE.txt` 참고
- 소스 코드: <https://ffmpeg.org/download.html#get-sources>, 빌드 스크립트: <https://github.com/BtbN/FFmpeg-Builds>
- MediaTranscriber 는 FFmpeg 를 별도 프로세스로 실행하며 링크하지 않습니다.

### yt-dlp (Unlicense)

- 출처: [yt-dlp/yt-dlp 공식 릴리스](https://github.com/yt-dlp/yt-dlp/releases) — `yt-dlp.exe` 와 `SHA2-256SUMS` 로 무결성 확인
- 용도: URL 변환에서 영상 탐색과 선택한 영상의 스트림 받기 (사용자가 「yt-dlp 자동 설치」를 누를 때만 내려받음)
- 라이선스: [The Unlicense](https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE) (퍼블릭 도메인).
  Windows 실행 파일에는 Python 런타임 등 번들 구성 요소가 포함되며 각자의 라이선스를 따릅니다
  ([THIRD_PARTY_LICENSES](https://github.com/yt-dlp/yt-dlp/blob/master/THIRD_PARTY_LICENSES.txt)).
- MediaTranscriber 는 yt-dlp 를 별도 프로세스로 실행하며 링크하지 않습니다.

### Whisper 모델 (ggml 형식)

- 출처: [ggerganov/whisper.cpp (Hugging Face)](https://huggingface.co/ggerganov/whisper.cpp)
- 원본: [OpenAI Whisper](https://github.com/openai/whisper) — MIT 라이선스

## 외부 서비스

Cloud 모드는 사용자가 입력한 API Key 로 OpenAI 또는 Google Gemini API 를 호출합니다.
각 서비스의 이용 약관과 데이터 정책이 적용됩니다.

- OpenAI: <https://openai.com/policies>
- Google Gemini API: <https://ai.google.dev/gemini-api/terms> — 무료 등급(Unpaid)으로 보낸 내용은
  Google 제품 개선에 사용되고 사람이 검토할 수 있습니다.

URL 변환은 사용자가 입력한 주소의 사이트에 접속합니다. 해당 사이트의 이용 약관과 저작권 정책은 사용자가 준수해야 합니다.
