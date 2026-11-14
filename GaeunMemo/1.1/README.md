# GaeunMemo 1.1

Windows 바탕화면 스티커 메모. Rust + Tauri 2, 프런트엔드는 바닐라 HTML/CSS/JS.

- 메모 하나 = 프레임 없는 창 하나. 접기, 항상 위 고정, 6가지 파스텔 색, 띠 모양(없음/얇은 띠/제목 줄), 다크·라이트 테마
- 메모 목록 창: 검색, 최근 수정순 정렬, 키보드 조작, 우클릭 삭제
- 트레이 상주, 컴퓨터 켤 때 자동 실행, 단일 인스턴스, 작업표시줄에서 메모별 미리보기
- 자동 저장(첫 입력 즉시 + 입력 중 최대 0.3초 간격), 원자적 쓰기, 하루 한 번 백업 7개 보관

## 1.1에서 바뀐 것

- **전체화면:** ⋯ → "전체화면"(최대화). 창을 직접 화면만큼 키워도 같은 상태. 제목 줄 축소 아이콘·"원래 크기"·제목 줄 더블클릭으로 원래 크기와 위치로 돌아감
- **서식:** 굵게·기울임·밑줄·취소선·글머리 기호. 도구 모음은 전체화면에서 제목 줄 아래에 표시, 작은 메모에서도 서식은 보이고 Ctrl+B/I/U 사용 가능
- **사진:** Ctrl+V 붙여넣기, 끌어다 놓기, 도구 모음 버튼. 메모 폭에 맞춰 표시, 원본은 `%APPDATA%\gaeunmemo\images\`에 저장
- **붙여넣기 정리:** 웹·워드에서 붙이면 지원 서식만 남기고 링크·스크립트·외부 사진은 제거. Ctrl+Shift+V는 글자만
- **잠금:** ⋯ → "잠금". 수정·서식·색·제목·삭제를 막고, 본문을 클릭하면 본문 전체를 글자로 복사(제목 자리에 "✓ 복사됨"). 잠긴 동안 글자 선택 표시 없음
- **작업표시줄:** 메모마다 작업표시줄 창(Windows가 한 아이콘으로 묶어 메모별 미리보기)
- **자동 실행 복구:** 재설치 등으로 Run 항목이 지워지면 다음 실행 때 저장된 설정대로 다시 등록
- **시작 로그:** `%APPDATA%\gaeunmemo\gaeunmemo.log`

## 빌드와 테스트

```
cd src-tauri
cargo test            # 저장 / 가져오기 / 사진 저장소 / 모니터 보정 단위 테스트
cargo tauri build     # 릴리스 exe + NSIS 설치 파일 (target/release/bundle/nsis)
```

메모 화면 UI 테스트: `gaeunmemo` 폴더에서 `python -m http.server` 후 브라우저로 `/tests/note_ui_test.html`을 열면 붙여넣기·사진·잠금 검사 결과가 오른쪽에 나온다.

아이콘: `python tools/make_icon.py` 가 `src-tauri/icons/` 에 모든 크기를 만든다.

설치 파일을 다시 설치할 때는 업데이트 모드(`/UPDATE`)로 실행한다. 일반 재설치는 Tauri 설치 관리자가 자동 실행 항목을 지운다(앱이 다음 실행 때 스스로 복구하긴 한다).

## 구조

| 경로 | 내용 |
| --- | --- |
| `src-tauri/src/main.rs` | 시작 순서, 첫 실행, 자동 실행 복구, 사진 주소(`gmimg`), 종료 시 저장 |
| `src-tauri/src/model.rs` | 데이터 모델(평문 + 서식 HTML, 잠금, 최대화), 기본값, 정규화 |
| `src-tauri/src/store.rs` | 불러오기, 손상 파일 백업, 원자적 쓰기, 일일 백업, 저장 스레드 |
| `src-tauri/src/images.rs` | 사진 저장·형식 확인·안 쓰는 사진 정리 |
| `src-tauri/src/import.rs` | StickyMemo 형식 1·2 가져오기 |
| `src-tauri/src/windows.rs` | 메모·목록 창, 접기, 핀, 띠, 전체화면, 모니터 보정 |
| `src-tauri/src/commands.rs` | IPC 명령 |
| `src-tauri/src/tray.rs` | 트레이 아이콘·메뉴 |
| `src-tauri/src/anchor.rs` | 세션 종료 저장용 숨은 창, 상단 가장자리 끌기 |
| `src-tauri/src/autostart.rs` | HKCU Run 등록·해제, 옛 StickyMemo 항목 정리 |
| `src-tauri/src/log.rs` | 시작·종료 진단 로그 |
| `ui/` | `note.*`, `list.*`, 공통 `common.css`, `ui.js`, `boot.js`, 서식 정리기 `rich.js` |
| `tests/note_ui_test.html` | 실제 메모 화면 코드에 가짜 앱을 붙인 UI 테스트 |
| `docs/` | GaeunMemo 2.0 Native 재개발 기술 분석·설계 보고서 원본(HTML). PDF는 `python docs/build_pdf.py`로 생성 (저장소에는 넣지 않음) |
| `tools/merge_claude_copy.ps1` | 다른 위치에 갈라진 notes.json 을 합치는 일회성 도구 |

## 데이터

- 메모: `%APPDATA%\gaeunmemo\notes.json` (임시 파일 → rename 원자적 쓰기). 서식 있는 본문(`html`)과 평문 사본(`text`)을 함께 저장해서 1.0에서도 글자는 읽힌다
- 사진: `%APPDATA%\gaeunmemo\images\<uuid>.<png|jpg|gif|webp>`
- 백업: `notes-backup-YYYYMMDD.json` 최근 7개, 손상 파일은 `notes-broken-<시각>.json`
- 로그: `%APPDATA%\gaeunmemo\gaeunmemo.log` (메모 내용은 기록하지 않음)
