# Phase 2 검증 범위

이 문서는 자동 테스트가 확인하는 사실과 실제 Mac에서 별도로 확인해야 하는 항목을 구분한다. 체크리스트는 실행 증거가 아니며, 실제 점검하지 않은 항목을 통과로 기록하지 않는다.

## PRD와 구현 대응

| PRD 요구 | 구현 | 자동 검증 |
| --- | --- | --- |
| `rec test`: command, exit code, duration | 호출 CLI의 cwd/env에서 argv 실행; 시작·완료 이벤트 | 종료 0/7, 실패 127, 타임아웃 124, 인자·환경 전달 |
| stdout/stderr summary | 스트림별 끝 4 KiB 보존, 영상 요약 길이 제한 | 양쪽 스트림, 큰 출력, Unicode/제어문자 |
| Exit 0을 PASS로 해석하지 않음 | TEST 시작 시 이전 verdict 제거, 결과에 판정 없음 | Rust 계약 테스트, 캡처 프로토콜 통합 테스트 |
| 시스템 오디오, 기본 OFF, 마이크 없음 | ScreenCaptureKit audio → AAC; 명시적 옵션 | 옵션 파싱, AAC 합성 인코딩, 무음 트랙 검사, 누락 시 최종화 거부 |
| MP4 chapters | Setup + TestStart + Checkpoint | 실제 FFmpeg/FFprobe 왕복, 제목·경계·특수문자 보존 |
| application capture | bundle ID/정확한 이름, 주 모니터의 앱 포함 필터 | 파싱·상호 배제·인자 전달·실패 시 대체 금지 |
| MP4 하나로 제출 | 원본·메타데이터는 내부 파일, 검증 후 게시 | 덮어쓰기 방지, 최종 파일 검사, 임시 Run 정리 |

테스트 1회는 `Tests` 집계에서 1건이며 시작·완료 각각 Step을 만든다. 시작 이벤트만 챕터 경계가 된다. 이 규칙은 자동 테스트와 README에 명시한다.

## 자동 검증 계층

### Rust 단위 테스트와 미디어 왕복

```bash
cargo test --all-targets
```

`media` 테스트는 실제 FFmpeg로 H.264/AAC 합성 파일을 만들고 최종화 경로로 챕터를 추가한다. FFprobe로 코덱·길이·챕터 제목·경계를 확인한다. 파일을 만들지 않고 단순히 명령 문자열만 확인하는 테스트가 아니다.

### Swift 상태 모델 및 네이티브 writer

```bash
swift test --package-path macos/RecCapture
swift build -c release --package-path macos/RecCapture
macos/RecCapture/.build/release/rec-capture self-test \
  --output ./synthetic-video-only.mp4
macos/RecCapture/.build/release/rec-capture self-test \
  --output ./synthetic-with-audio.mp4 --system-audio
```

합성 테스트는 실제 녹화와 같은 `MediaWriter`에 BGRA 프레임과 48 kHz PCM을 입력한다. H.264 및 선택적 AAC 인코딩과 finalization을 검증하되 ScreenCaptureKit이나 macOS 권한 허용 여부를 검증하지는 않는다. 합성 영상에는 `Synthetic codec test — NOT a screen recording`이 표시된다.

### CLI → IPC → 데몬 → MP4 통합

```bash
cargo build
python3 tests/e2e.py
```

이 테스트는 실제 `rec` 바이너리를 사용하되 화면 캡처 helper만 테스트 파일 안의 합성 구현으로 교체한다. HOME과 TMPDIR를 격리하며 사용자의 실제 세션이나 화면을 사용하지 않는다.

검증 대상은 세션 시작/종료, 앱·오디오 인자 전달, 테스트 실행 환경, 동시 테스트·조기 stop 거부, 다른 Run 결과 거부, 이벤트 시간, 출력 상한, 오디오 누락 실패, 챕터 보존, 덮어쓰기 방지 및 정리다.

`artifacts/`의 테스트 보고서·프로토콜·합성 MP4는 CI artifact로 보관한다. 실제 사용자가 만든 녹화는 CI에서 읽거나 전송하지 않는다.

## 실제 Mac에서 확인할 항목 — 자동 통과로 간주하지 않음

필요 환경은 로그인된 macOS 14+ 데스크톱, 주 모니터, Safari 등 여러 창을 열 수 있는 앱, 재생 가능한 소리, Screen Recording 권한을 제어할 수 있는 계정이다. 테스트 명령은 대상 프로젝트에 맞는 무해한 명령을 사용한다.

