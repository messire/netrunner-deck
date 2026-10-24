<!-- feature-planner, 2026-10-24, v2. Источники: docs/audit/2026-10-24-tech-debt.md (п.1–21), docs/plan/2026-10-24-roadmap-critique.md, код src/, src-tauri/src/, build.yml, package.json, tauri.conf.json; исходники форка Herdr /home/claude/herdr-fork/herdr (feat/accounts-sidebar-section). v1 сохранён как ROADMAP.v1.md. Сборка/тесты при планировании не запускались. -->

# Netrunner Deck — Roadmap (v2)

## 1. Цель и принципы

- **Тонкий клиент.** Deck ничего не хранит и не дублирует: workspaces, tabs, panes, агенты, вывод терминала живут в Herdr. Локально допустимы только настройки UI (размер окна, выбранная панель) — начиная с M4.
- **Herdr — источник правды.** Недостающее добавляется в API форка `messire/herdr-punk`, а не обходится на стороне клиента. Форк регулярно догоняет upstream, поэтому каждое собственное изменение API — постоянная стоимость rebase: сначала проверяем, нет ли нужного в upstream (см. M2, шаг 0).
- **Никаких фич до закрытия M1a.** Аудит нашёл отладочные леса в релизе (п.1), синхронный I/O на главном потоке (п.2), необработанные ошибки старта (п.3), невоспроизводимую сборку (п.5). Критерий закрытия M1a — «окно не виснет, ошибки видны, typecheck + vitest зелёные», а не «21/21».
- **Факты отдельно от предположений.** Исходники форка доступны, поэтому семантика API сверяется в день 0 и записывается в `docs/architecture.md`; в плане такие пункты помечены **[факт]** со ссылкой на файл форка. Оставшиеся **[предположение]** — явно.

Исполнители — агенты проекта (`.claude/agents/`): `code-quality-reviewer`, `tdd-test-engineer`, `debug-healer`, `ui-builder`, `architecture-documenter`. Отдельного Rust/CI-агента нет: Rust-мост и инфраструктуру ведёт `debug-healer` (skills `rust-engineer`, `devops-engineer`), логику с тестами — `tdd-test-engineer`.

Ограничения стенда **[факт]**: Tauri-сборка, `cargo` с crates.io и живой Herdr доступны только на Windows-машине пользователя; реализатор в этом окружении не может собрать Tauri и не имеет сети к crates.io. Поэтому параллельность — два потока («Rust», «UI»), ручные проверки — серийная очередь у пользователя, оценки ниже это учитывают.

---

## 2. День 0 — факты из форка и подготовка стенда

Выполняется до R2a. Результат — `docs/architecture.md` (черновик; дописывается в R16).

### 2.1 Факты из исходников форка (записать в `docs/architecture.md`)

| # | Факт | Источник в форке | Влияет на |
|---|---|---|---|
| F1 | События в проводе идут в `snake_case` в поле `event`: `pane_updated`, `workspace_created`, … Подписка принимает точечные имена (`pane.updated`). Текущий `kind.startsWith("pane.updated")` в `App.tsx:33` никогда не истинен → каждое событие делает `refetchTree`, `tick` не поднимается вовсе. | `src/api/schema/events.rs:115,193-194,244`; тест `tests/subscription_socket_tests.rs:171` | R7, аудит п.7 (диагноз «jank от pane.read каждые 120 мс» неверен) |
| F2 | `pane_updated` — событие **метаданных** (имя агента, токены, заголовок), несёт `pane: PaneInfo`, без `pane_id` на верхнем уровне; на смену label/title приходит. | `events.rs:500-502`; `app/api.rs:696`, `terminal_titles.rs:73`, `runtime.rs:66`, `agents.rs:60,136` | R7 |
| F3 | `pane.output_changed` (`pane_id`, `workspace_id`, `revision`) есть в схеме, но **не эмитится** приложением — только в тестах. События вывода нет; pane view обновляется только поллингом и после собственного ввода. | `events.rs:173,216,247,521-525` | R7, M1.5 |
| F4 | `pane.read` возвращает `{read:{pane_id, workspace_id, tab_id, source, format, text, revision, truncated}}`; `revision` захардкожен `0`; `lines` режется до 1000; ANSI — отрендеренные строки экрана (ghostty snapshot), не сырой байтовый поток. Текущий `rowsByLine()` и `replace(/\r\n?/g)` корректны. | `schema/panes.rs:729-738`; `app/api/panes.rs:1543-1548`; `api_helpers.rs:117` | R8, R11, M2 |
| F5 | `pane.send_text` оборачивает текст в bracketed paste (`\x1b[200~…\x1b[201~`), если панель его включила (Claude Code включает). Всё, что Deck шлёт через `sendText` как escape-последовательность (Shift+Tab `\x1b[Z`, F-клавиши, Ctrl+стрелки), приходит в CLI как вставленный текст. | `app/api_helpers.rs:25-32` | R9 |
| F6 | `pane.send_keys` парсит имена через `config::parse_key_combo`; `shift+tab` → backtab. | `api_helpers.rs:11-23`; тест `app/api/panes.rs:2457` | R9 |
| F7 | У Herdr есть собственный thin-client протокол: `src/protocol/{endpoint,wire,surface_delta,surface_scroll,render_ansi}.rs`, сообщения `PaneSurface`/`PaneSurfacePatch`, ресайз, дельты, реконнект; «endpoint generation 1» — стабильный контракт для Local/SSH/Cloud (CLAUDE.md форка). | `src/protocol/`, `tests/client_mode.rs`, `tests/multi_client.rs`, `tests/detach_reattach.rs` | M2 (шаг 0) |
| F8 | Серверный listener — `interprocess` local socket; наложение параллельных клиентских вызовов на Windows может дать `ERROR_PIPE_BUSY`. | `src/api/server.rs:7` | R3 (семафор нужен) |
| F9 | Есть `pane.wait_for_output` (long-poll с `match: substring|regex`, `timeout_ms`) и `events.wait`. Для «любое изменение» не подходит (нужен паттерн), но пригодится для M3 «jump to waiting». | `schema.rs:133`, `events.rs:95-112` | M3 |
| F10 | CLI умеет `herdr agent read <pane> --source recent --format ansi`; `strip_ansi` привязан к флагу `--raw` (`binding::boolean("raw", false)`) — для сырого ANSI передавать `--raw`. | `schema/panes.rs:396`, CLAUDE.md форка | R11 |

