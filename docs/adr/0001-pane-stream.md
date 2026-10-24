# ADR-0001: Как Deck получает содержимое pane в реальном времени

Status: **Proposed** (2026-10-24, шаг 0 M2; код стриминга не начат)

Пути к форку Herdr ниже — относительно `/home/claude/herdr-fork/herdr` (ветка `feat/accounts-sidebar-section`, HEAD с `src/app/output_events.rs`). Пути Deck — относительно корня репозитория. Всё, что не удалось установить по коду, помечено «не установлено».

## Контекст

**Сейчас (M1).** `TerminalView` опрашивает `pane.read` (`source: recent`, `format: ansi`, до 400 строк) каждые 2.5 с и после собственного ввода (`src/components/TerminalView.tsx:236`, `src-tauri/src/lib.rs:64-78`), сравнивает текст, затем делает `term.reset()` и полную перерисовку с декорациями и оценочным курсором на строке промпта (`TerminalView.tsx:113-114, 170-185`). Каждый вызов — отдельное подключение к named pipe `\\.\pipe\<config>\herdr.sock`, дедлайн 15 с, не больше 2 параллельных подключений (`src-tauri/src/herdr.rs:26-31, 113, 253-263`). Подписка `events.subscribe` живёт на отдельном соединении (`herdr.rs:356-397`, `lib.rs:280-319`).

**Что изменилось в форке после дня 0 (проверено по коду, уточняет F3/F4 roadmap).**
- `pane.output_changed` теперь эмитится: headless-цикл отмечает грязные PTY-источники и сбрасывает события (`src/server/headless.rs:558-559, 612`; `src/app/api.rs:812-852`). Коалесценция — не чаще одного события на pane за 100 мс, всегда с последним `revision`, хвост всплеска не теряется (`src/app/output_events.rs:14, 34-63`).
- `revision` = `TerminalRuntime::content_seq()` и в событии, и в `pane.read` (`src/app/api.rs:827`, `src/app/api/panes.rs:1547`). Счётчик инкрементируется до и после обработки каждого PTY-чанка, поэтому чётное значение означает стабильное содержимое, нечётное — запись в процессе (`src/pane.rs:2607-2611`).
- Подписка `pane.output_changed` не фильтруется по pane: приходят события всех панелей (`src/api/schema/events.rs:65-66`, `src/api/subscriptions.rs:161-163`).
- Доставка событий: кольцо на 512 событий (`src/api/event_hub.rs:19`); поток подписки опрашивает кольцо каждые 100 мс (`src/api/server.rs:30, 858-890`); отставший подписчик получает `error.code = "events_lost"` и должен переподписаться (`src/api/subscriptions.rs:329-343`).
- `pane.read` отдаёт отрендеренные ghostty строки (`visible_ansi`/`recent_ansi_snapshot`, `src/app/api_helpers.rs:109-141`, `src/pane/terminal.rs:498, 527`), без курсора. Курсор, флаг альтернативного экрана и метрики прокрутки у runtime есть, но через JSON API не отдаются (`src/pane/terminal.rs:100-106, 464, 482, 274`).

**Ограничения форка (`CLAUDE.md`).** «Runtime/client boundary guardrail» (строки 66-81): общие факты о runtime (в том числе terminal state) должны выходить через JSON API/события; нельзя добавлять общее поведение, доступное только через приватный клиентский сокет. «Stable client endpoint contract» (строки 83-94): кодеки generation 1 неизменяемы, enum'ы — append-closed, фикстуры `tests/fixtures/endpoint-*-v1.json` — контракт; новые возможности — только через advertised methods / `EndpointControl`.

**Цель M2 (roadmap §6).** Убрать снапшоты и `term.reset()`, реальный курсор, прокрутка/выделение без потерь, `dialogue.ts` поверх буфера; 15 активных панелей не должны класть ни Herdr, ни Deck.

## Что такое endpoint generation 1 (факты для варианта C)

