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
| `src/events.ts` | `routeEvent(ev) -> "tree" \| "tick" \| "ignore"`, `debounceTrailing`, `throttleLeading` | `events.test.ts` |
| `src/keys.ts` | `mapInput(data) -> {keys} \| {text} \| {drop, reason}` | `keys.test.ts` |
| `src/dialogue.ts` | `stripAnsi`, `leadStyle`, `classify`, `promptLine`, `blockWidth(lines, cols, widthOf)`, `defaultWidthOf`, `stripBackground` | `dialogue.test.ts` + `src/__fixtures__/pane-read/*.ans` / `*.expected.json` |
| `src/latest.ts` | `createLatestGate` — «последний запрос побеждает» для ресурсов `tree`/`accounts` | `latest.test.ts` |
| `src/App.tsx` | Ресурсы `tree`/`accounts`, сигналы `offline`/`accountsError`/`switching`/`switchError`, подписка на события, поллинг 15 с, выбор текущей панели | — |
| `src/components/TerminalView.tsx` | xterm + `pane.read`-снапшоты, рамки диалога, курсор, ввод, R14' hold-while-reading | ручная проверка |
| `src/tree.ts` | Модель дерева сайдбара: repo › space › tab › session, rollup статуса, правила раскрытия, полоса WAITING, open-state в `localStorage` | `tree.test.ts` |
| `src/components/Sidebar.tsx`, `TabBar.tsx`, `KeyBar.tsx` | Дерево сессий/аккаунты/карта сессий, вкладки, нижняя строка подсказок | — |
| `scripts/fake-herdr.mjs` | Фальшивый сервер API для ручных проверок моста (`silent`, `slow <ms>`, `ack-error`, `close-after-ack`, `ok`) | — |

## Локальное состояние (M4)

Только UI-настройки: размер и положение окна (`tauri_plugin_window_state`, файл в каталоге данных
приложения) и последняя выбранная панель (`localStorage`, ключ `deck.selectedPane`; `App.tsx`
`remember`/`remembered`). Если панели больше нет, выбор падает на сфокусированную панель
сфокусированного workspace, как и раньше. Состояние сессии (workspaces, панели, вывод) по-прежнему
живёт только в Herdr.

## Режим без Tauri

`src/tauri.ts` экспортирует `invoke`/`listen`/`getCurrentWindow`: внутри окна Tauri это реальные
функции `@tauri-apps/api`; в обычном браузере (нет `window.__TAURI_INTERNALS__`) — заглушка в памяти,
отвечающая на те же команды синтетическим деревом, растущим транскриптом с курсором и мутациями по
действиям M3. Нужна для визуальной проверки UI без Herdr; поведения, отличного от Rust-стороны,
в ней нет.

## Обновления (M5)

`plugins.updater` в `tauri.conf.json`: `pubkey` (minisign, пара к `~/.tauri/netrunner-deck.key`
у владельца репозитория) и один endpoint — `releases/latest/download/latest.json` на GitHub.
`release.yml` собирает с `TAURI_SIGNING_PRIVATE_KEY{,_PASSWORD}` из секретов, кладёт в релиз
переименованный инсталлятор `netrunner-deck_<ver>_x64-setup.exe`, его `.sig` и `latest.json`
(`platforms."windows-x86_64".{url,signature}`). Webview (`App.tsx`) вызывает `update_check` при
старте и раз в 4 ч (кроме `tauri dev`), показывает кнопку в титлбаре, установка — только по клику
(`update_install`). Скачивание идёт из Rust, CSP webview не затрагивается.

## Дерево сайдбара (M6)

Одно дерево вместо списка Spaces и отсортированного по статусу списка Agents. Уровни и цвета:
репозиторий (жёлтый, группировка по `worktree.repo_key`, подпись `repo_name`) › workspace
(красный; `root` — основной чекаут, `⎇` — linked worktree, по `worktree.is_linked_worktree`) ›
таб (бирюзовый) › сессия (панель). Таб с одной панелью схлопывается в строку сессии: имя таба,
вид агента, ctx-бар, слово статуса только там, где точка его не несёт (WAITING, DONE, SHELL).
Таб с `pane_count > 1` остаётся строкой с кареткой и перечисляет сессии уровнем ниже.
Workspace без `worktree` стоит на верхнем уровне без репозитория.

Статусы: для таба и workspace берутся агрегаты Herdr (`agent_status`), для репозитория — rollup
по workspace (`blocked` › `working` › `idle` › `done` › `unknown`). Порядок везде серверный.
Срочность — закреплённая полоса WAITING над деревом (все `blocked` сессии с путём
`repo / space / tab`, повторяющиеся имена схлопываются), F2 ходит по ней. Раскрытие: по умолчанию
открыты узлы со статусом `blocked`/`working` и узел с выбранной панелью; ручные переключения —
`localStorage` `deck.tree.open` (`{id: bool}`), перекрывают умолчание. Создание на любом уровне,
как в Herdr: `+` у репозитория → `workspace.create` с `source_workspace_id` корневого чекаута,
у workspace → `tab.create`, у сессии/таба → `pane.split` (`target_pane_id`, `direction`
right|down, `focus`). Вся логика без DOM — в `src/tree.ts` с тестами; `Sidebar.tsx` только рисует.
Сами сплиты Deck по-прежнему не рендерит рядом: выбранная панель одна, остальные — через дерево.

