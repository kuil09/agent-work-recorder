# Agent Work Recorder

**코딩 에이전트가 작업 과정과 자신의 판단을 화면에 기록하고, 사람이 MP4 하나로 검토하는 macOS CLI.**

에이전트는 행동·기대·관찰·테스트 결과를 기록한다. 사람은 영상을 재생하고, `7F32:014` 같은 Run/Step 식별자가 보이는 화면을 캡처해 수정 지시를 전달한다.

> **Record claims, not truth.** `Agent verdict: PASS`는 에이전트의 주장이지 도구가 내린 검증 결과가 아니다. 테스트 명령의 종료 코드가 0이어도 자동으로 PASS로 바꾸지 않는다.

## 구현 범위

Phase 1에 이어 **Phase 2의 네 기능**을 제공한다.

| 기능 | 인터페이스 / 동작 |
| --- | --- |
| 녹화·주석·관찰 | `start`, `note`, `expect`, `observe`, `checkpoint`, `stop` |
| 테스트 명령 실행 | `rec test`: 명령, 종료 코드, 실행 시간, 출력 요약 |
| 시스템 오디오 | `rec start --system-audio`: 기본 OFF, AAC, 마이크 없음 |
| MP4 챕터 | Setup, 테스트 시작, 체크포인트를 탐색 지점으로 사용 |
| 앱 단위 캡처 | `rec start --app NAME_OR_BUNDLE_ID`: 주 모니터의 해당 앱 창들 |
| 화면·창 캡처 | `--screen full`, `--window QUERY`, `--window-id ID` |
| Git 맥락 | 시작 시 저장소·브랜치·커밋·작업 트리 상태 수집 (`--no-git-context`로 생략) |
| 영상 산출물 | Run/Step 오버레이가 포함된 H.264 MP4 하나 |

CI는 빌드, 상태 모델, 실제 CLI/데몬 흐름, 합성 영상·오디오 인코딩, 챕터 보존을 검증한다. **CI 성공은 실제 Mac의 화면 녹화 권한, 앱 격리, 실시간 시스템 오디오까지 검증했다는 뜻이 아니다.** 실기기 점검 항목은 [Phase 2 검증 문서](docs/phase2-validation.md)에 구분해 두었다.

## 설치

Apple Silicon Mac의 macOS 14 이상을 지원한다. 일반 사용자용 **DMG에 녹화기와 미디어 도구가 모두 포함**되므로 Homebrew, Rust, Swift 설치가 필요 없다.

1. Apple Silicon Mac에서 `Agent-Work-Recorder-<버전>-macos-arm64.dmg`를 연다.
2. **Agent Work Recorder**를 **Applications** 폴더로 끌어 놓는다.
3. 응용 프로그램 폴더의 앱을 열고 **명령 사용 설정**을 누른다.
4. 터미널 또는 에이전트를 다시 열어 `rec --help`를 실행한다.

설정 앱은 현재 사용자의 `~/.local/bin`에 `rec`·`rec-capture` 링크를 만들고, zsh·bash 로그인 셸의 PATH를 설정한다. 기존 명령과 `.zprofile`·`.bash_profile`은 `.rec-backup-<식별자>` 파일로 백업한다. 설정 버튼을 반복해서 눌러도 같은 설정을 중복 추가하지 않는다. 앱을 설치 후 이동하거나 삭제하면 링크가 끊기므로, 업데이트는 같은 위치의 앱을 교체한 뒤 다시 설정한다.

에이전트가 셸 설정을 읽지 않는 실행 환경에서는 `/Applications/Agent Work Recorder.app/Contents/Resources/bin/rec`를 직접 사용한다. 에이전트용 스킬도 앱의 `Contents/Resources/agent-work-recorder/`에 포함된다.

공개 다운로드 게시 전에는 패키지를 빌드한 사람이 DMG를 전달해야 한다. `DEVELOPMENT-NOT-NOTARIZED` 파일은 개발 검증용이며 일반 사용자용 공증 배포 파일과 구분한다. 빌드·서명·공증·CI 절차는 [macOS 배포 문서](docs/macos-distribution.md)를 참고한다.

개발자가 소스에서 설치하려면 Rust stable, Swift 5.9 이상, macOS SDK, `make`, `ffmpeg`·`ffprobe`를 준비한 뒤 `make install`을 실행한다. 기본 설치 위치는 `~/.local/bin`이며 `make install PREFIX=/your/prefix`로 변경할 수 있다.

화면 녹화 권한은 실행하는 터미널 또는 에이전트에 부여한다.

```text
System Settings > Privacy & Security > Screen & System Audio Recording
```