### 2.2 Параллельные задачи пользователя (Windows, ~1 час)

| Задача | Результат | DoD |
|---|---|---|
| U1 | `cd src-tauri && cargo generate-lockfile`; `rust-toolchain.toml` с текущей версией (`rustc --version`) | Оба файла в PR; `cargo check` проходит |
| U2 | 3–4 снапшота: `herdr agent read <pane> --source recent --format ansi --raw > src/__fixtures__/pane-read/<name>.ans` (проверить, что флаг действительно `--raw`; если нет — `herdr agent read --help`). Набор: user-turn + ответ `●`; tool call + `⎿`; dim-сводка; промпт с набранным текстом | Файлы в PR, вычитаны на секреты/пути |
| U3 | `where cswap` — `.exe` или `.cmd`-шим | Строка в `docs/architecture.md` |

---

## 3. M1a — стабильность

Критерий закрытия: окно не замирает при зависшем/отсутствующем Herdr; ошибки старта и действий видны в UI; `npm run build` (с typecheck) и `npm test` зелёные локально; отладочных лесов нет; CSP включён.

Оценка: S — до полудня, M — день. Ручная проверка у пользователя — отдельная очередь (≈0.5 дня суммарно на M1a).

| ID | Поток | Что | Файлы / контракты | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|---|---|
| **R1** | — | Удалить отладочные леса: команду `debug_log`, `chrono_free_now`, регистрацию в `generate_handler!`, `api.debug`, три вызова в `TerminalView`. `EnvFilter` по умолчанию `info`. | `src-tauri/src/lib.rs`, `src/api.ts`, `src/components/TerminalView.tsx` | 1 | — | debug-healer | `rg "debug_log|api\.debug|chrono_free_now" src src-tauri` пусто; `npx tsc --noEmit` зелёный; после минуты `npm run tauri dev` файл `../deck-debug.log` не создаётся. | S |
| **R2a** | — (ворота) | Минимальный контур проверки: закоммитить `Cargo.lock` + `rust-toolchain.toml` (из U1); `package.json`: `"build": "tsc --noEmit && vite build"`, `"typecheck"`, `"test": "vitest run"`; `vitest` в devDependencies, `test.environment: "node"`, `include: src/**/*.test.ts`; smoke-тест `src/dialogue.test.ts` (`stripAnsi`); заготовка стенда `scripts/fake-herdr.mjs` (Node `net.createServer` на `\\.\pipe\…`/unix socket; режимы `silent`, `slow <ms>`, `ack-error`, `close-after-ack`; подключение через `HERDR_SOCKET_PATH`). | `src-tauri/Cargo.lock`, `rust-toolchain.toml`, `package.json`, `package-lock.json`, `vite.config.ts`, `src/dialogue.test.ts`, `scripts/fake-herdr.mjs` | 5 (lock/toolchain), часть 19 (typecheck), инфраструктура 20 | R1, U1 | debug-healer | `npm ci && npm run build && npm test` зелёные локально. `node scripts/fake-herdr.mjs silent` слушает, Deck с `HERDR_SOCKET_PATH` к нему подключается. **[факт]** `tsc` на текущем коде, скорее всего, уже зелёный (`as any`/`!` под `strict` компилируются) — ворота дешёвые. | S |
| **R3** | Rust | Снять I/O с главного потока: все `herdr_*` → `#[tauri::command(async)]`; общий дедлайн (константа ~15 с, unix и windows): `call` в потоке + `mpsc::recv_timeout` (у `std::fs::File` нет read timeout на Windows); зависший поток отпускается до закрытия пайпа — документировать; семафор на 1–2 одновременных вызова (F8). Полноценный async-клиент — после ADR M2 (транспорт может оказаться не JSON-пайпом). | `src-tauri/src/lib.rs`, `src-tauri/src/herdr.rs` | 2 | R2a | debug-healer (rust-engineer) | С `fake-herdr.mjs silent`: окно перетаскивается, кнопки титулбара работают, `herdr_tree` возвращает ошибку не позже дедлайна, баннер OFFLINE появляется. С живым Herdr поведение не изменилось. `cargo check` зелёный (clippy — предупреждения, см. R2b). | M |
| **R4** | Rust | Честный `subscribe`: первая строка через `parse_reply("events.subscribe", …)` → ошибка как `Err(Api)`; при `Ok(())` pump эмитит `herdr://offline` («event stream closed; reconnecting»); разбор строк вынести в чистую `classify_line(&str) -> Ack | Event | Error | Skip`; тесты `parse_reply` и `classify_line`. | `src-tauri/src/herdr.rs`, `src-tauri/src/lib.rs::start_event_pump` | 9, Rust-часть 20 | R3 (тот же `herdr.rs`, делать после) | tdd-test-engineer | `cargo test`: `parse_reply` (result / error с message / без message / без result / мусор / пустая) и `classify_line` ({id,result} → Ack, {id,error} → Error, {event,…} → Event). Вручную: `fake-herdr.mjs ack-error` → баннер с текстом ошибки; `close-after-ack` → баннер «reconnecting». | S |
| **R6** | UI | Ошибки видны: fetcher `tree` ловит ошибку → `setOffline(String(e))`, **успешный `refetchTree` снимает `offline`**; fetcher `accounts` → сигнал `accountsError`, fallback «cswap недоступен: …» в блоке Accounts; `onSwitchAccount`: флаг `switching`, `.catch` → ошибка в карточке, `finally` → `refetchAccounts`; `<ErrorBoundary>` вокруг `<App/>`. | `src/App.tsx`, `src/components/Sidebar.tsx`, `src/index.tsx` | 3, 17, optional «снимать OFFLINE при успешном refetch» | R2a | ui-builder | Запуск без Herdr: UI отрисован целиком, баннер OFFLINE с текстом; после старта Herdr не позже 15 с дерево появляется **и баннер исчезает**. `cswap` переименован: блок Accounts показывает причину, остальное живо. Два быстрых двойных клика по разным аккаунтам → один `cswap switch` (счётчик вызовов в `api.ts` под `import.meta.env.DEV`). | S |
| **R8** | UI | Жизненный цикл `TerminalView`: гонка (`id = p.paneId`, `seq = ++reqSeq`, после `await` выход при устаревании; `lastText` после успешного `render`), `onCleanup` чистит `debounce` и ставит `disposed`, проверяемый в `refresh`/`render`/`schedule`; `Clock` в `TabBar` — `onCleanup(clearInterval)`. Хелпер `latestWins()` в `src/latest.ts`. | `src/components/TerminalView.tsx`, `src/components/TabBar.tsx`, `src/latest.ts` | 4, 13, 14 | R2a | tdd-test-engineer | `npm test`: ответ со старым seq/id отбрасывается. Вручную: `fake-herdr.mjs slow 2000` — переключение панелей не показывает чужой текст; закрытие всех панелей в Herdr — без ошибок в консоли webview. | S |
| **R10** | UI | Типы без дыр: `spaces()` всегда `{ top: Workspace[]; groups: Group[] }`, убрать `as any`; `p.accounts!` → keyed `<Show>`. | `src/components/Sidebar.tsx` | 16 | R2a | ui-builder | `npm run typecheck` зелёный; `rg "as any" src/components/Sidebar.tsx` пусто; сайдбар визуально без изменений. | S |
| **R15** | Rust | CSP вместо `null`: `default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'` (`'unsafe-inline'` нужен xterm.js); `connect-src ipc:`/`http://ipc.localhost` Tauri добавляет сам — проверить. | `src-tauri/tauri.conf.json` | 6 | R2a | debug-healer | `npm run tauri dev`: HMR работает, в консоли нет CSP-ошибок; релизный exe рисует терминал, `dlg`-декорации, шрифты; `rg '"csp": null' src-tauri` пусто. | S |

