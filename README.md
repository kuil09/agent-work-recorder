# Agent Work Recorder

macOS CLI. Coding agent가 작업 화면을 MP4로 남긴다. 사용자는 MP4만 재생해서 검토한다.

Phase 1: `start` `note` `expect` `observe` `checkpoint` `stop`.

## 설치

```
make install
```

`~/.local/bin/rec` 와 `rec-capture` 에 설치한다. PATH에 `~/.local/bin` 필요.

Screen Recording 권한:

System Settings > Privacy & Security > Screen & System Audio Recording

실행하는 터미널(또는 Hermes)에 권한을 준 뒤 재실행.

## 사용

```
rec start --title "로그인 오류 UI 수정" --screen full
rec start --window "Google Chrome"
rec start --window-id 12345

rec note "로그인 오류 메시지 위치를 수정하고 브라우저에서 확인한다."
rec expect "잘못된 비밀번호 입력 시 버튼 바로 아래에 오류 메시지가 표시된다."
rec observe --status pass "오류 메시지가 로그인 버튼 아래에 표시됨"
rec checkpoint "로그인 실패 최종 화면"
rec stop
```

결과: `./recordings/<RUNID>-<slug>.mp4`

`pass`는 제품 판정이 아니다. 에이전트 주장이다. 화면에는 `Agent verdict: PASS` 로 표시된다.

`--status`: `pass` `fail` `uncertain` `info`

## 동작

- Run ID 4자리 hex. Step은 `7F32:014`.
- HUD는 항상 좌상단. 임의 캡처에도 Run:Step이 보여야 한다.
- note/expect/observe/checkpoint 는 Step을 만든다. 화면 변화만으로는 Step이 늘지 않는다.
- Git 정보는 start 시 수집. 저장소가 아니면 `Git: unavailable`. 녹화는 막지 않는다.
- 동시 다중 Run 없음.
- 마이크/시스템 오디오/chapters/`rec test` 는 Phase 1 밖.

## 산출물

사용자에게 주는 것은 MP4 하나. 런타임 임시 파일은 `rec stop` 성공 후 삭제된다.
