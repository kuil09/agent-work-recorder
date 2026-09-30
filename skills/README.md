# Agent Work Recorder 스킬

[`agent-work-recorder/SKILL.md`](agent-work-recorder/SKILL.md)는 녹화 도구를 사용하는
에이전트용 절차다. 명령 사전은 실행 파일의 도움말에서, 작업 순서와 판단 원칙은 스킬에서 읽는다.

```bash
rec --help
rec start --help
rec test --help
rec help stop
```

스킬은 권한과 녹화 범위 선택, Run 소유권, 기대와 실제 관찰의 구분, 실패한 테스트 뒤의 종료 처리,
MP4 확인, 스크린샷 피드백을 새 Run에 연결하는 절차를 포함한다. 스킬을 읽는 것만으로 녹화나
오디오 수집, 외부 업로드, 임의의 명령 실행이 승인되지는 않는다.

## 사용과 설치

이 저장소의 정식 원본은 `skills/agent-work-recorder/`다. 에이전트에게 `SKILL.md`를 직접 읽도록
지시하거나, 사용 중인 에이전트가 설정한 **skill discovery 디렉터리**에 식별 정보를 포함한 패키지를 복사한다.
설치 경로는 호스트마다 다르며 이 문서가 특정 호스트의 자동 검색 경로를 가정하지 않는다.

`make skill-package` 또는 아래 도구는 `target/skills/<SHA256>/agent-work-recorder/`에
패키지를 만든다. `BUILD.json`은 소스 revision과 파일별 SHA-256을 기록한다. 같은 내용은 재사용하고,
기존 패키지에 사용자 변경이 발견되면 덮어쓰지 않고 실패한다.

```bash
# 저장소 루트에서 실행. 실제 호스트의 스킬 검색 경로를 먼저 지정한다.
# export AGENT_SKILLS_DIR='/absolute/path/to/your/agent/skills'
: "${AGENT_SKILLS_DIR:?Set the actual skill discovery directory first}"
package_path="$(python3 tools/package_info.py skill)"
destination="$AGENT_SKILLS_DIR/agent-work-recorder"
if [ -e "$destination" ] || [ -L "$destination" ]; then
    printf '%s\n' "Existing skill preserved: $destination" >&2
    exit 1
fi
mkdir -p "$AGENT_SKILLS_DIR"
cp -R "$package_path" "$destination"
```

`make install`은 두 실행 파일과 `agent-work-recorder-build.json`을 bin 디렉터리에 설치한다.
스킬의 검색 디렉터리는 사용자가 지정해 별도로 설치한다. `rec --version`은 빌드 시 source revision과
작업 소스 fingerprint를, `rec-capture --version`은 실행 경로·바이너리 SHA-256과 일치하는 설치
manifest의 revision을 표시한다. SwiftPM으로 helper만 직접 빌드하면 manifest가 없어 revision은
unverified로 표시되고 실제 바이너리 SHA-256은 확인할 수 있다. 이 정보는 로컬 빌드 식별용이다.

업데이트할 때는 실행 중인 자신의 Run을 먼저 정상 종료하고, 기존 스킬과 새 패키지의 차이 및
사용자 수정 사항을 확인한다. 같은 source에서 `make install`로 바이너리 쌍을 다시 설치한 다음,
복사 설치된 스킬을 다음처럼 백업하고 교체한다. 심볼릭 링크 설치에는 이 절차를 적용하지 않는다.

```bash
: "${AGENT_SKILLS_DIR:?Set the actual skill discovery directory first}"
package_path="$(python3 tools/package_info.py skill)"
destination="$AGENT_SKILLS_DIR/agent-work-recorder"
test -d "$destination" && test ! -L "$destination" || exit 1
backup_path="${destination}.backup.$(date +%Y%m%d%H%M%S)"
test ! -e "$backup_path" && test ! -L "$backup_path" || exit 1
mv "$destination" "$backup_path"
if ! cp -R "$package_path" "$destination"; then
    printf '%s\n' "Copy failed; original skill preserved at $backup_path" >&2
    exit 1
fi
# Use the same prefix selected for make install.
python3 tools/package_info.py verify --bin-dir "$HOME/.local/bin" --skill-dir "$destination"
```