Сумма M1a: 7×S + 1×M ≈ **4.5 дня разработки + ≈0.5 дня ручных проверок у пользователя** → реалистично **одна рабочая неделя** (с учётом циклов «правка → проверка вечером»).

---

## 4. M1b — качество pane view, CI, документация

Стартует после закрытия M1a. Критерий закрытия: все 21 находка аудита закрыты или явно перенесены (R17).

| ID | Поток | Что | Файлы / контракты | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|---|---|
| **R2b** | Rust | CI: `permissions: contents: read`; `on: [push main, pull_request]`; `Swatinem/rust-cache@v2`; шаги `npm run typecheck`, `npm test`, `cargo fmt --check`, `cargo test`, `cargo clippy` **как предупреждения** (`-W`, без `-D`), затем `npm run tauri build` + artifact. Перевод clippy в `-D warnings` — в R17. Ubuntu-джоба (`cargo check` unix-веток) — только если влезет; требует webkit-зависимостей **[предположение]**. | `.github/workflows/build.yml` | 19, остаток 5 | R2a | debug-healer (devops-engineer) | Workflow зелёный на PR; в логах видны typecheck/test/fmt/clippy; артефакт `*.exe` загружен; повторный прогон без изменений не меняет `Cargo.lock`. | S |
| **R5** | Rust | Ошибки не глотать: `herdr_tree` — `tracing::warn!(workspace_id)` при падении `tab.list` + поле `partial: true` (тип `Tree.partial?`); `herdr_focus` — `_ => Err(Shape("unknown focus target"))`; `run_json` — `status.success()`, stderr в ошибку, JSON с первой строки, начинающейся на `{`; путь к `cswap` резолвить один раз при старте (по U3: `.exe`/`.cmd`), хранить в `tauri::State`. | `src-tauri/src/lib.rs`, `src/api.ts` | 15, 18, optional `herdr_focus` | R4 | debug-healer | `cargo test`: разбор stdout `cswap` (предупреждение перед JSON; ненулевой статус со stderr); `herdr_focus`-диспетчер вынесен в чистую функцию и покрыт тестом на неизвестный target (TS-тип `focus()` не даёт вызвать `"foo"`). Вручную: `cswap` переименован → в UI имя программы и stderr, а не «bad json». | S |
| **R7** | UI | Маршрутизация событий по реальной схеме (F1–F3): тип `HerdrEvent = { event: "pane_updated" | "workspace_created" | … }` в `api.ts`; чистая `routeEvent(ev) -> "tree" | "ignore"`; все структурные события, включая `pane_updated`, → `refetchTree` через trailing-debounce 200 мс; **`tick` от событий убрать** — события вывода нет, пока форк не эмитит `pane.output_changed` (M1.5); поллинг 2.5 с остаётся единственным триггером снапшота. | `src/App.tsx`, `src/api.ts`, `src/events.ts`, `src/events.test.ts` | 7 (исправленный диагноз) | R6 | ui-builder | `npm test`: `routeEvent` на фикстурных событиях из F1/F2 (`pane_updated` с `pane:{…}` → tree; неизвестное → ignore). Вручную: счётчик вызовов `herdr_tree` в `api.ts` (DEV) — всплеск из 10 событий → один вызов; **не** через DevTools Network (Tauri `invoke` там не виден). | S |
| **R9** | UI | Таблица клавиш `src/keys.ts` (`mapInput(data) -> {keys: string[]} | {text: string} | null`): явные имена для 0 и 27–31 (`ctrl+@`, `ctrl+[`, `ctrl+\`, `ctrl+]`, `ctrl+^`, `ctrl+_`), формула для 1–26; `\x1b[Z` → `shift+tab`; Ctrl/Alt/Shift+стрелки (`\x1b[1;5A` и т.п.) и F1–F12 → имена `parse_key_combo` (F6); **нераспознанные escape-последовательности не отправляются текстом** (F5: ушли бы как вставка) — `null` + `console.debug`. | `src/keys.ts`, `src/keys.test.ts`, `src/components/TerminalView.tsx` | 12, следствие F5 | R8 | tdd-test-engineer | `npm test`: `\x1c`→`ctrl+\`, `\x01`→`ctrl+a`, `\x1b[Z`→`shift+tab`, `\x1b[1;5C`→`ctrl+right`, `\x1bOP`→`f1`, `abc`→text, `\x1b[99~`→null. Вручную: Shift+Tab в Claude Code переключает режим, а не вставляет `[Z`; Ctrl+\ в shell даёт `^\Quit`. | S |
| **R11** | UI | Фикстуры из U2 + синтетические: `src/__fixtures__/pane-read/*.ans`, рядом `*.expected.json` с **границами блоков** (`[{kind, from, to}]`) и `promptLine` — не полный `LineKind[]`. Кейс п.10 — синтетическая строка `\x1b[1m\x1b[38;5;240m8 files changed`. Кейс CJK — синтетический. Тесты `classify`, `promptLine`, `stripBackground`. | `src/__fixtures__/pane-read/`, `src/dialogue.test.ts` | TS-часть 20 | R2a, U2 | tdd-test-engineer | `npm test` зелёный на текущем `dialogue.ts`, кейсы п.10/п.11 помечены `test.fails` и зеленеют в R12/R13. | S |
| **R12** | UI | `leadStyle`: первый печатный символ искать, пропуская ANSI (`/^(?:\x1b\[[0-9;?]*[ -\/]*[@-~]|\s)*/`), `DIM`/`RED` по этому префиксу. | `src/dialogue.ts` | 10 | R11 | tdd-test-engineer | Кейс «dim с цифрой» зелёный, остальные фикстуры без изменений ожиданий. | S |
| **R13'** | UI | Урезано: `blockWidth(lines, cols, widthOf = codePoints)` с инжектируемой функцией ширины; в `TerminalView` передать `widthOf` на основе простой East-Asian-Width таблицы (wide → 2). `placeCursor` не трогать; полный переход на ширины из буфера xterm — в M2. | `src/dialogue.ts`, `src/components/TerminalView.tsx` | 11 (частично) | R11 | tdd-test-engineer | `npm test`: `blockWidth` с CJK=2. Остаток п.11 (курсор) перенесён в M2 — зафиксировать в R17. | S |
| **R14'** | UI | Урезано до часа: в `refresh` не вызывать `render`, пока `term.hasSelection()` или viewport не внизу (`viewportY < baseY`); без индикатора и отложенного dirty-рендера. Полное решение — M2. | `src/components/TerminalView.tsx` | 8 (частично) | R8 | ui-builder | Вручную: выделение и прокрутка вверх переживают поллинг; прокрутка вниз — следующий снапшот применяется. | S (1 ч) |
| **R16** | — | Документация по факту: `docs/architecture.md` (день 0 + поток данных, event pump, ограничения снапшот-режима, F1–F10 как факты); README — `cswap` (как установлен, U3), `HERDR_SOCKET_PATH`/`HERDR_CONFIG_DIR`, `npm test`/`cargo test`, уточнить «Nothing is duplicated or stored»; `KeyBar` — убрать F1/F2/CTRL+K/ESC до M3. | `docs/architecture.md`, `README.md`, `src/components/KeyBar.tsx` | 21 | R5, R7, R9, R15 | architecture-documenter | Каждое утверждение README проверяется grep'ом по коду; в `architecture.md` нет пометок «[предположение]» рядом с форматом событий/`pane.read`/`send_text`. | S |
| **R17** | — | Повторное ревью по 21 находке: «закрыта (коммит)» / «перенесена (куда, почему)» — ожидаемые переносы: п.8 и п.11 частично → M2, п.19 unix-джоба. Перевести clippy в `-D warnings` (починить предупреждения). Проверить: ни одна команда не синхронная; нет `unwrap` на пути запросов; lifetimes в `TerminalView`. | `docs/audit/2026-10-24-tech-debt.md` (раздел «Статус»), `.github/workflows/build.yml` | все | R2b … R16 | code-quality-reviewer | Таблица 21×статус; CI зелёный с `clippy -D warnings`; нет открытых P0/P1. | S |

Сумма M1b: 10×S (R14' — час) ≈ **4.5–5 дней разработки + ≈0.5 дня ручных проверок** → вторая рабочая неделя. Итого M1 ≈ **2 календарные недели** (v1 обещала 5 дней при 11.5 человеко-днях работы — критика, вызов 2).

### 4.1 Граф зависимостей

```
День 0 (F1–F10, U1–U3) → R1 → R2a ──┬─ Rust: R3 → R4 → [M1a] → R2b → R5
                                     └─ UI:   R6, R8, R10 ∥ R15(Rust-конфиг) → [M1a] → R7, R9, R11 → R12, R13' ; R8 → R14'
M1b хвост: R16 (после R5, R7, R9, R15) → R17
```

Пересечения по файлам: `herdr.rs` (R3→R4 последовательно), `TerminalView.tsx` (R8 → R9 → R13' → R14' внутри UI-потока, по одному за раз), `api.ts` (R5/R7 — мелкие правки типов).

---

## 5. Стратегия тестирования M1

| Слой | Инструмент | Что покрываем | Фикстуры / стенд |
|---|---|---|---|
| TS, чистая логика | vitest, env `node` | `dialogue.ts` (`classify`, `promptLine`, `stripAnsi`, `stripBackground`, `blockWidth` с `widthOf`), `keys.ts`, `events.ts`, `latest.ts` | `src/__fixtures__/pane-read/*.ans` — сырой ANSI из `herdr agent read … --format ansi --raw` (F10) + синтетические строки для п.10/CJK; `*.expected.json` — границы блоков и `promptLine`. Вычитка на секреты до коммита. |
| Rust, чистая логика | `#[cfg(test)]`, `cargo test` (локально у пользователя и в CI с R2b) | `parse_reply`, `classify_line`, разбор stdout `cswap`, диспетчер `herdr_focus`, `socket_path`/`config_dir` (приоритет env) | Функции над `&str`, без сокетов. |
| Интеграция мост↔сервер | Ручной стенд | Дедлайн/отсутствие заморозки (R3), ошибочный ack и закрытие потока (R4), гонка панелей (R8), реконнект после перезапуска Herdr (R6) | `scripts/fake-herdr.mjs` (режимы `silent`, `slow <ms>`, `ack-error`, `close-after-ack`), `HERDR_SOCKET_PATH`. Dev-скрипт, в бандл не попадает. |
| UI / xterm | Ручная проверка по чек-листам DoD | CSP + HMR + релизный exe (R15), ошибки старта (R6), клавиши Shift+Tab/Ctrl+\ (R9), выделение/прокрутка (R14'), рамки с CJK (R13') | Живой Herdr с Claude Code. xterm в jsdom не тестируем. |
| Наблюдаемость | счётчики в `api.ts` под `import.meta.env.DEV`; `tracing` в Rust | Частота `herdr_tree`/`pane.read`/`cswap switch` (R6, R7) | Tauri `invoke` не виден в DevTools Network — DoD на него не опираются. |

Правило: правка `dialogue.ts` начинается с фикстуры (R11 предшествует R12/R13').

---

## 6. Бэклог после M1

Порядок: M1.5 → M2 → M3 → M4 → M5.

### M1.5 — форк: `pane.output_changed` и настоящий `revision`

**Цель.** Дать Deck честный триггер вместо поллинга 2.5 с: эмитить существующий `EventKind::PaneOutputChanged` (`pane_id`, `workspace_id`, `revision`) коалесцированно (≤ N/с на панель) и возвращать реальный `revision` в `pane.read` вместо `0` (F3, F4). Схема уже есть — изменений контракта нет, только эмиссия; диff в форке малый, стоимость rebase низкая.

**Зависимости.** M1a (R3/R4 — pump готов принимать); не блокирует M1b. В Deck — вернуть `tick` в `routeEvent` (R7) для `pane_output_changed` с совпадающим `pane_id`, сравнивать `revision` вместо текста.

**DoD.** В форке: тест на эмиссию и коалесцирование по правилу CLAUDE.md форка («multiplicative paths»: один scalar per event, без формата снапшота внутри горячего цикла); в Deck: при молчащей выбранной панели `pane.read` не вызывается чаще поллинга-fallback (который можно поднять до 10 с).

### M2 — потоковое подключение к панели

**Цель.** Убрать снапшоты: Deck получает начальное состояние + дельты, xterm ведёт реальный курсор, нет `term.reset()` и мерцания; прокрутка/выделение и ширины в ячейках решаются естественно (остатки п.8/п.11).

**Шаг 0 — ADR `docs/adr/0001-pane-streaming.md` (architecture-documenter, до кода).** Сравнить:
- (A) Deck как клиент **endpoint generation 1** (F7: `PaneSurface`/`PaneSurfacePatch`, bincode-кодек, уже с ресайзом, дельтами, реконнектом, multi-client);
- (B) новый JSON-метод `pane.stream` в форке (вариант v1).

Критерии: объём изменений в форке (A ≈ 0 на сервере; B — новый метод + тесты), merge-cost с upstream (B — вечный rebase; A — контракт, который upstream обязуется держать), поддержка Windows named pipe у endpoint-транспорта (проверить `src/protocol/endpoint.rs` и `HERDR_CLIENT_SOCKET_PATH`), стоимость bincode-клиента на Rust-стороне Deck (кодек можно взять как crate-зависимость из форка/upstream или вынести), модель размеров (чей размер у pty: TUI, Deck, последний подключившийся — у endpoint это уже определено), требование «`herdr agent attach` не работает на Windows» — относится ли оно к endpoint-клиенту.

**Открытые вопросы (остаток после дня 0).**
1. ~~Формат `pane.read`~~ — закрыт (F4: отрендеренные строки).
2. Схема событий — закрыт: `event` в snake_case (F1); `pane_updated` несёт `pane: PaneInfo`, на label/title приходит (F2).
3. ~~Семантика `send_text`~~ — закрыт (F5: bracketed paste).
4. Named pipe на Windows: число экземпляров у `interprocess`-listener, воспроизводится ли `ERROR_PIPE_BUSY` при 2+ параллельных вызовах (замерить с семафором R3 = 2).
5. Форма `cswap list --json` — типизирована по памяти; сверить с `cswap` (вне форка).
6. Для варианта (A): версия/генерация кодека в форке vs upstream, как Deck узнаёт, что сервер — форк с нужными методами (advertised API methods).

**Зависимости.** M1a + R7/R9/R11 из M1b (фикстуры нужны, чтобы `dialogue.ts` пережил смену источника); M1.5 желателен, но не обязателен для (A).

**DoD.** ADR принят; `TerminalView` без поллинга и без `term.reset()`; курсор от pty; `dialogue.ts` работает на буфере xterm (закрывает остатки п.8/п.11); фикстуры R11 зелёные; постоянное соединение с таймаутами и реконнектом (async-клиент под выбранный транспорт — сюда переезжает «полноценный tokio-клиент» из R3); спиннеры Claude Code не размножаются.

### M3 — действия в дереве и хоткеи

**Цель.** Контекстные меню (аккаунт: switch/refresh; workspace: focus/close/new tab), создание workspace/tab, клавиши из `KeyBar`: F1 пикер аккаунта, F2 переход к ожидающему агенту (`agent_status === "blocked"`; можно опереться на `events.wait`/`pane.agent_status_changed`, F9), Ctrl+K палитра, Esc.

**Зависимости.** M1 (R6 — ошибки действий; R16 — честный KeyBar; R9 — `mapInput`, чтобы глобальные клавиши не уходили в pty: `attachCustomKeyEventHandler`). Проверить в форке имена методов `workspace.create/close`, `tab.create/close` (`src/api/schema.rs`). UX-спека: ux-designer → ui-builder → ux-parity-reviewer.

**DoD.** Каждое действие вызывает метод Herdr и отражается в TUI Herdr; ошибки видны; хоткеи работают при фокусе в терминале и вне его; `KeyBar` отражает реальные клавиши.

### M4 — полировка

**Цель.** Rajdhani в бандле (`font-src 'self'` после R15); уведомление/звук при `blocked` (`tauri-plugin-notification` + capability); опциональный split view (два потока — после M2); сохранение размера окна (`tauri-plugin-window-state`) и выбранной панели (только UI-настройки); правила рамок для Codex — через фикстуры `codex-*.ans`.

**Зависимости.** M2 (split view, Codex-рендер на буфере), R11 (фикстуры), F2 (`pane_updated`/`pane.agent_status_changed` для уведомлений).

**DoD.** Чек-лист на фичу; Codex-фикстуры в `npm test`; README обновлён.

### M5 — релиз

**Цель.** Релиз по тегу (`*.exe` + GitHub Release), версия в одном месте (`"version": "../package.json"` в `tauri.conf.json`, `Cargo.toml` синхронизируется скриптом), канал обновлений (`tauri-plugin-updater`, ключ подписи в секретах, `latest.json`).

**Зависимости.** R2a/R2b; `contents: write` только в release-джобе по тегу.

**DoD.** `git tag v0.2.0 && git push --tags` → Release с `*.exe` и `latest.json`; предыдущая версия предлагает обновление; `rg '0\.1\.0'` — одно место.

---

## 7. Риски и открытые вопросы

| # | Риск / вопрос | Тип | Влияние | Митигация |
|---|---|---|---|---|
| 1 | Реализатор не может собрать Tauri и не имеет crates.io; все Rust-правки и ручные проверки — через пользователя или холодный Windows-раннер (10–20 мин) | факт | R3, R4, R5, R15; сроки | Два потока вместо четырёх; стенд `fake-herdr.mjs` в R2a; пакетировать правки Rust-потока под одну проверку вечером |
| 2 | `ERROR_PIPE_BUSY` при параллельных вызовах после перехода на async (F8) | предположение | R3 | Семафор 1–2; замер в M2 вопрос 4 |
| 3 | Дедлайн через поток + `recv_timeout` оставляет заблокированный поток до закрытия пайпа | факт | R3 | Документировать; ограничить число брошенных потоков; настоящий async-клиент — после ADR M2 |
| 4 | Без события вывода (F3) pane view остаётся на поллинге весь M1; задержка до 2.5 с и мерцание не уходят до M1.5/M2 | факт | ожидания от M1 | Явно сказано в критерии закрытия M1a; M1.5 — малый шаг в форке |
| 5 | Unix-ветки `herdr.rs` не компилируются в CI | факт | R2b | Ubuntu `cargo check` опционально; иначе — риск принят до M2 |
| 6 | Фикстуры могут содержать содержимое чужих сессий | факт | U2/R11 | Снимать с тестовой сессии, вычитывать перед коммитом |
| 7 | Имя флага сырого вывода CLI (`--raw`) выведено из `binding::boolean("raw")`, не проверено запуском | предположение | U2 | `herdr agent read --help` |
| 8 | Вариант (A) M2 может оказаться недоступен на Windows или требовать bincode-кодек, который нельзя переиспользовать как crate | предположение | M2 | Шаг 0 ADR проверяет это первым; fallback — (B) |
| 9 | `tsc --noEmit` может иметь ошибки, не видимые без запуска | предположение (маловероятно) | R2a | Чинить в R2a/R10 |
| 10 | Двойной клик меняет глобальные креды CLI без подтверждения | факт (optional аудита) | UX | Решить в M3 с F1-пикером |

---

## 8. Порядок выполнения — ближайшие две недели

**Неделя 1 — M1a**
1. День 0: пользователь — U1 (`Cargo.lock`, `rust-toolchain.toml`), U2 (3–4 фикстуры через `herdr agent read … --format ansi --raw`), U3 (`where cswap`). Параллельно реализатор — F1–F10 в `docs/architecture.md`, R1.
2. День 1: R2a (typecheck в build, vitest + smoke, `fake-herdr.mjs`) — зелёные `npm run build && npm test`. Это единственные ворота.
3. Дни 2–4: поток Rust — R3 → R4 (одна ручная проверка на `fake-herdr.mjs silent/ack-error` + живой Herdr); поток UI — R6, R8, R10, затем R15 (проверка HMR + релизный exe).
4. День 5: проверка критерия M1a («не виснет, ошибки видны, typecheck+vitest зелёные»). Если R3 вернулся с проверки второй раз — не начинать M1b, разбираться с R3.

**Неделя 2 — M1b**
5. R2b (CI с Rust-шагами, clippy как предупреждения) и R5 — Rust-поток; R7, R9, R11 → R12, R13', R14' — UI-поток.
6. R16 документация по итоговому состоянию, затем R17 (21×статус, clippy `-D`).
7. После R17: открыть M1.5 в форке (эмиссия `pane.output_changed`) и шаг 0 ADR M2 — до любого кода стриминга.

Ранние сигналы (из критики): нет зелёного `npm run typecheck && npm test` к концу дня 1; R3 на второй итерации проверки; `src/__fixtures__/pane-read/` пуст к старту R12; в `architecture.md` всё ещё «[предположение]» у формата событий; ADR 0001 без упоминания `PaneSurfacePatch`.

---

## Changelog v2

Нумерация — вызовы из `docs/plan/2026-10-24-roadmap-critique.md`.

- (1) Добавлен раздел «День 0»: факты F1–F10 из исходников форка с файлами-источниками, задачи пользователя U1–U3; сверка перенесена до R2a, а не «параллельно на дне 1–2».
- (1) R7 переписан под реальную схему: `event` в snake_case, `pane_updated` = метаданные → debounced `refetchTree`; `tick` от событий убран до появления события вывода; DoD через счётчик в `api.ts`, не DevTools Network (7).
- (1) R9 расширен: `\x1b[Z` → `shift+tab`, модификаторы+стрелки и F-клавиши → имена `parse_key_combo`; нераспознанные escape-последовательности не отправляются текстом (bracketed paste).
- (1, 5) В бэклог добавлен M1.5: форк эмитит `pane.output_changed` коалесцированно + реальный `revision`.
- (2) M1 разрезан на M1a (стабильность: R1, R2a, R3, R4, R6, R8, R10, R15) и M1b (остальное); параллельность — два потока; оценка пересчитана: ≈4.5+0.5 дня на M1a, ≈4.5–5+0.5 на M1b, итого две недели вместо пяти дней.
- (3) R14 урезан до часового предохранителя (не рендерить при выделении/прокрутке вверх); R13 урезан до `blockWidth(lines, cols, widthOf?)` + CJK-тест; остатки п.8/п.11 перенесены в M2.
- (4) R2 разделён на R2a (ворота: lock, toolchain, `tsc` в build, vitest + smoke, `fake-herdr.mjs`) и R2b (CI: cargo test, clippy `-W`, rust-cache, permissions read, PR-триггер; не ворота); clippy `-D` перенесён в R17.
- (5) R11 опирается на `herdr agent read … --format ansi --raw` вместо `scripts/capture-pane.mjs`; ожидания — границы блоков, не полный `LineKind[]`; кейс п.10 и CJK — синтетические фикстуры.
- (6) M2: добавлен шаг 0 ADR «Deck как клиент endpoint generation 1 (`PaneSurface`) vs новый JSON-метод» с критериями; вопросы 1 и 3 закрыты, вопрос 2 отвечен; добавлен вопрос о кодеке/генерации для варианта (A).
- (7) DoD исправлены: R5 — диспетчер `herdr_focus` тестируется в `cargo test`, не через нетипизируемый `api.focus("foo")`; R6 — «успешный refetch снимает OFFLINE» закреплён в R6 (R7 отложен в M1b).
- Добавлены F8 (`interprocess` listener) и F9 (`pane.wait_for_output`/`events.wait`) как факты, найденные при сверке; риски переписаны под новые факты (стенд, отсутствие события вывода, флаг `--raw`, доступность endpoint на Windows).
