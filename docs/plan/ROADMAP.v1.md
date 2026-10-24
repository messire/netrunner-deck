<!-- feature-planner, 2026-10-24. Источники: docs/audit/2026-10-24-tech-debt.md (п.1–21), код src/, src-tauri/src/, .github/workflows/build.yml, package.json, tauri.conf.json. Сборка/тесты при планировании не запускались. -->

# Netrunner Deck — Roadmap

## 1. Цель и принципы

- **Тонкий клиент.** Deck ничего не хранит и не дублирует: workspaces, tabs, panes, агенты, вывод терминала — всё живёт в Herdr. Локально допустимы только настройки UI (размер окна, выбранная панель) — и то начиная с M4.
- **Herdr — источник правды.** Если Deck чего-то не может показать, недостающее добавляется в JSON API форка (`messire/herdr-punk`), а не обходится на стороне клиента.
- **Никаких фич до закрытия M1.** Аудит нашёл отладочные леса в релизе (п.1), синхронный I/O на главном потоке (п.2), необработанные ошибки старта (п.3) и невоспроизводимую сборку (п.5). Пока нет `npm run build` с typecheck, `npm test` и `cargo test`, любую фичу невозможно проверить — поэтому M1 начинается с них.
- **Факты отдельно от предположений.** Всё, что касается семантики API Herdr (формат `pane.read`, поля событий, `send_text`), в коде сейчас сделано «по памяти автора» (аудит, раздел «Непроверенные риски»). Такие места помечены ниже как **[предположение]** и проверяются по исходникам форка до того, как на них что-то строить.

Исполнители — агенты проекта (`.claude/agents/`): `code-quality-reviewer`, `tdd-test-engineer`, `debug-healer`, `ui-builder`, `architecture-documenter`. Отдельного агента под Rust/CI нет: Rust-мост и инфраструктуру ведёт `debug-healer` (сборка/зависания/runtime) и `tdd-test-engineer` (логика с тестами); при необходимости они подключают skills `rust-engineer`, `devops-engineer`.

---

## 2. M1 — Рефакторинг

Покрытие аудита: **все P0 (1) и P1 (2–7)**, P2 с Effort 1 (9, 10, 12–19, 21) и P2, влияющие на корректность pane view (8, 11, 20). Итого все 21 находка.

Оценка: S — до полудня, M — день, L — больше дня.

### 2.1 Фаза 0 — фундамент (строго первой, последовательно)

| ID | Что | Файлы / контракты | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|---|
| **R1** | Удалить отладочные леса: команду `debug_log`, `chrono_free_now`, регистрацию в `generate_handler!`, `api.debug`, три вызова в `TerminalView`. Диагностику, если нужна, — `tracing::debug!` без текста панели. Заодно `EnvFilter` по умолчанию `info` (optional из аудита), чтобы `warn!` в event pump был виден. | `src-tauri/src/lib.rs`, `src/api.ts`, `src/components/TerminalView.tsx` | 1 | — | debug-healer | `rg "debug_log|api\.debug|chrono_free_now" src src-tauri` пусто; `cargo check` и `npx tsc --noEmit` зелёные; после `npm run tauri dev` и минуты работы файл `../deck-debug.log` не создаётся. | S |
| **R2** | Воспроизводимая сборка и контур проверки: (а) закоммитить `src-tauri/Cargo.lock`, добавить `rust-toolchain.toml` с конкретной версией; (б) `package.json`: `"build": "tsc --noEmit && vite build"`, `"typecheck"`, `"test": "vitest run"`; добавить `vitest` в devDependencies, конфиг через `vite.config.ts` (`test.include: src/**/*.test.ts`, окружение `node` — `dialogue.ts` без DOM); (в) `build.yml`: `permissions: contents: read`, `on: [push main, pull_request]`, `Swatinem/rust-cache@v2`, шаги `npm run typecheck`, `npm test`, `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` (`--manifest-path src-tauri/Cargo.toml`), затем `npm run tauri build` + upload artifact. (г) Один smoke-тест в `src/dialogue.test.ts` и один `#[cfg(test)]` в `herdr.rs`, чтобы раннеры реально что-то запускали. | `src-tauri/Cargo.lock`, `rust-toolchain.toml`, `package.json`, `package-lock.json`, `vite.config.ts`, `.github/workflows/build.yml`, `src/dialogue.test.ts`, `src-tauri/src/herdr.rs` | 5, 19, инфраструктурная часть 20 | R1 | debug-healer (skill devops-engineer) | Локально: `npm ci && npm run build && npm test && cargo test && cargo clippy -- -D warnings` зелёные. CI: workflow зелёный на PR; в логах видны шаги typecheck/test/clippy; артефакт `*.exe` загружен; повторный запуск CI без изменений резолвит те же версии crates (Cargo.lock не меняется). | M |