- **Транспорт.** Тот же `interprocess` local socket, что и у JSON API; на Windows — named pipe с именем из полного пути (`GenericNamespaced`, `src/ipc.rs:44-51`), т.е. `\\.\pipe\<config_dir>\herdr-client.sock`. Путь: `HERDR_SOCKET_PATH` → `<stem>-client.sock`; иначе `HERDR_CLIENT_SOCKET_PATH`; иначе `<config_dir>/herdr-client.sock` (`src/server/socket_paths.rs:9-58`, `src/session.rs:185-187`). TUI-клиент на Windows подключается именно так (`src/client/mod.rs:189, 245`); `herdr --remote` на Windows поддерживается (`CHANGELOG.md:257`). Ограничение «attach не работает на Windows» в коде клиентского пути не найдено — не установлено, к чему оно относится.
- **Кадрирование и кодек.** `[u32 LE длина][bincode standard]`, лимит 2 МБ (32 МБ для графики) (`src/protocol/wire.rs:20-29, 1656-1713`). Сообщения — enum'ы `ClientMessage`/`ServerMessage` с замороженным порядком вариантов (`wire.rs:459-617, 1379-1504`).
- **Handshake.** Клиент шлёт `ClientMessage::EndpointControl { kind: "endpoint.hello.v1", data: JSON(EndpointClientHello) }` с `generation: 1`, `surface_size {cols, rows}`, списками кодеков `shell.snapshot.v1`/`shell.surface.v1`/`shell.input.semantic.v1`/`shell.blob.v1`; сервер отвечает `EndpointControl { kind: "endpoint.welcome.v1", data: JSON(EndpointServerWelcome) }` с `methods[]`, `capabilities[]` или `error` (`src/protocol/endpoint.rs:15-33, 46-107`; `src/client/handshake.rs:191-277`; `src/server/client_transport.rs:738-848`). JSON внутри bincode-конверта.
- **Что приходит.** `ClientShellSnapshot` (workspaces/tabs/panes/agents, JSON в `EndpointControl`, `wire.rs:916-951`), затем `ServerMessage::PaneSurface(PaneSurfaceFrame)` и `ServerMessage::PaneSurfacePatch(PaneSurfacePatch)` (`wire.rs:1469, 1497`). `PaneSurfaceFrame.frame: FrameData` — **ячейки** `CellData { symbol, fg: u32, bg: u32, modifier: u16, skip, hyperlink }` в row-major порядке плюс `cursor: Option<CursorState { x, y, visible, shape }>` (`wire.rs:697-776`). Патч — изменённые спаны `PaneSurfacePatchRow { x, y, cells }` относительно `base_surface_revision` и итоговый курсор (`wire.rs:1289-1309`). Опциональные кодировки `surface_reuse`/`surface_delta`/`surface_scroll` включаются флагами hello (`endpoint.rs:58-66`).
- **Поверхность — это вкладка целиком**, не одна панель: BSP-раскладка всех панелей активной вкладки клиента с `PaneSurfacePane { pane_id, content_revision, rect, inner_rect, scroll, alternate_screen_active, … }` и разделителями (`wire.rs:1090-1120`; `src/server/client_shell.rs:286-400`). У каждого endpoint-клиента своя «локация» (сфокусированный workspace/tab) — `ClientShellLocation` (`src/server/clients.rs:64-107`, `src/server/headless/client_views.rs:54-90`).
- **Scrollback** не передаётся: клиент получает только viewport, прокрутка — метод `pane.scroll` и метрики `PaneSurfaceScrollMetrics { offset_from_bottom, max_offset_from_bottom, viewport_rows }` (`wire.rs:1105-1110`; `tests/fixtures/endpoint-method-shapes-v1.json:17`).
- **Ввод** — семантический: `ClientMessage::ClientShellPaneInput { pane_id, events: Vec<ClientPaneInputEvent> }` с `ClientKeyCode` (Backspace/Enter/…/Char/F) и битами модификаторов, `TextCommit`, `Paste`, `Mouse` (`wire.rs:64-84, 160-182, 579-583`).
- **Геометрия.** Активный shell-клиент претендует на управление размером вкладки: при активации он становится foreground и, если другой сфокусированный viewer не владеет вкладкой, PTY ресайзится под его `cols × rows` (`src/server/headless/surface_interest.rs:66-85`; `client_views.rs:541-580, 693-712`). То есть Deck как endpoint-клиент меняет размер PTY, который видит TUI.
- **Переиспользование кода.** Herdr — бинарный crate без `[lib]` (`Cargo.toml:1-7`, `src/main.rs:45 mod protocol;`), поэтому типы протокола нельзя подключить как зависимость; их пришлось бы скопировать в Deck с точным порядком полей и вариантов (bincode кодирует индексы). Лицензия Apache-2.0 это допускает.

