# Netrunner Deck — архитектура (состояние после M1b, 2026-10-24)

Deck — тонкий клиент к серверу Herdr. Всё состояние сессии (workspaces, tabs, panes,
агенты, вывод) живёт в Herdr; Deck не хранит и не кэширует его между запусками.
Локальное хранение допускается только для настроек UI, и то начиная с M4
(`docs/plan/ROADMAP.md` §1); на сегодня его нет.

## Карта модулей

| Модуль | Роль | Тесты |
|---|---|---|
| `src-tauri/src/herdr.rs` | Клиент JSON API Herdr: `socket_path`/`config_dir`, `call` (дедлайн + слоты), `subscribe` (долгое соединение), `parse_reply`, `classify_line` | `cargo test` (дедлайн, семафор, разбор ответов/строк потока) |
| `src-tauri/src/lib.rs` | Tauri-команды `herdr_*`, `cswap_*`; резолв пути `cswap` при старте; event pump `start_event_pump`; `run()` | `cargo test` (`parse_json_stdout`, `focus_method`, кандидаты `cswap`) |
| `src/api.ts` | Типы `Tree`/`Pane`/`Agent`/`CswapList`, обёртки `invoke`, `onEvent`/`onOffline`, DEV-счётчики вызовов `window.__deckCalls` | — |
| `src/events.ts` | `routeEvent(ev) -> "tree" \| "ignore"`, `debounceTrailing` | `events.test.ts` |
| `src/keys.ts` | `mapInput(data) -> {keys} \| {text} \| {drop, reason}` | `keys.test.ts` |
| `src/dialogue.ts` | `stripAnsi`, `leadStyle`, `classify`, `promptLine`, `blockWidth(lines, cols, widthOf)`, `defaultWidthOf`, `stripBackground` | `dialogue.test.ts` + `src/__fixtures__/pane-read/*.ans` / `*.expected.json` |
| `src/latest.ts` | `createLatestGate` — «последний запрос побеждает» для ресурсов `tree`/`accounts` | `latest.test.ts` |
| `src/App.tsx` | Ресурсы `tree`/`accounts`, сигналы `offline`/`accountsError`/`switching`/`switchError`, подписка на события, поллинг 15 с, выбор текущей панели | — |
| `src/components/TerminalView.tsx` | xterm + `pane.read`-снапшоты, рамки диалога, курсор, ввод, R14' hold-while-reading | ручная проверка |
| `src/components/Sidebar.tsx`, `TabBar.tsx`, `KeyBar.tsx` | Дерево/аккаунты/агенты/карта сессий, вкладки, нижняя строка подсказок | — |
| `scripts/fake-herdr.mjs` | Фальшивый сервер API для ручных проверок моста (`silent`, `slow <ms>`, `ack-error`, `close-after-ack`, `ok`) | — |

## Поток данных

```
Solid UI ──invoke──▶ #[tauri::command] async (lib.rs)
                        │  spawn_blocking → herdr::call
                        │  одно соединение на вызов, дедлайн 15 с, ≤ 2 параллельных
                        ▼
              Herdr JSON API (newline-delimited JSON)
              Windows: \\.\pipe\<config dir>\herdr.sock   unix: <config dir>/herdr.sock
                        │
   events.subscribe ────┘  долгое соединение в отдельном std-потоке
                           → Tauri event `herdr://event` (каждое событие как есть)
                           → `herdr://offline` (текст ошибки) при обрыве/отказе, реконнект через 2 с