Примечания к R2:
- `Cargo.lock` нужно сгенерировать на машине с Rust-тулчейном (у пользователя — Windows msvc); из этого окружения сделать нельзя. **[факт]** В рабочей копии файла нет, `.gitignore` его не исключает.
- `npx tsc --noEmit` сейчас может быть красным (п.16: `as any`, `p.accounts!`) — такие ошибки чинятся в рамках R2 минимально или сразу переносятся в R10, но CI с typecheck должен стать зелёным до старта Фазы 1.
- Ubuntu-джоба для `cargo check` (аудит, п.19) требует системных зависимостей Tauri (webkit2gtk, gtk и т.д.) — **[предположение]** это полдня возни; в M1 достаточно Windows-раннера. Unix-ветки `herdr.rs` остаются непроверенными — записать в риски.

### 2.2 Фаза 1 — параллельные группы (после R2)

Группы A–D независимы друг от друга; внутри группы шаги идут по порядку. Каждая группа заканчивается прогоном `npm run build && npm test && cargo test && cargo clippy -- -D warnings`.

#### Группа A — Rust-мост (`src-tauri/`)

| ID | Что | Файлы / контракты | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|---|
| **R3** | Снять I/O с главного потока и ограничить ожидание. Все `herdr_*` → `#[tauri::command(async)]` (или `async fn` + `tauri::async_runtime::spawn_blocking`). Общий дедлайн вызова (константа, одна для unix/windows, например 15 с): на Windows `std::fs::File` не умеет read timeout, поэтому `call` выполняется в потоке, результат ждётся через `mpsc::recv_timeout`; зависший поток отпускается (освободится при закрытии пайпа — документировать). Параллельные вызовы сериализовать глобальным `Mutex`/семафором (1–2 одновременных), чтобы не ловить `ERROR_PIPE_BUSY` (см. §6). Полноценный async-клиент на `tokio::net::windows::named_pipe` — в M2 вместе с постоянным соединением. | `src-tauri/src/lib.rs` (все команды), `src-tauri/src/herdr.rs` (`connect`, `call`, новая обёртка с дедлайном) | 2 | R2 | debug-healer (skill rust-engineer) | (1) Фейковый сервер `scripts/fake-herdr.mjs` (Node `net.createServer().listen('\\\\.\\pipe\\…')`, принимает соединение и молчит) + `HERDR_SOCKET_PATH`: окно Deck перетаскивается, кнопки титулбара работают, `herdr_tree` возвращает ошибку не позже дедлайна, баннер OFFLINE появляется. (2) С живым Herdr поведение не изменилось (tree, pane.read, ввод). (3) `cargo clippy -D warnings` зелёный. | M |
| **R4** | Честный `subscribe`: первую строку разбирать через `parse_reply("events.subscribe", …)`, ошибку возвращать как `Err(Api)`; при `Ok(())` (сервер закрыл поток) pump тоже эмитит `herdr://offline` с текстом «event stream closed; reconnecting». Для тестируемости вынести разбор строк в чистую функцию (`classify_line(line) -> Ack | Event | Skip | Error`). Юнит-тесты `parse_reply` и разбора ack. | `src-tauri/src/herdr.rs`, `src-tauri/src/lib.rs::start_event_pump` | 9, Rust-часть 20 | R2 (можно параллельно с R3, конфликт только в `herdr.rs` — согласовать порядок мержа) | tdd-test-engineer | `cargo test`: кейсы `parse_reply` (result / error с message / error без message / без result / мусор / пустая строка) и ack (`{id,result}` → Ack, `{id,error}` → Error, `{event,…}` → Event). Вручную: `fake-herdr.mjs` отвечает `{"id":…,"error":{"message":"x"}}` на subscribe → баннер OFFLINE с этим текстом, в логе `warn` каждые 2 с. | S |
| **R5** | Ошибки не глотать: `herdr_tree` — `tracing::warn!(workspace_id)` при падении `tab.list` и поле `partial: true` в ответе (тип `Tree.partial?: boolean` в `api.ts`); `herdr_focus` — `_ => Err(Shape("unknown focus target"))`; `run_json` — проверять `output.status.success()`, включать stderr в ошибку, JSON искать с первой строки, начинающейся на `{`; путь к `cswap` резолвить один раз при старте (PATH-поиск через `which`-подобную проверку, на Windows учитывать `.exe`/`.cmd`, см. §6) и хранить абсолютный путь в `tauri::State`. | `src-tauri/src/lib.rs`, `src/api.ts` (тип `Tree`) | 15, 18, optional `herdr_focus` | R2 | debug-healer | `cargo test` на разбор вывода `run_json` (вынести в чистую функцию: stdout с предупреждением перед JSON, ненулевой статус со stderr). Вручную: переименовать `cswap` → в UI текст ошибки с именем программы и stderr, а не «bad json»; `api.focus("foo", id)` отклоняется. | S |

