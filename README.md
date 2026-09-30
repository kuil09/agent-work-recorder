# Agent Work Recorder

**코딩 에이전트가 자신의 작업과 판단을 화면과 함께 기록해, 사람이 MP4 하나만 보고 검토할 수 있게 하는 macOS CLI 도구.**

Agent Work Recorder는 일반적인 화면 녹화기가 아니다. 에이전트가 작업 중 자신의 **행동, 기대 결과, 관찰 결과, 체크포인트**를 명시적으로 기록하고, 이를 실제 작업 화면 위에 Run/Step 정보와 함께 남긴다.

사용자는 영상의 특정 장면을 캡처해 다시 에이전트에게 전달할 수 있다. 캡처에는 `7F32:014` 같은 식별자가 항상 남으므로, "어느 실행의 어느 단계가 잘못됐는지"를 다시 참조할 수 있다.

> 이 도구는 자동 검증기가 아니다.
>
> `PASS`, `FAIL`, `Expected`, `Observed`는 **에이전트의 주장**을 기록한 값이다. 최종 검토자는 사람이다.

## 현재 상태

현재 구현 범위는 **MVP Phase 1**이다.

| 기능 | 상태 |
| --- | --- |
| `rec start` / `stop` | 구현 |
| `rec note` | 구현 |
| `rec expect` | 구현 |
| `rec observe` | 구현 |
| `rec checkpoint` | 구현 |
| 전체 화면 녹화 | 구현 |
| 특정 창 녹화 | 구현 |
| Window ID 지정 | 구현 |
| Git metadata 수집 | 구현 |
| Persistent Run/Step overlay | 구현 |
| 일시 Event Card | 구현 |
| H.264 MP4 출력 | 구현 |
| `rec test` | Phase 2 |
| 시스템 오디오 | Phase 2 |
| MP4 chapters | Phase 2 |
| 애플리케이션 단위 캡처 | Phase 2 |

## 핵심 사용 흐름

```text
Human gives task
      │
      ▼
Agent
      │
      ├─ rec start
      ├─ rec note
      ├─ work
      ├─ rec expect
      ├─ UI / app verification
      ├─ rec observe
      ├─ rec checkpoint
      └─ rec stop
      │
      ▼
     MP4
      │
      ▼
Human review
      │
      ├─ accept
      └─ screenshot + feedback
                    │
                    ▼
                  Agent
```

핵심 산출물은 별도 리포트가 아니라 **MP4 하나**다.

## 예시

로그인 오류 UI를 수정하고 검증한다고 가정한다.

```bash
rec start \
  --title "로그인 오류 UI 수정" \
  --window "Google Chrome"

rec note \
  "로그인 오류 메시지 위치를 수정하고 브라우저에서 확인한다."

rec expect \
  "잘못된 비밀번호 입력 시 버튼 바로 아래에 오류 메시지가 표시된다."

# 에이전트가 브라우저를 조작하고 결과를 확인한다.

rec observe \
  --status pass \
  "오류 메시지가 로그인 버튼 아래에 표시됨"

rec checkpoint \
  "로그인 실패 최종 화면"

rec stop
```

기본 출력 위치:

```text
./recordings/<RUNID>-<title-slug>.mp4
```

예:

```text
./recordings/7F32-로그인-오류-ui-수정.mp4
```

## Run과 Step

한 번의 `rec start`부터 `rec stop`까지를 하나의 **Run**으로 취급한다.

각 Run은 4자리 hexadecimal ID를 가진다.

```text
7F32
```

의미 있는 이벤트가 기록될 때마다 Step이 증가한다.

```text
7F32:001
7F32:002
7F32:003
```

Phase 1에서 Step을 만드는 명령:

```text
note
expect
observe
checkpoint
```

화면이 바뀌거나 시간이 흐르는 것만으로는 Step이 증가하지 않는다.

## 화면 오버레이