```

### Команды (`lib.rs`)

Все команды — `async fn`, тело выполняется через `tauri::async_runtime::spawn_blocking`
(хелпер `blocking()`), чтобы I/O по пайпу не занимал ни главный поток, ни немногочисленные
tokio-воркеры.

| Команда | Методы Herdr / процесс | Примечание |
|---|---|---|
| `herdr_ping` | `ping` | проба «Herdr запущен» |
| `herdr_tree` | `workspace.list`, `tab.list` (на каждый workspace), `pane.list`, `agent.list` | падение любого `tab.list` — ошибка всей команды, не пустой tab bar (аудит п.15) |
| `herdr_pane_read` | `pane.read` с `source: recent`, `format: ansi`, `strip_ansi: false`, `lines` (по умолчанию 400) | |
| `herdr_pane_send_text` | `pane.send_text` | только печатный текст (см. F5) |
| `herdr_pane_send_keys` | `pane.send_keys` | имена из `src/keys.ts` |
| `herdr_focus` | `workspace.focus` / `tab.focus` / `pane.focus` | неизвестный target → `HerdrError::Shape` до обращения к серверу (`focus_method`) |
| `cswap_list` | `cswap list --json` | |
| `cswap_switch` | `cswap switch <n> --json` | |

`cswap` резолвится один раз в `run()`: каталоги `PATH`, затем `~/.local/bin`
(`USERPROFILE`/`HOME`); на Windows имена — расширения из `PATHEXT`, затем `cswap.exe`,
`cswap.cmd`, `cswap`. Результат хранится в `tauri::State<CswapPath>`; `None` → обе команды
отвечают `cswap not found (searched PATH and ~/.local/bin)`. `run_json` проверяет код
выхода (stderr попадает в ошибку, до 300 символов) и берёт JSON с первой строки,
начинающейся на `{` — предупреждения перед JSON не мешают.

### Дедлайн и слоты (`herdr.rs`, R3)

- `CALL_TIMEOUT = 15 s` на один request/reply, включая ожидание слота. Действует на всех
  платформах: у `std::fs::File` (пайп на Windows) нет read timeout, поэтому дедлайн держит
  вызывающая сторона — `run_with_deadline`: работа в helper-потоке `herdr-call:<method>` +
  `mpsc::recv_timeout`. На unix дополнительно ставится socket read timeout 20 с.
- `MAX_CONCURRENT_CALLS = 2` — счётный семафор на `std::sync` (`Semaphore`/`SlotGuard`),
  чтобы не ловить `ERROR_PIPE_BUSY` (F8).
- По таймауту helper-поток **не убивается** (безопасно прервать блокирующий `read` нельзя):
  он держит слот, пока пайп не ответит или не закроется; поздний результат отбрасывается.
  Число брошенных потоков ограничено числом слотов: при занятых слотах новые вызовы падают
  с `Timeout` на дедлайне, не подключаясь. Мутирующий запрос, ушедший в таймаут, сервер
  может выполнить позже — теряется только ответ.
- `subscribe` слотов не берёт, дедлайна и read timeout не имеет (простой поток — норма).

### Event pump (`lib.rs::start_event_pump`, R4)

Отдельный `std::thread` в цикле вызывает `herdr::subscribe` со списком подписок (имена должны
точно совпадать с `Subscription` Herdr — неизвестное имя отвергает подписку целиком):

```
workspace.created  workspace.updated  workspace.renamed  workspace.closed  workspace.focused
tab.created  tab.closed  tab.focused  tab.renamed  tab.moved
pane.created  pane.closed  pane.updated  pane.focused  pane.agent_detected
layout.updated
```

`pane.agent_status_changed` не подписывается: это per-pane подписка (нужен `pane_id`),
а смена статуса и так приходит как `pane_updated` (F2, F2a).

Разбор потока — чистая `classify_line(&str) -> Ack | Event | Error | Skip`: первая осмысленная
строка — ack `{"id","result"}`; `{"id","error"}` → `Err(Api)`; `{"event",…}` → в webview;
мусор/пустые строки пропускаются. `subscribe` возвращает `Ok(())` только когда webview
перестал принимать события; любой другой выход (`Api`, EOF «event stream closed», `Io`,
`Connect`) → `herdr://offline` с текстом и повтор через 2 с.

На стороне UI (`src/events.ts`): событие приходит как `{event: "<snake_case>", …}`;
`routeEvent` возвращает `"tree"` для `pane_created|closed|updated|focused|moved|exited|
agent_detected|agent_status_changed`, `layout_updated` и любого имени с префиксом
`workspace_`/`worktree_`/`tab_`; остальное — `"ignore"`. Точечное написание терпимо
(`.` → `_`). Все `"tree"`-события → один `herdr_tree` через `debounceTrailing(…, 200 мс)`.
Маршрута `"tick"` (обновление панели) нет: события вывода у Herdr нет (F3); он появится
вместе с `pane_output_changed` из M1.5. Любое событие также снимает баннер OFFLINE.

### Pane view (`TerminalView.tsx`)

- Снапшот `pane.read` (`lines = max(200, rows*4)`) по поллингу 2,5 с и через 120 мс после
  собственного ввода; один запрос в полёте, повторная просьба ставит `dirty`. Ответ для другой
  панели (`id !== p.paneId`) или после `onCleanup` отбрасывается.
- Отрисовка: `term.reset()` + полная запись; рамки диалога — xterm decorations по
  `classify`; курсор — в конец строки промпта `promptLine` (оценка, позиции курсора в
  снапшоте нет).
- **R14' hold-while-reading:** если `term.hasSelection()` или viewport не внизу
  (`viewportY < baseY`), новый снапшот откладывается в сигнал `held`, в заголовке панели
  показывается `NEW OUTPUT ↓ (scroll down)`; `onScroll`/`onSelectionChange` применяют
  отложенный текст, как только пользователь вернулся вниз и снял выделение. Индикатора
  непрочитанного и дельта-рендера нет — это M2.

### Клавиши (`src/keys.ts`, R9)

`mapInput(data)` над `xterm.onData`:

- печатный текст без ESC/контрольных байт (кроме `\t\n\r` внутри вставки) → `{text}` →
  `pane.send_text`;
- одиночные `\r`/`\t`/`\n`, C0-байты (`ctrl+a…z`, `ctrl+@`, `ctrl+\`, `ctrl+]`, `ctrl+^`,
  `ctrl+_`), стрелки, `\x1b[Z` → `shift+tab`, F1–F12 (SS3 и CSI-формы), модификаторы
  `ctrl/alt/shift` из `CSI 1;N` / `CSI n;N ~`, `alt+<символ>` и `alt+<имя>` → `{keys}` →
  `pane.send_keys`;
- `home end insert delete pageup pagedown` (в любой форме и с модификаторами) → `{drop}`:
  у `parse_key_combo` форка нет этих имён, сервер ответил бы `invalid_key` (F6). Снимается
  патчем форка (M1.5), после чего убрать `UNNAMED_IN_HERDR`/`NAV`;
- всё нераспознанное → `{drop, reason}` + `console.debug` — через `send_text` оно ушло бы как
  вставка (F5).

### Эвристики диалога (`src/dialogue.ts`, R11–R13')

- `classify(lines, raw)`: начало блока `user` — строка `> текст`/`❯ текст`/`› текст`, кроме
  строки живого промпта (`promptLine`); начало блока `agent` — `● текст`/`⏺ текст`, если
  `текст` не похож на вызов инструмента (`Name(` — `TOOL_CALL`) или сводку (`Ran|Read|
  Searched|…` — `TOOL_SUMMARY`) и стиль первого печатного символа не dim и не red.
  Продолжение — строки с отступом ≥ 2, кроме сводок, результатов `⎿└├│`, статусов `※✻✽✶✳·`
  и dim-строк; пустая строка внутри блока допустима, если за ней идёт не-dim продолжение.
- `leadStyle(raw)` (R12): берёт префикс до первого печатного символа (`LEAD` — SGR/CSI и
  пробелы), применяет SGR по порядку: `2` → faint; `90` → dim; `31/91` → red; `38;5;n` —
  dim для 8 и 232–255, red для 1/9/196/160/203; `38;2;r;g;b` — dim при относительной
  яркости < 0.35, red при `r>180, g<120, b<120`; `0`/`39` сбрасывают.
- `blockWidth(lines, cols, widthOf = defaultWidthOf)` (R13'): ширина блока в ячейках;
  `defaultWidthOf` — компактная таблица East Asian Width (CJK, хангыль, кана, fullwidth,
  эмодзи → 2; combining/VS/ZWJ → 0). Полный переход на ширины из буфера xterm — M2.
- `stripBackground` снимает фоновые цвета (серая полоса под user-turn в Claude Code).
- Фикстуры `src/__fixtures__/pane-read/synthetic-claude-{basic,long-reply,tools,prompt-wrap}.ans`
  синтетические; `*.expected.json` задаёт границы блоков `[{kind, from, to}]`, `neverFramed`
  и `promptLine`. Снапшотов с живого Herdr (U2) в репозитории нет.

### CSP (`src-tauri/tauri.conf.json`, R15)

Релиз: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'
data:; font-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost`
(`'unsafe-inline'` в стилях нужен xterm.js). `devCsp` дополнительно разрешает
`http://localhost:1420` и `ws://localhost:1420` для Vite HMR.

## Факты об API Herdr (проверены по исходникам форка `messire/herdr-punk`, 2026-10-24)

| # | Факт | Источник |
|---|---|---|
| F1 | События приходят в поле `event` в **snake_case** (`pane_updated`, `workspace_created`); подписка принимает точечные имена (`pane.updated`). | `src/api/schema/events.rs:115,193-194`; `tests/subscription_socket_tests.rs:171` |
| F2 | `pane_updated` — событие **метаданных** (имя агента, токены, заголовок), payload `pane: PaneInfo`; на смену label/title приходит. | `events.rs:500-502`; `app/api.rs:696`; `terminal_titles.rs:73` |
| F2a | Глобальных подписок на `pane.agent_status_changed`, `pane.scroll_changed`, `pane.output_matched` нет — они требуют `pane_id`; подписка с неизвестным/неполным именем отвергается целиком. | `events.rs:60-66`; проверено 2026-10-24 |
| F3 | `pane.output_changed` (`pane_id`, `workspace_id`, `revision`) есть в схеме, но сервером **не эмитится**. События вывода нет. | `events.rs:521-525` |
| F4 | `pane.read` → `{read:{pane_id, workspace_id, tab_id, source, format, text, revision, truncated}}`; `revision` всегда `0`; `lines` ≤ 1000; `format=ansi` — отрендеренные строки экрана, не сырой поток. | `schema/panes.rs:729-738`; `app/api/panes.rs:1543-1548`; `api_helpers.rs:117` |
| F5 | `pane.send_text` оборачивает текст в bracketed paste (`\x1b[200~…\x1b[201~`), если панель его включила (Claude Code включает). Escape-последовательности через `send_text` приходят как вставка. | `app/api_helpers.rs:25-32` |
| F6 | `pane.send_keys` принимает имена `config::parse_key_combo` (`keybinds.rs:1281-1343`): модификаторы `ctrl/shift/alt/meta/super/hyper`, ключи `space enter return esc tab backspace left right up down minus comma period slash backslash quote semicolon colon percent ampersand backtick plus`, одиночный символ, `f<N>`; `shift+tab` → backtab. **Не знает** `home/end/pageup/pagedown/insert/delete` — на них сервер отвечает `invalid_key`; Deck такие клавиши отбрасывает (`src/keys.ts`) до патча форка. | `src/config/keybinds.rs:1281-1343`; `app/api/panes.rs:2046`; проверено R9 |
| F7 | У Herdr есть собственный thin-client протокол (`PaneSurface`/`PaneSurfacePatch`, ресайз, дельты, реконнект) — «endpoint generation 1», стабильный контракт. | `src/protocol/{endpoint,wire,surface_delta}.rs`; `tests/client_mode.rs` |
| F8 | Серверный listener — `interprocess` local socket; параллельные подключения на Windows могут дать `ERROR_PIPE_BUSY`. | `src/api/server.rs:7` |
| F9 | Есть `pane.wait_for_output` (long-poll по паттерну) и `events.wait`. | `schema.rs:133`; `events.rs:95-112` |
| F10 | CLI: `herdr agent read <pane> --source recent --format ansi --ansi --lines N` даёт сырой ANSI (флаг `--ansi`, не `--raw`). Вывод CLI — JSON по умолчанию (`herdr agent list`), флага `--json` нет. | проверено запуском 2026-10-24 |
| U3 | `cswap` на машине пользователя — `C:\Users\<user>\.local\bin\cswap.exe` (не `.cmd`-шим). | `where cswap`, 2026-10-24 |

## Ограничения снапшот-режима (до M2)

- Нет позиции курсора: Deck ставит курсор в конец строки промпта `❯` (оценка).
- Каждый снапшот перерисовывается целиком (`term.reset()`): прокрутка/выделение сбрасываются —
  поэтому R14' откладывает снапшот, пока пользователь читает историю или держит выделение.
- Триггер обновления — только поллинг и собственный ввод (см. F3). Задержка до 2,5 с.
- Ширины символов для рамок — из таблицы `defaultWidthOf`, не из буфера xterm (остаток п.11).
- Навигационные клавиши не доходят до панели (F6).

## Переменные окружения

- `HERDR_SOCKET_PATH` — явный путь к сокету/пайпу Herdr (на Windows — имя пайпа без `\\.\pipe\`).
- `HERDR_CONFIG_DIR` — каталог конфигурации Herdr (по умолчанию `%APPDATA%\herdr`; на unix
  `$XDG_CONFIG_HOME/herdr` или `~/.config/herdr`).
- `RUST_LOG` — фильтр `tracing` Rust-стороны (по умолчанию `info`).

## Открытые вопросы

- Патч форка для `home/end/pageup/pagedown/insert/delete` и эмиссия `pane.output_changed`
  записаны в ROADMAP как M1.5; в материалах репозитория не видно, начата ли эта работа в форке.
- Снапшоты с живого Herdr (U2) для фикстур не добавлены; текущие фикстуры синтетические.
- `ERROR_PIPE_BUSY` при 2 параллельных вызовах не замерялся (ROADMAP, M2 вопрос 4).
