# URL 변환: 영상 출처별 지원 현황

조사일 2026-11-20 · yt-dlp 2026.10.26 기준. 「확인됨」은 이 프로젝트에서 실제로 돌려 본 것, 「문서」는 yt-dlp 소스·이슈·공식 문서로만 확인한 것이다.

## 앱의 영상 찾기 순서

1. **yt-dlp** — 추출기 1,755개. 일반 페이지에서도 `<video>`/`<source>`, JW Player 설정, JSON-LD, og:video, 알려진 플레이어 iframe(YouTube · Vimeo · Kaltura · Panopto 등)을 스스로 찾는다.
2. **403 · Cloudflare · 429** 로 거부되면 `--impersonate chrome`(브라우저 요청 흉내, exe 에 curl_cffi 포함)으로 한 번 더.
3. 그래도 없으면 **페이지를 직접 확인** (`src/url/page.rs`, 로그인 쿠키 없이):
   1. 강의 플레이어 설정 — Xinics 커먼즈 `content.php` XML → MP4 (`src/url/xinics.rs`)
   2. 페이지·스크립트·JSON 안의 `.mp4` / `.m3u8` 주소 (`\/`, `&`, `&amp;` 이스케이프 해제)
   3. `<iframe>` 최대 6개: 각각 yt-dlp(바깥 페이지를 Referer 로) → 위 1·2 한 번 더
   4. 아무것도 없고 DRM·보안 플레이어 흔적(Kollus, Widevine, PallyCon, INKA, PlayReady, FairPlay)이 있으면 「DRM 으로 처리 불가」로 안내 (우회하지 않음)
4. 받을 때: 찾을 때와 같은 Referer·impersonate 옵션을 쓴다. 직접 찾은 미디어는 FFmpeg 가 주소를 직접 읽는다
   (구간 요청, 영상 저장 없음, `-protocol_whitelist http,https,tcp,tls,crypto`).

공통 안전 장치: 페이지에서 얻은 주소는 http/https · 검증 통과 · 공개 페이지에서 내부망 주소 금지, 외부 프로그램에는 인자로만(`--` 뒤), 로그에는 쿼리를 지운 주소만.

## 출처별 정리

| 출처 | 지원 | 전달 방식 | 로그인·조건 | 상태 |
|---|---|---|---|---|
| MP4 직접 주소 | yt-dlp generic | progressive MP4 | — | 확인됨 (색인이 끝에 있는 MP4 는 임시 파일 대체 경로) |
| m3u8 (HLS) | yt-dlp generic | HLS, AES-128 가능 | 서명 토큰은 주소에 포함 | 확인됨 |
| 영상 여러 개인 페이지 | yt-dlp generic | `<video>` 목록 | — | 확인됨 |
| **Xinics 커먼즈 (UniPlayer, Canvas LearningX)** | 앱 자체 처리 | `content.php` XML → `*.commonscdn.com` MP4 | 페이지 출처를 Referer 로 (없으면 403) | 확인됨 (아주대 실제 강의, iframe 안 포함) |
| 스크립트·JSON 설정 안의 mp4/m3u8 | 앱 자체 처리 | MP4 / HLS | 페이지 출처를 Referer 로 | 확인됨 (로컬 재현) |
| iframe 안의 일반 영상 페이지 | yt-dlp + Referer | — | 바깥 페이지 Referer | 확인됨 (로컬 재현) |
| YouTube | `youtube` | DASH/HLS | JS 런타임 필요 (deno 기본, 없으면 경고·일부 형식 누락). 앱은 deno 가 없고 Node.js 22+ 가 있으면 `--js-runtimes node` 사용 | 확인됨 |
| Vimeo (비공개 임베드) | `vimeo` | HLS/DASH | 임베드 전용 영상은 임베드한 페이지가 Referer 로 필요 → iframe 경로가 자동으로 넘김 | 문서 |
| Panopto | `Panopto`, `PanoptoList` | HLS/MP4 (`DeliveryInfo.aspx`) | 비공개는 쿠키 필요. `--cookies-from-browser` 가 안 되는 이슈 있음(#17561) → **쿠키 파일 권장** | 문서 |
| Kaltura | `Kaltura` | HLS/MP4 | 대개 공개. Canvas 연동(KAF)은 세션 필요할 수 있음 | 문서 |
| Mediasite | `Mediasite`, `MediasiteCatalog` | HLS/MP4 | 비공개는 쿠키 | 문서 |
| Opencast | `Opencast` | — | 정해진 기관 목록만 인식 | 문서 |
| Blackboard Collaborate 녹화 | `BlackboardCollaborate(Launch)` | — | launch 주소의 토큰 | 문서 |
| MS Stream / SharePoint | `SharePoint` | — | 세션 쿠키 필수, **브라우저 쿠키 읽기 불가 → 쿠키 파일만** (#9904, #11318) | 문서 |
| Zoom 녹화 | `zoom`, `zoom:clips` | — | 암호 필요 시 `--video-password` (앱은 아직 암호 입력 없음) | 문서 |
| Google Drive · Box · Dropbox · 네이버 · 카카오 · SOOP · 치지직 | 각 추출기 | — | 공유 설정에 따름 | 문서 |
| **Kollus (카테노이드)** | 없음 | JWT 서명 주소, Kollus DRM · Multi-DRM | — | **처리 불가 → DRM 안내** (확인됨: 로컬 재현) |
| Echo360 · YuJa · 위캔디오 | 전용 추출기 없음 (Echo360 PR #6503 미병합) | 미확인 | 미확인 | 페이지 직접 확인(2·3단계)에 걸리면 동작, 아니면 미지원 |

## 알려진 한계

- **로그인이 필요한 페이지의 직접 확인(3단계)** 은 쿠키 없이 요청한다. yt-dlp 단계는 브라우저·쿠키 파일을 쓰지만, 앱이 쿠키를 읽지 않는다는 원칙 때문에 3단계에는 넘기지 않는다. 로그인 뒤에만 보이는 플레이어 페이지는 강의 영상의 **플레이어 주소(iframe 주소)를 직접 붙여넣는** 것이 가장 확실하다.
- Chrome·Edge 브라우저 쿠키는 브라우저가 실행 중이거나 쿠키 암호화 때문에 못 읽을 수 있다 → Firefox 또는 쿠키 파일.
- 이 문서의 「문서」 항목은 실제 계정으로 시험하지 않았다. 사이트 구조가 바뀌면 설정 → 「최신 버전으로 업데이트」로 yt-dlp 를 갱신한다.

## 참고

- yt-dlp 지원 사이트 목록: https://github.com/yt-dlp/yt-dlp/blob/master/supportedsites.md
- JS 런타임(EJS): https://github.com/yt-dlp/yt-dlp/wiki/EJS
- FAQ (쿠키·Cloudflare): https://github.com/yt-dlp/yt-dlp/wiki/FAQ
- Panopto 쿠키 이슈: https://github.com/yt-dlp/yt-dlp/issues/17561 · Echo360: https://github.com/yt-dlp/yt-dlp/issues/3966
- Kollus DRM: https://docs.kollus.com/en/service-guide/vod/security/drm/
- 커먼즈 플레이어 구조 참고(숭실대 확장 프로그램): https://github.com/hack-soongsil/hacklms