영상에는 항상 최소한의 persistent HUD가 표시된다.

```text
7F32:014

Verify login error layout

Agent verdict: PASS
```

Run/Step 식별자는 영상 전체에서 유지된다. 따라서 사용자가 임의의 프레임을 캡처해도 어떤 실행의 어느 단계인지 식별할 수 있다.

### Event Card

`note`, `expect`, `observe`, `checkpoint`가 발생하면 해당 내용이 일시적인 카드로 표시된다.

예:

```text
EXPECT

Wrong password should display
an error below the login button.
```

카드는 약 4초 뒤 사라진다. 긴 설명은 persistent HUD에 계속 남기지 않는다.

### Git Context

`rec start` 시 현재 작업 디렉터리에서 다음 정보를 수집한다.

- repository name
- repository root
- branch
- HEAD commit hash
- working tree clean / dirty
- changed files count

영상 시작 시 Git Context가 잠시 표시되며 이후 화면에서는 제거된다.

Git 저장소가 아니어도 녹화는 계속된다.

```text
Git: unavailable
```

## 명령

### 녹화 시작

기본값은 main display 전체 화면이다.

```bash
rec start
```

명시적으로 전체 화면:

```bash
rec start --screen full
```

제목 지정:

```bash
rec start --title "Checkout validation"
```

특정 창:

```bash
rec start --window "Google Chrome"
```

Window ID 직접 지정:

```bash
rec start --window-id 12345
```

출력 파일 지정:

```bash
rec start --output ./result.mp4
```

`--screen`, `--window`, `--window-id`는 동시에 하나만 사용할 수 있다.

특정 창을 요청한 경우, 캡처 실패를 이유로 전체 데스크톱 녹화로 자동 확대하지 않는다. 명시적 캡처 범위를 유지하기 위해 해당 Run을 실패시킨다.

### 현재 행동 기록

```bash
rec note "CSS spacing 수정"
```

`note`는 현재 작업 또는 행동 맥락을 기록한다.

### 기대 결과 기록

```bash
rec expect \
  "로그인 실패 시 오류 메시지가 버튼 아래 표시된다."
```

Expectation은 일시적인 Event Card로 표시되며 현재 Action을 영구적으로 덮어쓰지 않는다.

### 관찰 결과 기록

```bash
rec observe \
  "오류 메시지가 버튼 아래 표시되었다."
```

상태를 함께 기록할 수 있다.

```bash
rec observe \
  --status pass \
  "오류 메시지가 예상 위치에 나타남"
```

지원 상태:

```text
pass
fail
uncertain
info
```

`pass`와 `fail`은 제품의 객관적 판정이 아니다.

화면에는 다음처럼 표시된다.

```text
Agent verdict: PASS
```

### Checkpoint

```bash
rec checkpoint "로그인 실패 최종 화면"
```

사람이 특히 확인해야 할 장면을 표시한다.

현재 Phase 1에서는 Step과 Checkpoint Event Card를 생성한다. MP4 chapter 생성은 Phase 2 범위다.

### 녹화 종료

```bash
rec stop
```

성공하면 임시 런타임 파일을 정리하고 최종 MP4 경로를 출력한다.

## 설치

### 요구 사항

- macOS 14+
- Rust toolchain
- Swift 5.9+
- `ffmpeg`
- Screen Recording 권한

빌드 및 설치:

```bash
make install
```

기본 설치 위치:

```text
~/.local/bin/rec
~/.local/bin/rec-capture
```

필요하면 `~/.local/bin`을 `PATH`에 추가한다.

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## macOS 권한

화면 캡처를 실행하는 터미널 또는 에이전트 프로세스에 Screen Recording 권한이 필요하다.

```text
System Settings
  > Privacy & Security
  > Screen & System Audio Recording
```

권한을 부여한 뒤 해당 터미널 또는 에이전트 프로세스를 다시 실행한다.

권한이 없으면 녹화를 시작하지 않는다.