#### Группа B — состояние приложения (`src/App.tsx`, `Sidebar`, `TabBar`, ввод)

| ID | Что | Файлы / контракты | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|---|
| **R6** | Ошибки ресурсов и действий видны пользователю. Fetcher'ы `tree`/`accounts` ловят ошибки: `tree` → `setOffline(String(e))`, `accounts` → отдельный сигнал `accountsError` (fallback «cswap недоступен: …» в блоке Accounts). `onSwitchAccount`: флаг `switching` (блокирует повторный клик), `.catch` → ошибка в карточке аккаунта, `finally` → `refetchAccounts`. `<ErrorBoundary>` вокруг `<App/>` в `index.tsx` как последняя линия. | `src/App.tsx`, `src/components/Sidebar.tsx`, `src/index.tsx` | 3, 17 | R2 | ui-builder | Запуск без Herdr: интерфейс отрисован целиком, виден баннер OFFLINE с текстом ошибки, через 15 с после старта Herdr дерево появляется. `cswap` отсутствует в PATH: блок Accounts показывает причину, остальной UI жив. Два быстрых двойных клика по разным аккаунтам → один `cswap switch`. | S |
| **R7** | Экономный event pump на стороне UI: `pane.updated` поднимает `tick` только если событие относится к `currentPane()`; остальные события → `refetchTree` через trailing-debounce 200 мс; успешный `refetchTree` снимает `offline` (optional из аудита). **[предположение]** имя поля с id панели и форма события (`ev.event` vs `ev.type`) — сверить с исходниками форка (`events.subscribe`) до реализации и зафиксировать в комментарии к типу `HerdrEvent` в `api.ts`. | `src/App.tsx`, `src/api.ts` (тип `HerdrEvent`) | 7 | R2; факт по схеме событий из форка | ui-builder | Юнит-тест чистой функции `routeEvent(ev, currentPane) -> "tick" | "tree" | "ignore"` в `src/events.test.ts`. Вручную: два активных агента, выбрана молчащая панель — в DevTools Network/console нет `pane.read` чаще раза в 2.5 с; всплеск из 10 событий → один `herdr_tree`. | S |
| **R8** | Жизненный цикл `TerminalView`: защита от гонки (в `refresh` фиксировать `id = p.paneId`, `seq = ++reqSeq`, после `await` выходить, если устарело; `lastText` выставлять после успешного `render`), `onCleanup` чистит `debounce` и ставит `disposed`, проверяемый в начале `refresh`/`render`/`schedule`; `Clock` в `TabBar` — `onCleanup(clearInterval)`. Логику «последний запрос побеждает» вынести в чистый хелпер `latestWins()` для теста. | `src/components/TerminalView.tsx`, `src/components/TabBar.tsx`, `src/latest.ts` (новый) | 4, 13, 14 | R2 | tdd-test-engineer | `npm test`: тест хелпера (ответ со старым seq/id отбрасывается). Вручную с `fake-herdr.mjs`, отвечающим на `pane.read` с задержкой 2 с: переключение панелей во время запроса не показывает чужой текст; закрытие всех панелей в Herdr не даёт ошибок в консоли webview. | S |
| **R9** | Таблица управляющих клавиш: явные имена для 0 и 27–31 (`ctrl+@`, `ctrl+[`, `ctrl+\`, `ctrl+]`, `ctrl+^`, `ctrl+_`), формула только для 1–26. Вынести в `src/keys.ts` (`mapInput(data) -> {keys} | {text}`). **[предположение]** имена клавиш, которые принимает `pane.send_keys`, — сверить со списком в форке. | `src/keys.ts` (новый), `src/components/TerminalView.tsx` | 12 | R2 | tdd-test-engineer | `npm test`: таблица кейсов (`\x1c` → `ctrl+\`, `\x01` → `ctrl+a`, `\r` → `enter`, `abc` → text). Вручную: Ctrl+\ в shell-панели даёт SIGQUIT (`^\Quit`). | S |
| **R10** | Типы без дыр: `spaces()` всегда `{ top: Workspace[]; groups: Group[] }`, убрать `as any`; `p.accounts!` → keyed `<Show when={p.accounts}>{(acc) => …}`; `tsc --noEmit` без ошибок остаётся обязательным в `npm run build`. | `src/components/Sidebar.tsx` | 16 | R2 | ui-builder | `npm run typecheck` зелёный; `rg "as any|!\." src/components/Sidebar.tsx` пусто; сайдбар визуально без изменений. | S |

#### Группа C — pane view и `dialogue.ts` (test-first)

| ID | Что | Файлы / контракты | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|---|
| **R11** | Фикстуры и тесты эвристик. Снять 6–8 реальных снапшотов `pane.read` (ANSI, как приходит в `read.text`) в `src/__fixtures__/pane-read/*.ans` + рядом `*.expected.json` с `LineKind[]` и индексом `promptLine`. Набор: user-turn `>`/`❯` + ответ `●`; tool call `● Read(…)` + `⎿`; dim-сводка, начинающаяся с цифры (`8 files changed`, кейс п.10); красная системная заметка; абзацы внутри ответа; строка с CJK/эмодзи (для R13); пустой промпт `❯`; промпт с набранным текстом. Тесты `classify`, `promptLine`, `stripAnsi`, `stripBackground`, `blockWidth`. Способ снятия — см. §3. | `src/__fixtures__/pane-read/`, `src/dialogue.test.ts`, `scripts/capture-pane.mjs` (опционально) | TS-часть 20 | R2; доступ к живому Herdr с Claude Code | tdd-test-engineer | `npm test` зелёный на текущем `dialogue.ts`, **кроме** кейсов п.10/п.11, которые помечены `test.fails`/`todo` и переходят в зелёные в R12/R13. Фикстуры не содержат секретов (проверить глазами перед коммитом). | M |
| **R12** | `leadStyle`: первый печатный символ искать, пропуская ANSI (`/^(?:\x1b\[[0-9;?]*[ -\/]*[@-~]|\s)*/`), `DIM`/`RED` проверять по этому префиксу. | `src/dialogue.ts` | 10 | R11 | tdd-test-engineer | Кейс «dim-строка с цифрой» из R11 зелёный; остальные фикстуры не изменили ожидания. | S |
| **R13** | Ширина в ячейках, а не code points: `blockWidth` и `typed` в `placeCursor` считать по буферу xterm (`buf.getLine(r)`, сумма `cell.getWidth()`), т.к. `rowsByLine()` уже даёт строки буфера; `dialogue.ts` остаётся без зависимости от xterm (принимает готовые ширины или функцию `widthOf`). | `src/dialogue.ts`, `src/components/TerminalView.tsx` | 11 | R11, R8 (один файл) | tdd-test-engineer + ui-builder | Юнит-тест `blockWidth` с инжектированной `widthOf` (CJK = 2). Вручную: ответ агента с эмодзи — рамка упирается ровно в край текста; курсор на промпте с эмодзи стоит после последнего символа. | M |
| **R14** | Не сбрасывать прокрутку/выделение: перед `term.reset()` снимать `atBottom = viewportY >= baseY` и `hasSelection()`; при `!atBottom || sel` ставить `dirty` и откладывать рендер до возврата вниз (`onScroll`) / снятия выделения (`onSelectionChange`); индикатор «NEW OUTPUT ↓» в заголовке панели, пока `dirty`. Решение снапшот-уровня, уйдёт вместе с M2. | `src/components/TerminalView.tsx`, `src/styles.css` | 8 | R8 | ui-builder | Вручную: прокрутить вверх в активной панели — в течение 10 с вывода viewport не прыгает, виден индикатор; прокрутка вниз — применяется последний снапшот; выделение текста переживает поллинг. Ресайз окна по-прежнему перерисовывает. | M |

#### Группа D — поверхность безопасности и документация

| ID | Что | Файлы / контракты | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|---|
| **R15** | CSP вместо `null`: `default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'` (`'unsafe-inline'` для style нужен xterm.js). При необходимости `connect-src ipc: http://ipc.localhost` — Tauri добавляет сам, проверить. | `src-tauri/tauri.conf.json` | 6 | R2 | debug-healer | `npm run tauri dev`: HMR работает, в консоли webview нет CSP-ошибок; `npm run tauri build` → релизный exe рисует терминал, декорации `dlg` и шрифты; `rg '"csp": null' src-tauri` пусто. | S |
| **R16** | Документация по факту: README — требования (`cswap` в PATH, как установлен), переменные `HERDR_SOCKET_PATH`/`HERDR_CONFIG_DIR`, раздел Dev (`npm test`, `cargo test`), убрать/уточнить фразу «Nothing is duplicated or stored» с оговоркой про UI-настройки M4; `KeyBar` — убрать подписи F1/F2/CTRL+K/ESC до M3 (или пометить как план, по решению ux-parity-reviewer); короткий `docs/architecture.md` (поток данных команда → пайп → Herdr, event pump, ограничения снапшот-режима) как база для ADR M2. | `README.md`, `src/components/KeyBar.tsx`, `docs/architecture.md` (новый) | 21 | R1, R2, R5, R15 (чтобы описывать итоговое состояние) | architecture-documenter | Каждое утверждение README проверяется grep'ом по коду (переменные, команды, скрипты `package.json`). `KeyBar` не рекламирует нереализованных клавиш. | S |