## Варианты

**A. JSON API, событие + снапшот.** `events.subscribe pane.output_changed` → при событии для выбранной панели с `revision` больше последнего применённого — `pane.read --format ansi` → как сейчас, полная перерисовка в xterm. Никаких изменений в форке сверх уже сделанного (M1.5).

**B. Новая JSON-подписка в форке** — например `pane.surface { pane_id, format: ansi }` в `events.subscribe`: сервер на каждом тике опроса (100 мс) сравнивает `content_seq()` с последним отданным и, если изменился и чётный, шлёт `{event: "pane_surface", pane_id, revision, screen: <ANSI строк viewport>, cursor: {x, y, visible, shape}, alternate_screen, scroll: {…}}`. Прецедент — сэмплирующие подписки `PaneOutputMatched`/`ScrollChanged`, которые уже читают состояние на тике (`src/api/subscriptions.rs:320-324`). Источники данных на сервере есть: `visible_ansi()`, `cursor_state()`, `alternate_screen_active()`, `scroll_metrics()`. Вариант с построчными дельтами возможен, но это отдельная сложность без выигрыша для локального pipe.

**C. Deck — endpoint generation-1 клиент.** Отдельное соединение к `herdr-client.sock`, bincode-кадры, hello/welcome, разбор `ClientShellSnapshot`, `PaneSurface`/`PaneSurfacePatch`, выбор вкладки через `tab.focus`/`workspace.focus` своей локации, вырезание `inner_rect` нужной панели из кадра, трансляция `CellData` в ANSI для xterm (аналог серверного `src/protocol/render_ansi.rs`), ввод через `ClientShellPaneInput`.

### Сравнение