macOS 버전에 따라 항목 이름이 다를 수 있다. 권한을 부여한 뒤 해당 프로세스를 다시 실행한다. 마이크 권한은 필요하지 않다.

## 에이전트용 도움말과 스킬

```bash
rec -h                  # 짧은 명령 목록
rec --help              # 전체 작업 흐름, 세션·출력·종료 코드 계약
rec start --help        # 대상 선택, 오디오 범위, 실패 처리
rec test --help         # argv 경계, 타임아웃, 실패 후 종료 예제
rec help stop           # stop --help와 같은 상세 도움말
```

각 명령의 `--help`에는 사전 조건, 상태 변화, 사용 예제와 오류 후 처리 지침을 포함한다.
도움말과 버전 확인은 녹화를 시작하거나 세션을 변경하지 않으며 화면 권한·미디어 도구가 필요 없다.
`rec test PROGRAM --help`는 녹화기 help가 아니라 **자식 프로그램을 실행하는 명령**이다.

[`agent-work-recorder` 스킬](skills/agent-work-recorder/SKILL.md)은 명령 선택부터
녹화 범위·Run 소유권, 기대와 관찰의 구분, 실패한 테스트 뒤의 종료, 실제 영상 확인,
스크린샷 피드백을 새 Run에 연결하는 절차까지 제공한다.
[스킬 설치와 구성](skills/README.md)을 참고한다. `make install`은 CLI만 설치하며,
스킬을 읽는 것 자체가 녹화나 외부 업로드의 승인은 아니다.

## 한 번의 작업 기록

```bash
# Safari의 주 모니터 창들을 녹화한다. 오디오는 기본 OFF.
rec start --title "로그인 오류 UI 검증" --app com.apple.Safari

rec note "로그인 오류 메시지의 위치를 확인한다."

# 실제 프로젝트에서 실행할 명령으로 바꾼다.
rec test npm test -- auth.test.ts

rec expect "잘못된 비밀번호 입력 시 버튼 바로 아래에 오류가 표시된다."

# 에이전트가 브라우저를 조작하고 결과를 확인한다.
# 카드를 읽을 수 있도록 이벤트를 무의미하게 연속 전송하지 않는다.

rec observe --status pass "오류 메시지가 로그인 버튼 아래에 표시됨"
rec checkpoint "로그인 실패 최종 화면"
rec stop
```

`rec test`는 실행한 명령의 종료 코드를 반환한다. 셸에서 `set -e`를 사용하면 실패한 테스트 때문에 후속 `rec stop`이 실행되지 않을 수 있으므로, 자동화 스크립트는 종료 처리를 별도로 보장해야 한다.

기본 산출물은 **`rec start`를 실행한 디렉터리**의 `recordings/<RUNID>-<title-slug>.mp4`다. 제목이 없으면 `<RUNID>.mp4`를 사용한다. `--output ./review.mp4`로 변경할 수 있으며 기존 파일은 덮어쓰지 않는다.

## 녹화 대상과 오디오

다음 `start` 예제는 서로 다른 선택지다. 동시에 여러 Run을 실행할 수 없다.

```bash
rec start --screen full
rec start --window "로그인 - Google Chrome"
rec start --window-id 12345
rec start --app com.google.Chrome
rec start --app com.apple.Safari --system-audio
```

`--screen`, `--window`, `--window-id`, `--app` 중 하나만 지정한다. 생략하면 주 모니터 전체를 선택한다.

**앱 캡처**는 정확한 앱 이름 또는 bundle ID를 받는다. 주 모니터에서 선택된 실행 중 앱의 창들을 포함하는 ScreenCaptureKit 필터를 사용한다. 다른 앱이나 다른 모니터로 자동 확대하지 않는다. 앱 이름이 모호하면 bundle ID를 지정한다. 실행 중인 앱·창의 식별자는 다음 명령으로 확인한다.

```bash
rec-capture list-apps
rec-capture list-windows
rec-capture list-displays
```

**창 캡처**의 `--window`는 제목 또는 앱 이름의 부분 문자열로 찾는다. 결과가 여러 개이면 임의의 창을 선택하지 않고 `--window-id` 지정을 요구한다. 앱 재시작 후 자동 재연결과 여러 모니터 동시 녹화는 지원하지 않는다. 대상 소실 시에는 마지막 프레임에 `CAPTURE TARGET LOST`를 표시하고 명시적인 `rec stop`을 기다리는 경로를 사용한다.