| 항목 | 절차 | 수락 기준 |
| --- | --- | --- |
| 권한 거부 | 터미널의 화면 녹화 권한을 끄고 앱 캡처 시작 | 권한 안내와 실패; 전체 화면 대체 녹화 없음 |
| 권한 허용 | 권한 부여 후 터미널 재실행, 앱 캡처 | 실제 앱 프레임이 기록됨 |
| 화면 필터 | 주 모니터 전체 캡처와 창 ID 캡처를 각각 실행 | 요청한 범위만 영상에 포함 |
| 앱 필터 | 선택 앱에 두 창을 열고 다른 앱을 나란히 표시 | 선택 앱의 창들만 포함; 다른 앱 내용이 보이지 않음 |
| 새 창 | 앱 캡처 시작 후 같은 실행 중 앱에서 새 창 생성 | 같은 주 모니터의 새 창이 필터에 포함됨 |
| OS 파일 선택 창 | 실제 브라우저의 file input을 눌러 기본 파일 창에서 검증 파일 선택; 앱·단일 창 모드를 각각 확인 | 최종 MP4에 파일 창·선택 파일명·브라우저 선택 결과가 포함; 앱/OS/모니터별로 결과 기록 |
| 파일 직접 지정 자동화 | 브라우저 자동화 API로 검증 파일 지정 | OS 창이 실제로 열렸는지 구분; 열리지 않았다면 직접 지정 경로와 결과 UI를 기록 |
| 대상 소실 | 창 닫기 또는 선택 앱 종료 | 대상 소실 표시, 다른 앱으로 전환하지 않음, 명시적 stop 가능 |
| 모니터 경계 | 선택 앱 창을 보조 모니터로 이동 | 다른 모니터를 자동 추적·확대하지 않음 |
| 오디오 기본값 | 소리를 재생하며 옵션 없이 녹화 | 최종 MP4에 오디오 트랙 없음 |
| 오디오 선택 | `--system-audio`로 소리가 있는 앱 녹화 | AAC 트랙과 실제 소리가 존재, 영상과 자연스럽게 동기화 |
| 창 오디오 범위 | 동일 앱의 두 창 중 하나를 영상 대상으로 선택하고 다른 창에서 소리 재생 | 앱 단위 오디오 범위가 문서와 일치함을 확인 |
| 무음 입력 | 오디오 옵션을 켠 채 소리 없는 상태에서 녹화 | 받은 오디오 샘플/트랙 여부에 따라 명시적으로 처리; 샘플이 없는데 성공을 주장하지 않음 |
| 마이크 제외 | 시스템 소리 없이 마이크 앞에서 말하며 녹화 | 마이크 음성이 포함되지 않음 |
| 정지 화면 주석 | 화면 조작 없이 note/expect/test result를 순서대로 입력 | 정지된 화면에서도 Run/Step과 카드가 갱신됨 |
| 최종 카드 | checkpoint 직후 stop | 카드 표시 시간이 보존되고 파일이 정상 종료됨 |
| 한국어·긴 내용 | 한국어 설명, 긴 명령과 결과를 기록 | Run/Step, command/exit/duration을 읽을 수 있음; 축약이 오해를 만들지 않음 |
| 챕터 탐색 | 챕터 지원 플레이어와 FFprobe로 확인 | Setup/테스트 시작/체크포인트가 해당 영상 시점으로 이동 |
| 일반 재생 | QuickTime, Finder 미리보기 등에서 재생 | 파일 열기·탐색·음성 재생에 문제가 없음 |
| 장시간 녹화 | 실제 작업 분량으로 녹화 후 stop | 메모리·CPU·파일 크기·오디오 동기화가 사용 환경에 적합 |

실기기 점검 결과는 OS/기기, 커밋 SHA, 실행 명령, 관찰, MP4의 Run/Step을 함께 기록한다. 화면 권한 실패나 오디오 누락을 합성 helper로 우회해 실제 캡처 테스트 성공이라고 보고하지 않는다.

## 캡처 범위에 관한 주의

화면은 창 단위로 좁혀도 시스템 오디오는 앱 단위로 필터링된다. 따라서 같은 앱의 다른 창에서 발생한 소리가 포함될 수 있다. 앱 단위 캡처는 현재 주 모니터에 한정하며 다른 모니터 합성과 앱 재시작 후 재연결은 제공하지 않는다.

참조: [Apple — Take ScreenCaptureKit to the next level (WWDC22)](https://developer.apple.com/videos/play/wwdc2022/10155/), [FFmpeg formats — Metadata](https://ffmpeg.org/ffmpeg-formats.html#Metadata).

## Live capture health acceptance (2026-10-01)

Implemented in the live-capture-health branch; these checks do not retroactively
change the published 0.1.0 DMG. `rec status --json` is the read-only polling surface.
Target probes and helper reports run once per second. A missing source/idle sample
older than 3 seconds is distinct from telemetry older than 3 seconds. Readiness
validates a complete, correctly sized source buffer and successful video append,
not semantic correctness of captured content. Dark/static pixel heuristics are
advisory; they never establish target loss or broaden the filter.

Observed on macOS 26.6.2 with a dedicated native dark/static window (audio off):

- Before closing, status reported a validated first frame and available target,
  with no target-loss intervals. The video showed the intended dark window.
- Closing the selected window emitted target_lost and a ScreenCaptureKit error
  at Run time 32.709 seconds. An in-Run status query returned target_available=false,
  the error, source ages and affected intervals before stop.
- Stop produced a valid 48.043-second H.264 MP4 at 1400 × 984, the same selected
  window boundary. Reviewed later frames retained scoped pixels and the target-lost
  warning. The target-lost interval was 32.709–48.043 seconds.
- Raw video, session/events and capture-health.json were retained in the Run temp
  directory. The per-Run diagnostic log remained in the user's logs directory.

Synthetic telemetry checks additionally cover pending first frame, dark warning,
missing frames, recovery, stream errors, read-only session/event preservation,
and publishing validated MP4 with impairment intervals. Swift pixel tests cover
dark/static warnings independently from target availability and source counters
independently from encoded heartbeat copies.

The original intermittent Chrome black-window failures on macOS 26.5.2 were
not reproduced or attributed to window closure. The deterministic window-close
case demonstrates detection/preservation, not the original failure's root cause.
App restart, secondary displays and live audio remain separate runtime checks.