| Критерий | A: событие + `pane.read` | B: JSON-подписка `pane.surface` | C: endpoint gen-1 |
|---|---|---|---|
| Латентность (оценка по коду, не замер) | PTY → коалесценция ≤100 мс → опрос подписки ≤100 мс → Deck → новое подключение + `pane.read` (рендер до 400 строк) → `reset` + перерисовка. Порядка 150–400 мс плюс мерцание | Те же два шага ≤100 мс, но payload уже в событии; без второго round-trip. Порядка 100–250 мс | Патч на каждом цикле рендера сервера после грязного PTY (`headless.rs:554-559`); частота кадров по коду не измерялась. Наименьшая |
| Трафик при 15 активных панелях | События всех 15 панелей (нет фильтра): ≤150 соб/с × ~120 Б ≈ 20 КБ/с. Снапшоты — только для просматриваемой: ≤10/с × размер 400-строчного ANSI (десятки КБ) → сотни КБ/с и 10 подключений/с к pipe | Payload только по подписанным панелям; viewport (например 50×200 с SGR, ~10–30 КБ) ≤10/с ≈ 100–300 КБ/с на просматриваемую панель; остальные 14 — ничего, если не подписаны | Патчи только изменённых спанов видимой вкладки; полный кадр (~8–16 Б/ячейка bincode, 10 тыс. ячеек ≈ 100–150 КБ) при смене раскладки/размера. Панели вне вкладки — ничего |
| Курсор | Нет (F4); остаётся эвристика `promptLine` | Есть: `cursor_state()` → `{x,y,visible,shape}` | Есть: `FrameData.cursor`/`PaneSurfacePatch.cursor` |
| Scrollback | `source: recent` до 1000 строк — локальный scrollback xterm заполняется из снапшота, но каждый `reset` его пересобирает | Только viewport в событии; история — отдельный `pane.read recent` при первом показе, далее дописывать нельзя без дельт строк → scrollback xterm деградирует до «viewport + начальный снимок» | Только viewport; прокрутка серверная (`pane.scroll` + метрики). Локальный scrollback xterm пуст; выделение «вверх» — через серверные методы |
| Альтернативный экран | Что возвращает `recent_ansi_snapshot` при активном alt-screen — не установлено | Флаг `alternate_screen_active` + viewport: корректно по построению | Флаг в `PaneSurfacePane` + ячейки: корректно по построению |
| Ширина строк | Снапшот отрендерен при `cols` PTY (размер TUI); в xterm другой ширины перенос строк расходится (остаток п.11) | То же: Deck не управляет размером PTY | Deck становится контроллером геометрии вкладки и ресайзит PTY под себя — TUI видит смену размера |
| Сложность Rust (Deck) | ~0: пump уже есть; добавить подписку на `pane.output_changed` | Малая: новый тип события в `classify_line`/маршрутизации; одно долгоживущее соединение | Большая: второй транспорт, bincode-кадры, копия ~40 типов `wire.rs`/`endpoint.rs` с точным порядком вариантов, handshake, реконнект, обработка `boot_id`/`surface_revision`, кроппинг `inner_rect`, генерация ANSI из `CellData`, семантический ввод |
| Сложность TS | Малая: `routeEvent` → `tick` по `pane_id`/`revision`, поднять fallback-опрос до 10 с | Средняя: писать viewport без `reset` (CUP + строки), курсор по `cursor`, `dialogue.ts` по строкам viewport | Средняя-большая: тот же рендер viewport + серверная прокрутка, маппинг клавиш в `ClientKeyCode` вместо `keys.ts` |
| Сложность в форке | 0 (M1.5 уже сделан) | Средняя: `Subscription::PaneSurface`, сэмплирующая ветка в `poll_batch`, новый `EventData`, тесты; формат — JSON, вне горячего цикла рендера | 0 на сервере |
| Работа с upstream Herdr | Деградация до опроса (upstream не эмитит событие — F3) | Деградация до варианта A/опроса; стриминга нет | Да, если upstream держит gen-1 (обязательство в `CLAUDE.md:85`) |
| Риск ломки при обновлении Herdr | Низкий: схема `pane.read`/события общая с upstream | Средний: форк-only код, вечный rebase; формат под контролем Deck | Низкий по контракту, но высокий по реализации: скопированные enum'ы append-closed; любой новый вариант/поле в upstream — тихий рассинхрон декодера, тесты Deck его не поймают без собственных фикстур-дайджестов |
| Влияние на `dialogue.ts` (строки ANSI) | Нет | Нет: viewport приходит строками ANSI; классификатор ограничен viewport'ом (история — начальный снимок) | Нужен слой «ячейки → строки ANSI» перед классификацией; история недоступна |
| Соответствие guardrail форка | Да | Да: terminal state через JSON API — ровно то, что предписывает `CLAUDE.md:72, 79` | Да (ничего не меняем), но Deck завязывается на «private same-install»-контур поведения (геометрия, foreground) |

## Решение

**Поэтапно: A сразу, B как целевой M2, C не выбирается.**