### 2.3 Фаза 2 — закрытие

| ID | Что | Закрывает | Зависит | Исполнитель | DoD | Оценка |
|---|---|---|---|---|---|---|
| **R17** | Повторное ревью по чек-листу аудита: для каждой из 21 находки — «закрыта (коммит)» или «перенесена (куда, почему)». Отдельно: ресурсные lifetimes в `TerminalView`, отсутствие `unwrap`/`expect` на пути запросов, что ни одна команда не осталась синхронной. | все | R1–R16 | code-quality-reviewer | Таблица 21×статус в `docs/audit/2026-10-24-tech-debt.md` (дописать раздел «Статус») или в PR; CI зелёный; нет открытых P0/P1. | S |

### 2.4 Граф зависимостей

```
R1 → R2 → { A: R3, R4, R5 } ∥ { B: R6, R7, R8 → R9, R10 } ∥ { C: R11 → R12, R13; R8 → R14 } ∥ { D: R15 } → R16 → R17
```

Параллелить можно четыре потока сразу после R2. Пересечения по файлам: `herdr.rs` (R3/R4), `TerminalView.tsx` (R8/R13/R14 — делать последовательно внутри одного потока), `api.ts` (R5/R7 — мелкие правки типов, мерж тривиален).

---

## 3. Стратегия тестирования M1

