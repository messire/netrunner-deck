# Netrunner Deck — архитектура (черновик, день 0)

Deck — тонкий клиент к серверу Herdr. Всё состояние (workspaces, tabs, panes,
агенты, вывод) живёт в Herdr; Deck ничего не хранит.

## Поток данных

```
Solid UI ──invoke──▶ Tauri command (src-tauri/src/lib.rs)
                        │  одно соединение на вызов
                        ▼
              Herdr JSON API (newline-delimited JSON)
              Windows: \\.\pipe\<config dir>\herdr.sock   unix: <config dir>/herdr.sock
                        │
   events.subscribe ────┘  долгое соединение в фоновом потоке → Tauri event `herdr://event`
```

Pane view в M1 — снимки `pane.read` (поллинг 2,5 с + после собственного ввода),
отрисованные в xterm.js; рамки диалога — xterm decorations по эвристикам
`src/dialogue.ts`.

## Факты об API Herdr (проверены по исходникам форка `messire/herdr-punk`, 2026-10-24)

| # | Факт | Источник |
|---|---|---|
| F1 | События приходят в поле `event` в **snake_case** (`pane_updated`, `workspace_created`); подписка принимает точечные имена (`pane.updated`). | `src/api/schema/events.rs:115,193-194`; `tests/subscription_socket_tests.rs:171` |
| F2 | `pane_updated` — событие **метаданных** (имя агента, токены, заголовок), payload `pane: PaneInfo`; на смену label/title приходит. | `events.rs:500-502`; `app/api.rs:696`; `terminal_titles.rs:73` |
| F3 | `pane.output_changed` (`pane_id`, `workspace_id`, `revision`) есть в схеме, но сервером **не эмитится**. События вывода нет. | `events.rs:521-525` |
| F4 | `pane.read` → `{read:{pane_id, workspace_id, tab_id, source, format, text, revision, truncated}}`; `revision` всегда `0`; `lines` ≤ 1000; `format=ansi` — отрендеренные строки экрана, не сырой поток. | `schema/panes.rs:729-738`; `app/api/panes.rs:1543-1548`; `api_helpers.rs:117` |
| F5 | `pane.send_text` оборачивает текст в bracketed paste (`\x1b[200~…\x1b[201~`), если панель его включила (Claude Code включает). Escape-последовательности через `send_text` приходят как вставка. | `app/api_helpers.rs:25-32` |
| F6 | `pane.send_keys` принимает имена `config::parse_key_combo` (`enter`, `tab`, `backspace`, `esc`, стрелки, `home/end/pageup/pagedown/insert/delete`, модификаторы `ctrl+`/`alt+`/`shift+`); `shift+tab` → backtab. | `api_helpers.rs:11-23`; `src/input/parse.rs:516-530`; тест `app/api/panes.rs:2457` |
| F7 | У Herdr есть собственный thin-client протокол (`PaneSurface`/`PaneSurfacePatch`, ресайз, дельты, реконнект) — «endpoint generation 1», стабильный контракт. | `src/protocol/{endpoint,wire,surface_delta}.rs`; `tests/client_mode.rs` |
| F8 | Серверный listener — `interprocess` local socket; параллельные подключения на Windows могут дать `ERROR_PIPE_BUSY`. | `src/api/server.rs:7` |
| F9 | Есть `pane.wait_for_output` (long-poll по паттерну) и `events.wait`. | `schema.rs:133`; `events.rs:95-112` |
| F10 | CLI: `herdr agent read <pane> --source recent --format ansi --ansi --lines N` даёт сырой ANSI (флаг `--ansi`, не `--raw`). Вывод CLI — JSON по умолчанию (`herdr agent list`), флага `--json` нет. | проверено запуском 2026-10-24 |
| U3 | `cswap` на машине пользователя — `C:\Users\<user>\.local\bin\cswap.exe` (не `.cmd`-шим). | `where cswap`, 2026-10-24 |

## Ограничения снапшот-режима (до M2)

- Нет позиции курсора: Deck ставит курсор в конец строки промпта `❯` (оценка).
- Каждый снапшот перерисовывается целиком (`term.reset()`): прокрутка/выделение сбрасываются.
- Триггер обновления — только поллинг и собственный ввод (см. F3).

## Переменные окружения

- `HERDR_SOCKET_PATH` — явный путь к сокету/пайпу Herdr.
- `HERDR_CONFIG_DIR` — каталог конфигурации Herdr (по умолчанию `%APPDATA%\herdr` / `~/.config/herdr`).
- `RUST_LOG` — уровень логирования Rust-стороны (по умолчанию `info`).