Обоснование.
1. **A — немедленно и бесплатно.** После M1.5 единственное, чего не хватает, — подписка на `pane.output_changed` и сравнение `revision` в Deck. Это убирает 2.5-секундную задержку при нулевых изменениях в форке и совместимо с upstream (деградация до опроса). Не решает курсор, мерцание и ширину.
2. **B решает цели M2 с наименьшей совокупной сложностью.** Курсор, alt-screen и метрики прокрутки уже есть у runtime; формат — строки ANSI, то есть `dialogue.ts`, фикстуры R11 и `keys.ts` остаются как есть. Одно долгоживущее соединение вместо 10 подключений/с снимает риск `ERROR_PIPE_BUSY` (F8). Это прямо соответствует guardrail форка («terminal state → server/runtime → JSON API»), поэтому у метода есть шанс попасть в upstream; пока не попал — стоимость rebase ограничена одним файлом подписок и одним типом события.
3. **C отклоняется для M2.** Выгоды (патчи ячеек, нулевые правки сервера) не окупают: (а) Herdr не является библиотекой — Deck должен поддерживать копию замороженных, но append-closed типов без помощи компилятора; (б) поверхность — вся вкладка, а не панель: нужен кроппинг, а split view M4 всё равно потребует собственной раскладки Deck; (в) Deck как активный shell-клиент перехватывает геометрию PTY и статус foreground у TUI (`surface_interest.rs:66-85`), что меняет поведение Herdr у пользователя — недопустимо для «тонкого клиента-наблюдателя»; (г) серверная прокрутка и отсутствие истории ухудшают выделение/чтение по сравнению с xterm-scrollback. C остаётся запасным путём, если B будет отклонён как upstream-непригодный и rebase станет дороже копии протокола.

### План

**M2, сразу (Deck, без форка)** — реализовано в M1.5 (см. roadmap §6), кроме обработки нечётного `revision` (вопрос 6) и отдельного кода для `events_lost`:
- `lib.rs::start_event_pump`: добавить `"pane.output_changed"` в подписку; `routeEvent`: `pane_output_changed` с `pane_id === selected` → `tick`, если `revision` чётный и больше применённого; нечётный — отложить до следующего события или fallback.
- `TerminalView`: fallback-опрос 10 с; хранить `appliedRevision`; `pane.read` по тику; отбрасывать ответ, если `read.revision < appliedRevision`.
- Обработать `events_lost` в pump: переподписка + принудительный `refetchTree` и `tick` (сейчас ошибка приводит к баннеру OFFLINE и реконнекту через 2 с, `lib.rs:308-317` — этого достаточно, но без OFFLINE-баннера для этого кода).
- DEV-счётчики частоты `pane.read` (как в R7) — критерий: при молчащей панели вызовов нет.

**M2, целевое (форк + Deck):**
- Форк: `Subscription::PaneSurface { pane_id, format }` в `events.rs`/`subscriptions.rs`; сэмплирующая ветка в `poll_batch` рядом с `OutputMatched`: при изменении чётного `content_seq()` — событие `pane_surface` с `screen` (`visible_ansi`), `cursor`, `alternate_screen`, `scroll`, `revision`, `cols`, `rows`. Тесты: эмиссия один раз на revision, пропуск нечётных, отсутствие событий при тишине. Формат зафиксировать в `docs/architecture.md` Deck и в схеме форка (`schemars`).
- Deck Rust: тип события в `classify_line`-маршруте без изменений (это обычный `{event: …}`); отдельное соединение подписки на выбранную панель (переподписка при смене панели).
- Deck TS: рендер viewport без `term.reset()` (синхронизированный вывод `CSI ?2026h/l`, CUP на строку, запись строк, установка курсора по `cursor`); `dialogue.ts` по строкам viewport; scrollback — начальный `pane.read recent` при открытии панели, далее viewport.
- Критерий DoD M2 (roadmap) переформулировать: «курсор от pty» и «нет `term.reset()`» — выполняются; «scrollback естественно» — частично (см. открытые вопросы).

**Отложить:** split view на одной подписке (M4 — несколько `pane.surface` одновременно); дельты строк внутри `pane_surface` (только если замер покажет, что viewport ≤10/с — узкое место); вариант C — пересмотреть, если появится `[lib]`-таргет протокола в upstream или B будет отвергнут.

## Последствия