| Слой | Инструмент | Что покрываем | Фикстуры / стенд |
|---|---|---|---|
| TS, чистая логика | vitest, env `node` | `dialogue.ts` (`classify`, `promptLine`, `stripAnsi`, `stripBackground`, `blockWidth` с `widthOf`), `keys.ts`, `events.ts` (`routeEvent`), `latest.ts` | `src/__fixtures__/pane-read/*.ans` — сырые ответы `pane.read` (`format=ansi`, `strip_ansi=false`) с реальных сессий Claude Code; `*.expected.json` — ожидаемые `LineKind[]` + `promptLine`. Снимать через `scripts/capture-pane.mjs` (Node подключается к пайпу/сокету Herdr, шлёт `pane.read`, пишет `read.text` в файл) либо через CLI Herdr, если в форке есть `herdr pane read` — проверить. Перед коммитом фикстуры вычитываются на предмет секретов/путей. |
| Rust, чистая логика | `#[cfg(test)]` + `cargo test` | `parse_reply` (6 кейсов), разбор строк `subscribe` (ack/error/event/мусор), разбор stdout `cswap` (JSON после предупреждения, ненулевой статус), `socket_path`/`config_dir` (приоритет env) | Без сети и сокетов: тестируются функции над `&str`. Для env-тестов — `temp_env` или сериализация через `Mutex`. |
| Интеграция мост↔сервер | Ручной стенд | Дедлайн и отсутствие заморозки (R3), ошибочный ack (R4), гонка панелей (R8), восстановление после перезапуска Herdr | `scripts/fake-herdr.mjs`: режимы `silent` (не отвечает), `slow <ms>`, `ack-error`, `close-after-ack`. Запуск через `HERDR_SOCKET_PATH`. Это dev-скрипт в `scripts/`, в бандл не попадает. |
| UI / xterm | Ручная проверка | CSP + HMR + релизный exe (R15), сохранение прокрутки/выделения (R14), рамки и курсор с широкими символами (R13), Ctrl+\ (R9), ошибки старта (R6) | Живой Herdr с Claude Code; чек-лист из DoD соответствующих шагов. xterm-тесты в jsdom в M1 не делаем — выигрыш не окупает стенд. |

