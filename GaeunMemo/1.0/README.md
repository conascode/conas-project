# GaeunMemo 1.0

Windows 바탕화면 스티커 메모. Rust + Tauri 2, 프런트엔드는 바닐라 HTML/CSS/JS.

- 메모 하나 = 프레임 없는 창 하나. 접기, 항상 위 고정, 6가지 파스텔 색, 띠 모양(없음/얇은 띠/제목 줄), 다크·라이트 테마
- 메모 목록 창: 검색, 최근 수정순 정렬, 키보드 조작, 우클릭 삭제
- 트레이 상주, 컴퓨터 켤 때 자동 실행, 단일 인스턴스, 작업표시줄에서 메모별 미리보기
- 자동 저장(첫 입력 즉시 + 입력 중 최대 0.3초 간격), 원자적 쓰기, 하루 한 번 백업 7개 보관

## 빌드

```
cd src-tauri
cargo test            # 저장 / 가져오기 / 모니터 보정 단위 테스트
cargo tauri build     # 릴리스 exe + NSIS 설치 파일 (target/release/bundle/nsis)
```

아이콘: `python tools/make_icon.py` 가 `src-tauri/icons/` 에 모든 크기를 만든다.

설치 파일을 다시 설치할 때는 업데이트 모드(`/UPDATE`)로 실행한다. 일반 재설치는 Tauri 설치 관리자가 자동 실행 항목을 지운다(앱이 다음 실행 때 스스로 복구하긴 한다).

## 구조

| 경로 | 내용 |
| --- | --- |
| `src-tauri/src/main.rs` | 시작 순서, 첫 실행, 자동 실행 복구, 종료 시 저장 |
| `src-tauri/src/model.rs` | 데이터 모델, 기본값, 정규화 |
| `src-tauri/src/store.rs` | 불러오기, 손상 파일 백업, 원자적 쓰기, 일일 백업, 저장 스레드 |
| `src-tauri/src/import.rs` | StickyMemo 형식 1·2 가져오기 |
| `src-tauri/src/windows.rs` | 메모·목록 창, 접기, 핀, 띠, 모니터 보정 |
| `src-tauri/src/commands.rs` | IPC 명령 |
| `src-tauri/src/tray.rs` | 트레이 아이콘·메뉴 |
| `src-tauri/src/anchor.rs` | 세션 종료 저장용 숨은 창, 상단 가장자리 끌기 |
| `src-tauri/src/autostart.rs` | HKCU Run 등록·해제, 옛 StickyMemo 항목 정리 |
| `src-tauri/src/log.rs` | 시작·종료 진단 로그 |
| `ui/` | `note.*`, `list.*`, 공통 `common.css`, `ui.js`, `boot.js` |
| `docs/` | GaeunMemo 2.0 Native 재개발 기술 분석·설계 보고서 원본(HTML). PDF는 `python docs/build_pdf.py`로 생성 (저장소에는 넣지 않음) |
| `tools/merge_claude_copy.ps1` | 다른 위치에 갈라진 notes.json 을 합치는 일회성 도구 |

## 데이터

- 메모: `%APPDATA%\gaeunmemo\notes.json` (임시 파일 → rename 원자적 쓰기)
- 백업: `notes-backup-YYYYMMDD.json` 최근 7개, 손상 파일은 `notes-broken-<시각>.json`
- 로그: `%APPDATA%\gaeunmemo\gaeunmemo.log` (메모 내용은 기록하지 않음)