- Плюс: A даёт выигрыш за один PR без изменений протокола; B даёт настоящий курсор и убирает мерцание, не трогая классификатор и фикстуры.
- Минус: до B остаются полная перерисовка и оценочный курсор; после B история панели — снимок на момент открытия, scrollback не «живой»; ширина строк по-прежнему определяется PTY/TUI.
- Стоимость: B — постоянная позиция в rebase форка (один файл подписок + схема), пока не принят upstream.
- Совместимость: с upstream Herdr Deck работает в режиме A/опроса; обнаружение возможности — по ошибке подписки на неизвестный `type` (сервер отклоняет всю подписку, R4), поэтому `pane.surface` нужно открывать отдельным соединением, не смешивая со структурными событиями.
- Риски: нагрузка на ghostty-lock при 10 рендерах/с `visible_ansi` на подписчика (один подписчик — одна панель; замерить); `events_lost` при 15 шумных панелях — 512 событий ≈ 3.4 с при 150 соб/с, поток подписки опрашивает каждые 100 мс, так что потери возможны только при блокировке записи в pipe Deck.

## Открытые вопросы

1. Поведение `recent_ansi_snapshot` при активном альтернативном экране (нужно для режима A): проверить на живом Herdr с `less`/`vim`.
2. Стоимость `visible_ansi()` под `core.lock()` при 10 вызовах/с на фоне 15 активных PTY — замер `just bench-render-scale` или аналог в форке до принятия B.
3. Нужна ли Deck «живая» история (дельты строк) или достаточно viewport + начального снимка; ответ определяет, нужен ли второй формат `pane.surface`.
4. Готов ли upstream принять `pane.surface`-подписку; в материалах обсуждения с upstream нет — если нет, B остаётся форк-only.
5. Как Deck узнаёт, что сервер — форк с `pane.surface`: по ошибке подписки или добавить метод в `ping`/capabilities форка.
6. Для режима A: допустимо ли читать по нечётному `revision` (запись в процессе) или ждать следующего события — проверить, насколько часто события приходят с нечётным значением.
7. ~~Имя файла~~ — roadmap §6 приведён к `docs/adr/0001-pane-stream.md` (2026-10-24).

## Ссылки

Deck: `src-tauri/src/herdr.rs:26-31, 113, 253-263, 356-397`; `src-tauri/src/lib.rs:64-78, 280-319`; `src/components/TerminalView.tsx:62-124, 170-185, 236`; `src/dialogue.ts:81, 177`; `docs/plan/ROADMAP.md` §2.1 F3/F4/F7/F8, §6 M1.5/M2.

Форк: `CLAUDE.md:66-94`; `src/app/output_events.rs:14, 34-76`; `src/app/api.rs:812-852`; `src/server/headless.rs:554-559, 612`; `src/app/api/panes.rs:1512-1552`; `src/app/api_helpers.rs:109-141`; `src/pane.rs:2602-2612, 3357`; `src/pane/terminal.rs:100-106, 274, 464, 482, 498, 527`; `src/api/event_hub.rs:19-67`; `src/api/server.rs:30, 811-890`; `src/api/subscriptions.rs:161-163, 290-343`; `src/api/schema/events.rs:65-66, 175-179, 281`; `src/ipc.rs:35-52`; `src/server/socket_paths.rs:9-58`; `src/session.rs:185-187`; `src/protocol/wire.rs:20-29, 64-84, 160-182, 459-617, 697-776, 916-951, 1090-1120, 1289-1309, 1379-1504, 1656-1713`; `src/protocol/endpoint.rs:15-107`; `src/protocol/surface_delta.rs:1-28`; `src/protocol/render_ansi.rs:1-27`; `src/client/handshake.rs:191-277`; `src/client/mod.rs:189, 245`; `src/server/client_transport.rs:738-848`; `src/server/client_shell.rs:286-400`; `src/server/clients.rs:64-107, 502-530`; `src/server/headless/client_views.rs:54-90, 541-580, 693-712`; `src/server/headless/surface_interest.rs:9-98`; `src/server/headless/retained_surface.rs:227-306`; `tests/fixtures/endpoint-method-shapes-v1.json`; `tests/fixtures/endpoint-snapshot-v1.json`; `Cargo.toml:1-7`; `src/main.rs:45`; `CHANGELOG.md:257`.