Правило: любая правка `dialogue.ts` начинается с фикстуры, воспроизводящей кейс (R11 — первая задача группы C).

---

## 4. Бэклог после M1

Порядок по ценности: M2 → M3 → M4 → M5. Детализация — при открытии вехи.

### M2 — `pane.stream`: потоковое подключение к панели

**Цель.** Заменить снапшоты `pane.read` на поток: Deck получает начальное состояние + инкрементальные байты pty, xterm ведёт реальный курсор, нет мерцания и `term.reset()`. Кросс-репозиторий: метод добавляется в форк `messire/herdr-punk` (ветка `feat/accounts-sidebar-section`; `herdr agent attach` на Windows не поддерживается — отсюда новый JSON-метод), Deck его потребляет.

**Зависимости.** M1 закрыт (R3 — async-команды; R4 — корректный разбор многострочных ответов; R11 — фикстуры, чтобы `dialogue.ts` пережил смену источника данных). ADR `docs/adr/0001-pane-stream.md` до кода (architecture-documenter).

**Проверить в форке до проектирования (вопросы из аудита):**
1. Формат `pane.read source=recent format=ansi` — отрендеренные строки экрана или сырой байтовый поток? От этого зависит `text.replace(/\r\n?/g, "\n")` в `TerminalView.tsx:52` и вся `rowsByLine()`.
2. Схема событий `events.subscribe`: имя поля типа (`event` или `type`), есть ли `pane_id` в `pane.updated`, приходит ли `pane.updated` при смене label/title (иначе дерево отстаёт до 15 с).
3. Семантика `pane.send_text`: пишет байты в pty как есть или экранирует? От этого зависит работа Shift+Tab, Ctrl+стрелок, F-клавиш, уходящих текстом.
4. Named pipe на Windows: сколько экземпляров создаёт сервер, возвращает ли клиент `ERROR_PIPE_BUSY` при параллельных подключениях; нужен ли `WaitNamedPipe`.
5. Точная форма ответа `pane.read` (`{read:{text,revision}}` типизирована по памяти) и `cswap list --json`.
6. Для нового метода: модель размеров — чей размер у pty (Herdr TUI, Deck, последний подключившийся)? Нужен ли `pane.resize` или Deck подстраивает xterm под размер Herdr? Формат кадров (начальный снапшот + дельты, курсор, `revision` для ресинка), поведение при отключении клиента, лимит одновременных стримов.