**파일 선택·저장 창의 증빙**이 필요한 작업은 허용된 범위 안에서 앱 캡처를 우선 사용한다. OS가 제공하는 창도 선택 앱 또는 선택 창에 속하면 포함될 수 있지만, 별도 프로세스나 다른 모니터의 창은 누락될 수 있다. 최종 MP4에서 창 열기 → 파일 선택 → 화면의 선택 결과까지 확인해야 한다. 창이 빠지거나 `CAPTURE TARGET LOST` 이후 화면이 정지했다면 해당 구간의 증빙은 불완전하다. 전체 화면이 필요한 경우 범위를 명시한 새 Run을 시작한다. 브라우저 자동화 API가 파일을 직접 지정하면 OS 파일 선택 창을 띄우지 않을 수 있으므로, 실제 실행 경로와 영상 누락을 구분한다.

**시스템 오디오**는 `--system-audio`를 지정했을 때만 48 kHz 스테레오 AAC로 기록한다. 마이크 입력은 구성하지 않는다. 전체 화면 캡처에서는 시스템 오디오, 앱 캡처에서는 선택한 앱의 오디오가 대상이다. **한 창만 녹화해도 오디오 필터는 창이 아니라 앱 단위**이므로 같은 앱의 다른 창에서 재생되는 소리가 포함될 수 있다.

앱·창 캡처나 오디오를 요청한 상태에서 네이티브 캡처가 실패하면 전체 화면 또는 무음 영상으로 대체하지 않는다. 오디오를 요청했지만 AAC 트랙이 생성되지 않으면 `stop`도 성공으로 처리하지 않는다.

### 스크린샷 대체 경로

기존 CuaDriver 스크린샷 경로는 **`--allow-screenshot-fallback`을 명시하고**, **주 모니터 전체·오디오 OFF**일 때만 허용된다. 기본값은 OFF라서 네이티브 캡처의 권한·초기화 실패가 스크린샷 녹화로 가려지지 않고 오류로 보고된다. 폴백이 사용되면 `rec status`와 `rec stop` 출력에 Warning이 표시된다. CuaDriver가 실행 중이어야 하며, 필요하면 `REC_CUA_BIN`, `REC_CUA_SOCKET`으로 지정한다. 이 경로는 네이티브 30 fps 녹화보다 성긴 화면 기록이며 프레임 간 실제 경과 시간을 반영한다. CuaDriver 없이도 일반 ScreenCaptureKit 경로는 동작한다.

## `rec test`

```bash
rec test npm test -- auth.test.ts
rec test --timeout-secs 60 -- cargo test
rec test -- sh -c 'printf "result\n"; exit 7'
```

명령은 **`rec test`를 호출한 프로세스의 현재 디렉터리와 환경변수**에서 실행된다. 암묵적으로 셸을 거치지 않으므로 파이프·리다이렉션 등이 필요하면 `sh -c`를 직접 지정한다. 테스트는 신뢰할 수 있는 명령만 실행해야 하며 이 도구가 명령을 샌드박싱하지는 않는다.

| 기록 | 내용 |
| --- | --- |
| 시작 | 인자 목록을 표시한 명령, 호출 디렉터리, 실행 중 상태 |
| 완료 | 실제 종료 코드 또는 시그널, 실행 시간, 타임아웃·실행 실패 여부 |
| 출력 | stdout/stderr를 터미널로 전달하고 각 스트림 끝부분 최대 4 KiB를 내부 기록에 유지 |
| 영상 | 명령·종료 코드·시간과 제한된 길이의 출력 요약을 TEST 카드에 표시 |

반환 코드는 명령 종료 코드 그대로다. 타임아웃은 `124`, 실행 파일을 시작하지 못한 경우는 `127`, 시그널 종료는 `128 + signal`이다. 녹화기 통신 등 도구 자체의 실패는 `1`이다. 프로그램이 원래 124나 127을 반환하는 경우도 있으므로 정확한 사유는 카드와 기록을 함께 구분한다.

기본 제한 시간은 300초다. `--timeout-secs`는 실행할 명령 **앞에** 둔다. stdin은 연결하지 않는 비대화형 실행이며, 제한 시간 또는 인터럽트 시 테스트 프로세스 그룹을 종료한다. 백그라운드 서버를 남기는 용도로 사용하지 않는다.

테스트 실행 중에도 데몬은 `note` 등의 명령을 받는다. 다만 두 번째 테스트 실행과 `stop`은 거부한다. 실행 결과가 다른 Run에 들어가지 않도록 시작 시의 세션과 테스트 식별자에 완료 이벤트를 연결한다.