## 설계 원칙

### Record claims, not truth

도구는 에이전트가 "무엇을 했다고 주장하는지"와 "무엇을 봤다고 판단하는지"를 기록한다.

자동으로 UI가 올바른지 판단하지 않는다.

### Human remains the reviewer

최종 검증자는 사람이다.

### Video first

사용자는 추가 JSON이나 HTML 리포트 없이 MP4만으로 핵심 작업 흐름을 검토할 수 있어야 한다.

### Explicit recording

`rec start` 없이는 녹화하지 않는다.

상시 백그라운드 녹화를 하지 않는다.

### Screenshot-addressable

모든 프레임에서 Run/Step을 식별할 수 있어야 한다.

이를 통해 다음과 같은 피드백이 가능하다.

```text
[7F32:021가 보이는 screenshot]

오류 메시지가 버튼과 너무 붙어 있다.
간격을 늘려라.
```

## 개인정보 보호

Phase 1의 기본 정책:

- 명시적 `rec start` 없이는 녹화하지 않는다.
- 백그라운드 상시 녹화를 하지 않는다.
- 클라우드로 데이터를 전송하지 않는다.
- 결과물은 로컬에만 저장한다.
- 마이크를 녹음하지 않는다.
- 특정 창 요청을 전체 데스크톱 녹화로 자동 확대하지 않는다.

시스템 오디오는 아직 지원하지 않으며 Phase 2에서 명시적 opt-in으로 추가할 예정이다.

## 내부 구조

```text
rec CLI (Rust)
    │
    │ Unix Domain Socket
    ▼
Recorder Daemon (Rust)
    │
    ├─ Run / Step state
    ├─ Git metadata
    ├─ events.jsonl
    └─ capture control
            │
            ▼
      rec-capture (Swift)
            │
            ├─ ScreenCaptureKit
            ├─ overlay rendering
            └─ H.264 MP4
```

사용자에게는 MP4만 제공하지만 실행 중에는 내부적으로 임시 상태와 이벤트 파일을 사용할 수 있다.

```text
/tmp/agent-recorder/<RUNID>/
    raw.mp4
    events.jsonl
    session.json
```

`rec stop`의 finalization이 성공하면 임시 디렉터리를 삭제한다.

현재 세션 정보는 다음 경로에서 관리한다.

```text
~/.agent-recorder/session
```

MVP에서는 동시에 하나의 Run만 허용한다.

## 개발

Rust 테스트:

```bash
cargo test --all-targets
```

Swift capture helper 빌드:

```bash
swift build -c release --package-path macos/RecCapture
```

전체 release 빌드:

```bash
make
```

설치:

```bash
make install
```

CI는 macOS runner에서 Rust 테스트와 Swift release 빌드를 모두 실행한다.

## Phase 2

다음 기능은 PRD상 Phase 2다.

### `rec test`

에이전트가 subprocess를 실행하고 다음 정보를 영상에 기록한다.

- command
- exit code
- duration
- stdout/stderr summary

단순히 exit code 0이라고 해서 자동으로 `PASS`로 판정하지 않는다.

### System audio

```bash
rec start --system-audio
```

기본값 OFF. 마이크는 계속 지원하지 않는다.

### MP4 chapters

Step과 별도로 사람이 긴 영상을 탐색하기 위한 큰 작업 단위를 MP4 chapter metadata로 기록한다.

### Application capture

특정 Window가 아니라 애플리케이션 단위 캡처를 추가한다.

## Non-goals

현재 제품은 다음을 목표로 하지 않는다.

- Windows / Linux 지원
- 클라우드 업로드
- 웹 기반 리뷰 시스템
- AI 자동 영상 분석
- 자동 UI 정답 판정
- 에이전트 행동 재생
- 자동 Git commit 또는 PR 생성
- 마이크 녹음
- 별도 JSON/HTML 사용자 리포트

## License

MIT