**DoD.** В форке: метод задокументирован и покрыт тестом на стороне Herdr; в Deck: постоянное соединение на поток (async-клиент на tokio named pipe/unix socket, таймауты, реконнект), `TerminalView` без поллинга и без `term.reset()`, курсор от pty, `dialogue.ts` работает на буфере xterm, фикстуры R11 зелёные; ручная проверка — спиннеры Claude Code не размножаются, ввод отображается без задержки.

### M3 — действия в дереве и хоткеи

**Цель.** Контекстные меню (аккаунт: switch/refresh; workspace: focus/close/new tab), создание workspace/tab, клавиши, которые уже обещает `KeyBar`: F1 переключение аккаунта (пикер), F2 переход к ожидающему агенту (`agent_status === "blocked"`), Ctrl+K палитра команд, Esc назад.

**Зависимости.** M1 (R6 — обработка ошибок действий; R16 — честный KeyBar). Проверить в форке наличие и имена методов `workspace.create/close`, `tab.create/close`. Глобальные клавиши при фокусе в xterm перехватываются терминалом — нужен `attachCustomKeyEventHandler`, иначе F1/F2/Ctrl+K уйдут в pty. UX-спека меню/палитры — ux-designer → ui-builder → ux-parity-reviewer.

**DoD.** Каждое действие вызывает метод Herdr и видно в TUI Herdr; ошибки показываются в UI; хоткеи работают при фокусе в терминале и вне его; `KeyBar` отражает реальные клавиши.

### M4 — полировка

**Цель.** Шрифт Rajdhani в бандле (`font-src 'self'` из R15 это допускает); уведомление/звук, когда агент переходит в `blocked` (`tauri-plugin-notification` + capability); опциональный split view (две панели); сохранение размера окна (`tauri-plugin-window-state`) и выбранной панели (локально — только UI-настройки, см. принципы); правила рамок диалога для Codex (сейчас только Claude Code) — через фикстуры в `src/__fixtures__/pane-read/codex-*.ans`.

**Зависимости.** M2 для split view (два стрима) и для корректного Codex-рендера; M1 R11 для фикстур; событие `pane.agent_status_changed` — схема из M2 п.2.

**DoD.** Каждая фича с отдельным ручным чек-листом; тесты `dialogue.ts` расширены Codex-фикстурами; README обновлён.

### M5 — релиз

**Цель.** CI собирает инсталлятор по тегу и публикует GitHub Release; версия в одном месте (`"version": "../package.json"` в `tauri.conf.json`, `Cargo.toml` синхронизируется скриптом/`tauri-action`); канал обновлений (`tauri-plugin-updater`: ключ подписи в секретах, `latest.json` в релизе).