## Действия и хоткеи (M3)

`src/actions.ts` (чистый, с тестами): `hotkeyOf` — F1 (пикер аккаунта), F2 (следующий агент в
`blocked`, по кругу, `nextBlockedPane`), Ctrl+K (палитра), Esc только при открытом оверлее;
`commands` — пункты палитры из дерева и аккаунтов. `App.tsx` слушает `keydown` на `window` в
фазе захвата и гасит событие, а `TerminalView` дополнительно отфильтровывает эти клавиши через
`attachCustomKeyEventHandler`, так что в PTY они не попадают. Контекстные меню
(`components/ContextMenu.tsx`): workspace — focus / new tab / new space / close (+ close group для
primary-репозитория с worktree); tab — focus / new tab / close; агент — open / focus / close pane;
аккаунт — switch / refresh. Деструктивные пункты требуют второго клика в течение 3 с. Ошибки
действий — в заголовке окна на 8 с; `workspace_group_close_required` объясняется словами.

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
| `herdr_action` | `workspace.create` / `workspace.close` / `tab.create` / `tab.close` / `pane.close` | M3; имя действия → метод и фиксированный набор параметров (`action_request`): create всегда `focus: true`, из `options` проходят только `cwd`, `label`, `close_group`; неизвестное действие → `Shape` |
| `notify` | OS notification через `tauri-plugin-notification` | M4; решение «когда» принимает webview (агент перешёл в `blocked`, его панель не на экране или окно не в фокусе) |
| `update_check` | `tauri-plugin-updater` → `latest.json` релиза | M5; возвращает `{version,current,notes,date}` или `null`; найденный `Update` хранится в `PendingUpdate` |
| `update_install` | скачивание, проверка подписи, запуск NSIS (passive) | M5; только по клику пользователя |
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
`pane_output_changed` (форк, M1.5) → `"tick"`, только если `pane_id` совпадает с панелью
на экране; для других панелей — `"ignore"` (метаданных оно не несёт). `"tick"` → счётчик
`outputTick` в `App` → `TerminalView` читает сразу, а при потоке событий — не чаще раза в
100 мс (`throttleLeading`, хвост всплеска не теряется). Любое событие также снимает баннер
OFFLINE.

### Pane view (`TerminalView.tsx`)

- Снапшот `pane.read` (`lines = max(200, rows*4)`) по тику вывода (сразу, далее ≤ 1 раз в
  100 мс), по поллингу 2,5 с и через 120 мс после собственного ввода; один запрос в полёте,
  повторная просьба ставит `dirty`. Ответ для другой
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

Шрифт Rajdhani (веса 400/600/700, `src/fonts/*.ttf`, OFL 1.1) подключён через `@font-face` в
`styles.css`; Vite кладёт файлы в `dist/assets`, так что `font-src 'self'` покрывает их без
изменений CSP. Сетевых запросов за шрифтом нет.

## Факты об API Herdr (проверены по исходникам форка `messire/herdr-punk`, 2026-10-24)