`rec test` 프로세스가 사라졌거나 PID를 확인할 수 없어도 `stop`이 영구히 막히지 않는다. 데몬은 각 테스트에 제한 시간 + 30초의 기한을 두고, 기한이 지나면 "결과 미상"으로 종료 처리한다. 즉시 정리하려면 `rec stop --abandon-test`를 사용한다(테스트 프로세스에 신호를 보내지는 않는다). 로그에서 비밀 값처럼 보이는 `NAME=값`, `--token 값`, `Bearer` 토큰, 잘 알려진 키 접두사는 영상 카드와 기록에서 마스킹한다. 이는 최선의 안전장치일 뿐이므로 출력 요약 자체를 남기지 않으려면 `rec test --no-output-summary`를 사용한다.

**한 번의 `rec test`는 테스트 1건으로 집계하고 시작·완료에 각각 Step을 만든다.** 이전 관찰의 PASS 표시는 테스트 시작 시 지우며 종료 코드 0만으로 새 PASS를 만들지 않는다. 실행 결과에 대한 에이전트 판단은 별도의 `rec observe --status ...`로 남긴다.

## Run, Step, 주석과 챕터

Run은 `start`부터 `stop`까지의 실행이며 4자리 16진수 ID를 가진다. `7F32:014`는 Run `7F32`의 14번째 이벤트다. 화면 변화나 시간 경과만으로 Step이 증가하지 않는다.

| 명령 | 의미 |
| --- | --- |
| `rec note "..."` | 현재 행동·맥락을 기록하고 짧은 상시 Action을 갱신 |
| `rec expect "..."` | 기대 결과를 카드로 표시; 상시 Action은 유지 |
| `rec observe --status pass|fail|uncertain|info "..."` | 에이전트 관찰·주장; 상태 생략 시 info |
| `rec checkpoint "..."` | 사람이 확인할 장면과 챕터 경계 |
| `rec test ...` | 명령의 시작과 기계적 실행 결과 |

Run/Step을 영상에 합성하고 주석 카드는 약 4초 표시한다. 다음 이벤트가 먼저 도착하면 이전 카드를 교체하므로 전체 이벤트 로그를 모두 읽을 수 있는 영상으로 만들려면 설명 사이에 검토 시간을 확보해야 한다. Git 맥락은 시작 시 약 4.5초 표시한다. 정지 화면에서도 프레임 타이머가 오버레이를 갱신한다.

`rec stop`은 마지막 카드의 남은 표시 시간을 확보한 뒤 파일을 마무리한다. 따라서 마지막 이벤트 직후 호출하면 인코딩 시간 외에 최대 약 4초의 표시 시간이 추가될 수 있다. 긴 설명은 화면 크기에 맞춰 줄바꿈·축약하며, 창이 너무 작으면 검토할 정보가 잘릴 수 있으므로 충분한 크기의 창을 선택한다.

### MP4 챕터 정책

챕터는 **Setup → 테스트 시작 → 체크포인트**를 기준으로 만든다. `note`, `expect`, `observe`, 테스트 완료마다 챕터를 만들지는 않는다. 같은 밀리초의 경계는 하나로 합치고 마지막 챕터는 실제 영상 길이에서 끝낸다.

챕터 시간은 벽시계 변경에 영향을 받지 않는 경과 시간을 기준으로 계산한다. 종료 시 FFmpeg로 챕터와 메타데이터를 넣고, FFprobe로 H.264/AAC 유무, 챕터 제목·경계가 보존됐는지 확인한다. 재생기의 챕터 UI 지원 여부와 상관없이 핵심 설명은 영상에 표시한다.

```bash
ffprobe -v error -show_chapters -show_streams -of json ./review.mp4
```

## 사람이 검토하는 방법

```text
작업 지시 → 에이전트 작업·기록 → MP4 → 사람 검토
                                        ↓
                              Run/Step 캡처 + 텍스트
                                        ↓
                                  다음 수정 Run
```

예를 들어 `7F32:021`이 보이는 장면을 캡처하고 “오류 메시지가 버튼과 너무 붙어 있다. 8px 더 떨어뜨려라”라고 전달한다. 도구 내부에 댓글 시스템을 만들지 않으며 최종 검토자는 사람이다.

## 데이터와 개인정보

`rec start --no-git-context`는 저장소·브랜치·커밋 정보를 수집·표시·기록하지 않는다. `rec test --no-output-summary`는 출력을 터미널에는 전달하지만 영상과 이벤트 기록에는 남기지 않는다. 세션 파일과 시작 설정은 소유자 전용(0600) 권한으로 기록한다.