**Зависимости.** M1 R2 (воспроизводимая сборка — база); `permissions: contents: write` только в release-джобе по тегу.

**DoD.** `git tag v0.2.0 && git push --tags` → Release с `*.exe` и `latest.json`; установленный Deck предыдущей версии предлагает обновление; `rg '0\.1\.0'` даёт одно место с версией.

---

## 5. Риски и открытые вопросы

| # | Риск / вопрос | Тип | Влияние | Митигация |
|---|---|---|---|---|
| 1 | Семантика API Herdr (формат `pane.read`, поля событий, `send_text`, имена клавиш) известна по памяти, не по схеме | предположение | R7, R9, весь M2 | Прочитать исходники форка до R7/R9; зафиксировать в `docs/architecture.md` (R16) |
| 2 | `Cargo.lock` и `rust-toolchain.toml` можно создать только на машине с тулчейном (Windows msvc пользователя); из этого окружения — нет | факт | R2 | Первая задача следующей недели выполняется локально у пользователя |
| 3 | `tsc --noEmit` может быть красным уже сейчас (`as any`, `!`) | предположение | R2 | Чинить минимально в R2, остальное — R10 |
| 4 | Async-команды снимут сериализацию, которую сейчас даёт главный поток → параллельные подключения к пайпу → возможный `ERROR_PIPE_BUSY` | предположение | R3 | Семафор на 1–2 вызова в R3; проверить число экземпляров пайпа в форке (M2 п.4) |
| 5 | Дедлайн через поток + `recv_timeout` оставляет заблокированный поток до закрытия пайпа | факт | R3 | Документировать; ограничить число «брошенных» потоков; полноценный tokio named-pipe клиент в M2 |
| 6 | Unix-ветки `herdr.rs` не компилируются в CI (только Windows) | факт | R2 | Ubuntu-джоба с `cargo check` после установки webkit-зависимостей — опционально, если уложимся |
| 7 | Фикстуры `pane.read` могут содержать содержимое чужих сессий/секреты | факт | R11 | Снимать с тестовой сессии, вычитывать перед коммитом |
| 8 | Способ установки `cswap` на Windows (`.exe` или `.cmd`-шим) неизвестен — влияет на резолв пути | предположение | R5 | Спросить/проверить `where cswap` у пользователя до R5 |
| 9 | Без тестов на xterm часть регрессий pane view (R13, R14) ловится только глазами | факт | группа C | Чек-листы в DoD; при повторных регрессиях — рассмотреть jsdom-стенд в M2 |
| 10 | Двойной клик переключает глобальные креды CLI без подтверждения | факт (optional из аудита) | UX | Решение — в M3 вместе с F1-пикером; в M1 не трогаем |

---

## 6. Порядок выполнения — следующая неделя

1. **День 1 (локально у пользователя, Windows):** R1, затем R2 — сгенерировать `Cargo.lock`, зафиксировать toolchain, добавить vitest, typecheck и шаги в CI; добиться зелёного `npm run build && npm test && cargo test && cargo clippy -D warnings` локально и в CI. Пока это не зелёное, остальное не стартует.
2. **День 1–2, перед группами A/B:** открыть исходники форка и записать факты по рискам 1, 4, 8 (поля событий, `send_keys`-имена, экземпляры пайпа, `cswap` на Windows) в `docs/architecture.md` (черновик, дополнит R16).
3. **День 2–4, параллельно:** A (R3 → R4 → R5), B (R6, R7, R8 → R9, R10), C (R11 → R12 → R13 → R14), D (R15).
4. **День 4–5:** R16 документация по итоговому состоянию, затем R17 — повторное ревью по 21 находке, закрытие M1.
5. **После закрытия M1:** ADR по `pane.stream` и ответы на вопросы §4/M2 в форке — только после этого начинать M2.