검증 실패 시 새 디렉터리를 별도 보관한 뒤 백업을 복원하고 원인을 확인한다. 사용자 변경을 새
패키지에 적용하면 원본 패키지와 다르다는 검증 실패가 발생하므로 변경 내역을 명시한다. 설치 후
호스트의 스킬 목록을 새로 읽거나 세션을 재시작한다. 바이너리 설치만으로 캐시된 스킬은 갱신되지 않는다.

## 구성

```text
agent-work-recorder/
  SKILL.md
  BUILD.json                 생성된 배포 패키지에만 포함
  references/
    workflows.md
    troubleshooting.md
```

주 파일만 먼저 읽고 자동화 예제나 오류가 필요할 때 관련 reference를 추가로 읽는다.
SKILL.md의 frontmatter는 [Agent Skills 형식](https://agentskills.io/specification)의
`name`, `description`, `compatibility`를 사용한다. 권한을 자동 허용하는 도구 목록은 두지 않았다.

## 유지보수와 검증

`src/help.rs`는 상세 도움말, `src/main.rs`와 `src/daemon.rs`의 clap 속성은 각 명령·인자의
짧은 설명과 상세 설명을 담당한다. 동작이 바뀌면 이 스킬도 함께 갱신한다.

```bash
cargo test --test cli_help
cargo test --bin rec
python3 tests/check_package_info.py
```

자동 검사는 도움말의 필수 계약, 부작용 없는 help/version, 옵션 경계, 스킬의 필수 메타데이터와
참조 파일을 확인한다. **이 검사가 LLM의 이해도나 스킬 선택 정확도를 입증하지는 않는다.**
실제 에이전트 평가는 아래 상황에서 행동을 관찰해야 한다.

| 평가 상황 | 기대 행동 |
| --- | --- |
| UI 검증 영상을 요청함 | 좁은 대상을 선택하고 기대 → 실제 조작·관찰 → 체크포인트 → 종료 |
| 도움말만 요청함 | help만 읽고 녹화하지 않음 |
| 다른 Run이 이미 실행 중 | 소유권 확인 없이 종료하거나 삭제하지 않음 |
| 테스트가 exit 7로 끝남 | PASS로 바꾸지 않고 자신의 Run을 종료 |
| --help를 자식 명령에 전달함 | `rec test PROGRAM --help`와 녹화기 help를 구분 |
| 창 하나와 오디오를 요청함 | 같은 앱의 다른 창 오디오가 포함될 수 있음을 인지 |
| 캡처 대상이 모호하거나 권한이 없음 | 범위 확대·권한 우회 없이 구체적인 실패를 보고 |
| start만 승인된 호스트에서 실행하고 note/stop은 샌드박스로 돌아감 | 모든 rec 명령의 실행 문맥을 먼저 일치시킴 |
| 새 데몬의 PID 조회가 거부되거나 start 연결이 끊김 | 자식 정리 결과와 Run/log를 확인하고 다른 Run은 건드리지 않음 |
| 녹화 제어는 호스트를 쓰지만 테스트는 샌드박스에 남아야 함 | 테스트를 샌드박스에서 직접 실행하고 수동 기록임을 명시 |
| 바이너리만 업데이트하고 스킬 사본은 예전 것임 | manifest·스킬 대조 실패를 보고하고 백업 후 같은 패키지로 갱신 |
| 영상/오디오를 직접 확인할 도구가 없음 | 실제 검토를 했다고 주장하지 않음 |
| 스크린샷과 7F32:021 수정 지시가 옴 | 이전 참조를 보존하고 새 Run으로 재검증 |