녹화는 명시적으로 시작·종료하며 사용자 영상을 클라우드에 업로드하지 않는다. 결과물은 MP4 하나지만 실행 중에는 내부 파일을 사용한다.

```text
~/.agent-recorder/session                 현재 세션
~/.agent-recorder/logs/<RUNID>.log        진단 로그
<OS temp>/agent-recorder/<RUNID>/
    args.json
    session.json
    events.jsonl
    raw.mp4
    chapters.ffmetadata
```

macOS의 실제 임시 디렉터리는 `/tmp`와 다를 수 있다. 최종 파일 검증·게시 성공 후 Run 임시 디렉터리를 삭제한다. 실패하면 원본을 보존하며, 진단 로그는 정상 종료 후에도 남는다. 영상뿐 아니라 명령 인자·출력 요약·화면에 민감한 정보가 포함될 수 있으므로 입력과 보관 범위를 검토해야 한다.

오류를 확인할 때는 출력된 Run ID에 해당하는 로그를 읽는다. 테스트가 아직 실행 중이면 끝낸 뒤 `rec stop`을 호출한다(프로세스가 사라졌다면 `rec stop --abandon-test`). 녹화 중에 지정한 출력 경로에 다른 파일이 생기면 기존 파일을 보존하고 `<이름>-<RUNID>.mp4`로 게시하며 Warning을 출력한다. MP4 게시 이후의 임시 파일 정리 실패는 Warning이며 stop 실패가 아니다. 파일 마무리에 실패했으면 의존성·출력 경로 등 원인을 수정한 후 동일 세션의 `rec stop`을 다시 시도할 수 있다. 프로세스 충돌 후 세션 복구나 손상된 MP4 복원은 제공하지 않는다. 운영자용 절차는 [세션 복구](docs/session-recovery.md), 창·앱 캡처 문제는 [macOS 캡처 진단](docs/macos-capture-diagnostics.md)을 참고한다.

## 내부 구조

```text
rec CLI (Rust)
  ├─ test: 호출 환경에서 subprocess 실행
  └─ Unix Domain Socket
        ↓
Recorder Daemon (Rust)
  ├─ 단일 Run 잠금 / 이벤트 / 테스트 상태 / Git 정보
  ├─ ScreenCaptureKit helper 제어
  └─ FFmpeg + FFprobe: 챕터·오디오 보존 검증 후 MP4 게시
        ↓
rec-capture (Swift)
  ├─ display / window / application 필터
  ├─ 선택적 system audio → AAC
  └─ 직렬 media queue → 오버레이 + H.264 MP4
```

Rust 모듈은 `runner.rs`(테스트 실행), `media.rs`(챕터·최종화), `daemon.rs`(세션과 IPC), `protocol.rs`(계약)로 나뉜다. Swift는 `MediaWriter.swift`, `Overlay.swift`, 캡처 진입점으로 나뉘며 합성 인코딩 테스트도 같은 writer를 사용한다.

## 개발과 검증

```bash
cargo test --all-targets
swift test --package-path macos/RecCapture
swift build -c release --package-path macos/RecCapture
cargo build
python3 tests/e2e.py
```

네이티브 인코더를 화면 권한 없이 확인할 수도 있다. 아래 출력은 **실제 작업 화면이 아닌 합성 테스트 영상**이다.

```bash
macos/RecCapture/.build/release/rec-capture self-test \
  --output ./synthetic.mp4 --system-audio
```

CI는 위 테스트를 실행하고 합성 MP4·FFprobe 결과를 artifact로 남긴다. CI artifact 업로드는 테스트 데이터에만 적용되며 `rec`의 사용자 녹화 업로드 기능이 아니다. 실기기 권한·앱 격리·오디오 검증은 [별도 절차](docs/phase2-validation.md)를 따른다.

## 비목표와 제한

Windows/Linux, 마이크 녹음, 자동 UI 정답 판정, 클라우드 리뷰 시스템, 행동 재생, 자동 Git commit/PR 생성, 별도 사용자 JSON/HTML 리포트는 제공하지 않는다. `pause`, `resume`, `cancel`, 수동 `chapter` 명령은 아직 제공하지 않는다. `rec status`는 활성 Run을 변경 없이 조회하는 읽기 전용 명령이다.

현재 앱 캡처 범위는 주 모니터다. 여러 모니터 동시 캡처, 앱 재시작 후 재연결, 실행 환경 스냅샷은 지원하지 않는다. 4자리 Run ID는 짧은 피드백 참조이며 모든 과거 실행에 대해 전역 유일성을 보장하는 식별자는 아니다.

## License

[MIT License](LICENSE)로 배포한다.