| # | Факт | Источник |
|---|---|---|
| F1 | События приходят в поле `event` в **snake_case** (`pane_updated`, `workspace_created`); подписка принимает точечные имена (`pane.updated`). | `src/api/schema/events.rs:115,193-194`; `tests/subscription_socket_tests.rs:171` |
| F2 | `pane_updated` — событие **метаданных** (имя агента, токены, заголовок), payload `pane: PaneInfo`; на смену label/title приходит. | `events.rs:500-502`; `app/api.rs:696`; `terminal_titles.rs:73` |
| F2a | Глобальных подписок на `pane.agent_status_changed`, `pane.scroll_changed`, `pane.output_matched` нет — они требуют `pane_id`; подписка с неизвестным/неполным именем отвергается целиком. | `events.rs:60-66`; проверено 2026-10-24 |
| F3 | `pane.output_changed` (`pane_id`, `workspace_id`, `revision`): upstream **не эмитит** и не даёт подписки. **Форк (M1.5)**: подписка `pane.output_changed` (глобальная, без фильтра), эмиссия из headless-цикла по PTY-damage, коалесцирование ≤ 1 события / 100 мс / панель с хвостовой доставкой, `revision = TerminalRuntime::content_seq()`. Pump Deck пробует подписку с ним и при `Api`-ошибке переподключается без него (upstream-совместимость). | форк `src/app/output_events.rs`, `src/server/headless.rs`, `src/api/subscriptions.rs`; Deck `src-tauri/src/lib.rs` |
| F4 | `pane.read` → `{read:{pane_id, workspace_id, tab_id, source, format, text, revision, truncated}}`; `revision`: upstream всегда `0`, форк — `content_seq` (чётное = стабильное содержимое, нечётное = запись в процессе); `lines` ≤ 1000; `format=ansi` — отрендеренные строки экрана, не сырой поток. Deck пропускает чтение с уже применённой чётной ревизией. | `schema/panes.rs:729-738`; `app/api/panes.rs:1543-1548`; `api_helpers.rs:117` |
| F5 | `pane.send_text` оборачивает текст в bracketed paste (`\x1b[200~…\x1b[201~`), если панель его включила (Claude Code включает). Escape-последовательности через `send_text` приходят как вставка. | `app/api_helpers.rs:25-32` |
| F6 | `pane.send_keys` принимает имена `config::parse_key_combo` (`keybinds.rs:1281-1343`): модификаторы `ctrl/shift/alt/meta/super/hyper`, ключи `space enter return esc tab backspace left right up down minus comma period slash backslash quote semicolon colon percent ampersand backtick plus`, одиночный символ, `f<N>`; `shift+tab` → backtab. Upstream **не знает** `home/end/pageup/pagedown/insert/delete` (`invalid_key`); форк (M1.5) принимает `home end pageup|page_up|pgup pagedown|page_down|pgdn insert|ins delete|del`, Deck шлёт канонические имена (`src/keys.ts`). | `src/config/keybinds.rs:1281-1343`; `app/api/panes.rs:2046`; проверено R9 |
| F7 | У Herdr есть собственный thin-client протокол (`PaneSurface`/`PaneSurfacePatch`, ресайз, дельты, реконнект) — «endpoint generation 1», стабильный контракт. | `src/protocol/{endpoint,wire,surface_delta}.rs`; `tests/client_mode.rs` |
| F8 | Серверный listener — `interprocess` local socket; параллельные подключения на Windows могут дать `ERROR_PIPE_BUSY`. | `src/api/server.rs:7` |
| F9 | Есть `pane.wait_for_output` (long-poll по паттерну) и `events.wait`. | `schema.rs:133`; `events.rs:95-112` |
| F10 | CLI: `herdr agent read <pane> --source recent --format ansi --ansi --lines N` даёт сырой ANSI (флаг `--ansi`, не `--raw`). Вывод CLI — JSON по умолчанию (`herdr agent list`), флага `--json` нет. | проверено запуском 2026-10-24 |
| U3 | `cswap` на машине пользователя — `C:\Users\<user>\.local\bin\cswap.exe` (не `.cmd`-шим). | `where cswap`, 2026-10-24 |

## Снапшот-режим после M2

- Курсор: форк отдаёт в `pane.read` поле `cursor` (`line` — индекс строки среди строк `text`,
  `col`, `visible`, `shape`) и `alternate_screen`; Deck ставит курсор в поле ввода по
  `cursor.line − <начало поля ввода>`. На upstream поля нет — курсор ставится в конец строки
  промпта `❯` (оценка).
- Снапшоты применяются инкрементально (`src/render-plan.ts`): буфер xterm хранит всё написанное
  как историю, новый снапшот выравнивается с ним (сдвиг окна + общий префикс) и переписывается
  только изменившийся хвост в пределах viewport. `term.reset()` — только первый показ, смена
  панели, ресайз; лишь эти случаи R14' откладывает, пока пользователь читает историю или держит
  выделение.
- Триггер обновления — собственный ввод (120 мс debounce), `pane_output_changed` (форк:
  коалесценция ≤ 1/100 мс на сервере; в Deck — чтение сразу, затем throttle 100 мс) и поллинг:
  2,5 с, пока не пришёл первый tick, затем 10 с как страховка. На upstream — только поллинг.
  Замер 2026-11-04 на живом Herdr: событие приходит через ~190 мс после ввода, `pane.read`
  отвечает 2 или ~103 мс — API-сервер Herdr опрашивает pipe раз в 100 мс
  (`CONNECTION_POLL_INTERVAL`) и так же раз в 100 мс сбрасывает события подписчикам. Прежний
  trailing-debounce 120 мс сбрасывался каждым событием и при непрерывном выводе не срабатывал.
- Ширины символов для рамок — из таблицы `defaultWidthOf`, не из буфера xterm (остаток п.11).
- На upstream Herdr навигационные клавиши не доходят до панели (F6); на форке доходят.

## Переменные окружения

- `HERDR_SOCKET_PATH` — явный путь к сокету/пайпу Herdr (на Windows — имя пайпа без `\\.\pipe\`).
- `HERDR_CONFIG_DIR` — каталог конфигурации Herdr (по умолчанию `%APPDATA%\herdr`; на unix
  `$XDG_CONFIG_HOME/herdr` или `~/.config/herdr`).
- `RUST_LOG` — фильтр `tracing` Rust-стороны (по умолчанию `info`).

## Открытые вопросы

- Снапшоты с живого Herdr (U2) для фикстур не добавлены; текущие фикстуры синтетические.
- `ERROR_PIPE_BUSY` при 2 параллельных вызовах не замерялся (ROADMAP, M2 вопрос 4).
- Снапшот при `alternate_screen: true` (vim/less в панели) Deck рисует как обычный текст;
  отдельного режима нет.
