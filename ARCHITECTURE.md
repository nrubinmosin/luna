# Архитектура

Desktop-обёртка над Claude Code CLI и Codex CLI. Принцип: **всё состояние сессии живёт в самом CLI**,
приложение только мультиплексирует терминалы, папки и аккаунты. Поэтому ядро на Rust маленькое, а вся
логика представления — на фронте.

Два провайдера — два независимых слоя (`claude/` и `codex/` и в Rust, и на фронте). Общее между ними —
только pty, worktree-уборка, папка аккаунтов и механика скачивания бинарника. Всё, что касается
настроек, лимитов, trust и чтения сессии с диска, у каждого своё и в своих терминах: Codex не
знает слов «permission mode» и «Fable», Claude — «sandbox» и «rollout».

## Стек

- **Tauri 2** — Rust-ядро + системный WebView (лёгкий бинарь, нативный доступ к pty/fs).
- **React 19 + TypeScript + Vite** — фронт.
- **Zustand** (+ `persist` в localStorage) — состояние UI: чаты/папки, раскладка панелей.
- **xterm.js + FitAddon** — терминалы в панелях.
- **portable-pty** (Rust) — ConPTY на Windows / openpty на Unix, один код на все платформы.

## Слои

```
src/                          фронт (feature-sliced)
  app/        App, theme.css (токены light/dark), keymap: Ctrl+N / Ctrl+Shift+N,
              Ctrl+1..4 (peek), Ctrl+0 (выход), Ctrl+Shift+1..4 (раскладка) —
              слушается в capture-фазе, иначе xterm гасит их до window
  shared/     types, format, status
  features/
    chats/    сайдбар: папки + чаты (drag source, клик = показать), chats.store
    panes/    сетка 1/2/3/4, Pane (drop target), Terminal (xterm), panes.store;
              peek — лист поверх притемнённой доски, той же панелью без перемонтирования;
              terminals.ts — реестр живых xterm'ов, переживающих свои панели
    new-chat/ диалог создания: папка, аккаунт (он же выбирает провайдера), чекбокс
              "Git worktree" (по умолчанию вкл), дальше поля провайдера (Fields из
              providers/*) — они открываются на том, что скажут *_defaults; draft.ts —
              черновик настроек с пометкой «тронуто рукой»; create.ts — общий путь
              создания для диалога и для Ctrl+Shift+N (для Codex здесь же делается worktree)
    providers/ claude/ и codex/ — всё, что UI знает о каждом CLI: STOCK, defaults,
              Fields диалога, Chips титлбара, LimitBars ряда аккаунта, login, trust,
              регэкспы worktree; index.ts — ui(provider) для того немногого, что общее
    accounts/ панель аккаунтов: бейдж провайдера, лимиты + добавление/удаление, accounts.store
    settings/ диалог за ⚙: папка аккаунтов, версии Luna/Claude Code/Codex и кнопки обновления
    status-bar/ часы, сводка, лимиты аккаунтов; чип обновления — только когда есть новость
    power/    чип ⏻ и меню: keep-awake, взвод «выключить/усыпить, когда всё закончится»;
              power.store слушает power://state, сам ничего не считает
    agents/   bridge.ts — фронтовая половина агентов: agent://spawn → createChat с parentId
              и ensure_* с промптом → agent_spawned; agent://deleted → deleteChat;
              agentAccounts.store — какие аккаунты агентам нельзя (settings.json ядра)
  ipc/        commands.ts (invoke), events.ts (listen) — единственная граница с Rust

src-tauri/src/
  provider.rs   enum Provider { Claude, Codex } — тег, больше ничего
  accounts.rs   list/create/delete папок в <accounts root>/{anthropic,openai}/<name>;
                Codex-аккаунт при создании получает seed config.toml
  settings.rs   <data>/settings.json: accountsRoot (по умолчанию Documents/luna-accounts),
                меняется через ⚙ в панели аккаунтов
  cli.rs        общая механика собственных копий CLI: versions/<ver>/, current, sha256,
                прогресс, prune, автообновление раз в 6ч; откуда брать версию — Source
                провайдера
  pty.rs        менеджер сессий: общий spawn в pty, scrollback-буфер, события pty://output
                и pty://exit, write/resize/kill; ensure_claude_session / ensure_codex_session
                собирают командную строку; session_meta диспетчит по провайдеру
  worktree.rs   уборка worktree обоих провайдеров (.claude/worktrees, .codex/worktrees),
                create_worktree для Codex, поиск сирот
  throttle.rs   cool-off после 429 usage-эндпоинтов, общий на оба
  procs.rs      что у CLI запущено под ним: Job Object на сессию, список pid,
                CPU/IO/время старта каждого, правило «занят» (judge)
  hub.rs        loopback HTTP (tiny_http) на случайном порту с секретом на запуск;
                POST /hook/<chat>/<secret> принимает stdin hook'ов Claude Code;
                POST /mcp с bearer-токеном сессии — MCP для агентов; поток на запрос
  mcp.rs        JSON-RPC MCP-сервера: initialize (с instructions), tools/list, tools/call;
                семь инструментов luna_* с короткими описаниями (~2k токенов)
  agents.rs     реестр сессий с инструментами и порождённых ими: токены, родство,
                права (только потомки), spawn через фронт (agent://spawn ↔ agent_spawned),
                send в pty, read из транскрипта, wait по activity, kill, delete (+worktree)
  activity.rs   семплер раз в 5 с: ход (hooks + registry / rollout), потомки, вывод →
                busy / waiting / idle на сессию и сводка
  power.rs      keep-awake (PowerSetRequest) и «выключить, когда всё закончится»:
                взвод, тихое окно, отсчёт, действие; команды и событие power://state
  claude/
    cli.rs      Source: downloads.claude.ai, latest + manifest.json, голый exe
    defaults.rs model/effortLevel/permissions.defaultMode из настроек Claude Code в его
                порядке: settings.json аккаунта → .claude/settings.json проекта →
                .claude/settings.local.json → machine-wide managed
    limits.rs   OAuth usage + кэш CLI (.claude.json)
    oauth.rs    рефреш протухшего access-токена так же, как это делает CLI (его локи, CAS)
    models.rs   окна контекста из Models API
    trust.rs    hasTrustDialogAccepted в .claude.json
    session.rs  registry <config>/sessions/<pid>.json + транскрипт projects/<cwd>/<sid>.jsonl:
                контекст, тайтл CLI (custom-title / ai-title), первый промпт
  codex/
    cli.rs      Source: GitHub Releases openai/codex, rust-v<ver>, package-тарбол + SHA256SUMS
    config.rs   config.toml через toml_edit (комментарии и формат сохраняются), seed
    defaults.rs model / model_reasoning_effort / approval_policy / sandbox_mode из
                config.toml аккаунта, затем .codex/config.toml проекта (только trusted)
    limits.rs   chatgpt.com/backend-api/wham/usage по auth.json; fallback — rate_limits
                из последнего token_count в rollout
    trust.rs    [projects.'<путь>'] trust_level = "trusted" в config.toml
    session.rs  rollout-файлы sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl: какой наш,
                статус, модель, контекст, первый промпт; имя треда из session_index.jsonl
  paths.rs      data_dir(): рядом с exe (portable) либо LOCALAPPDATA\luna
  lib.rs        Builder + generate_handler
```

## Ключевые решения

- **Тонкое Rust-ядро.** Rust не знает про «чаты» — только про pty-сессии по id и папки аккаунтов.
  Список чатов, раскладка, настройки — persisted-состояние фронта. Меньше IPC-поверхность,
  проще эволюция UI.
- **Свои бинарники CLI.** Luna не зависит от глобальных `claude`/`codex`: `cli.rs` держит копии в
  `<data>/claude-cli/versions/<ver>/claude.exe` и `<data>/codex-cli/versions/<ver>/bin/codex.exe`
  (`<data>` — папка exe для портативной сборки, чтобы всё жило на одном диске), `current`
  указывает на активную версию. Версии никогда не перезаписываются (запущенный exe на Windows не
  заменить) — новая ложится рядом, указатель переключается, старые сметаются, когда их никто не
  держит. Claude-сессиям ставится `DISABLE_AUTOUPDATER=1`, Codex-аккаунтам —
  `check_for_update_on_startup = false`, так что обновляет CLI только Luna. Пока копии нет —
  fallback на PATH. Codex приезжает package-тарболом (единственный ассет с строкой в
  `codex-package_SHA256SUMS`, и в нём лежат `codex-command-runner.exe` и прочие хелперы
  Windows-песочницы), распаковывается целиком в папку версии.
- **Сессия Claude = запуск `claude`** с аргументами:
  `--model <alias> --effort <low|medium|high|xhigh|max|ultracode> --permission-mode <mode> [--worktree]`,
  `cwd` = папка чата, `CLAUDE_CONFIG_DIR` = папка аккаунта (изоляция логина/настроек на аккаунт).
- **Сессия Codex = запуск `codex`** с аргументами:
  `[resume <uuid>] -C <cwd> [--model <m>] -c model_reasoning_effort="<e>" -a <on-request|never>
  -s <read-only|workspace-write|danger-full-access>` (пара `never` + `danger-full-access`
  заменяется на `--dangerously-bypass-approvals-and-sandbox`), `CODEX_HOME` = папка аккаунта.
  Worktree для Codex делает Luna сама (`git worktree add -b codex-<hex> <папка>/.codex/worktrees/codex-<hex>`,
  плюс запись в `.git/info/exclude`) и запускает сессию уже внутри — нативный `--worktree` Codex
  экспериментальный, кладёт checkout под `CODEX_HOME` и не чистит за собой. Логин — та же команда
  с `login: true`, т.е. `codex login` в папке аккаунта.
- Флаги ставятся всегда, поэтому собственные настройки CLI были бы перекрыты на каждой
  сессии — `claude/defaults.rs` и `codex/defaults.rs` читают их сами и подставляют в диалог, так
  что по умолчанию флаги совпадают с файлами, а расхождение бывает только там, где его попросили
  руками. Модель у Codex — свободная строка; пусто = флаг не ставится, Codex берёт свой дефолт, а
  что он реально запустил, читается из `turn_context` rollout-файла (`modelSeen` в чате).
- **Аккаунт = папка** `<accounts root>/anthropic/<name>` или `<accounts root>/openai/<name>`
  (по умолчанию `Documents/luna-accounts`, корень выбирается в настройках и хранится в Rust — его
  читает и фоновый поток `models.rs`). Создание — mkdir, удаление — rm -rf, список — readdir по
  двум подпапкам. Никакой собственной БД. Чаты помнят провайдера и имя аккаунта, поэтому смена
  корня не требует миграции — только перенести папки.
- **Сессия Codex на диске.** У Codex нет живого registry, только rollout-файл треда
  `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<локальное время>-<uuid>.jsonl`, строка = JSON
  `{timestamp,type,payload}`. Какой файл наш: для свежей сессии — самый ранний из появившихся
  после спавна с тем же cwd в `session_meta`, для возобновлённой — по uuid в имени; ответ
  запоминается на чат, так что два чата в одной папке не делят файл. Из хвоста (128 KiB):
  `turn_started`/`turn_complete`/`turn_aborted` → working/resting, `token_count.info` →
  контекст (`last_token_usage.total_tokens − reasoning_output_tokens` к `model_context_window`,
  как считает сам Codex), `turn_context.model` → модель, `token_count.rate_limits` → fallback
  лимитов. Approval-запросы в rollout не пишутся, поэтому «waiting» для Codex — это turn в
  полёте при экране, который не менялся 3 с (спиннер Codex рисует непрерывно).
- **Scrollback в Rust.** Буфер вывода (2MB cap) живёт в ядре, чтобы перенос чата между панелями
  или пересоздание xterm восстанавливали экран (`ensure_session` возвращает бэклог).
- **Терминал переживает панель.** Раньше xterm принадлежал панели, и смена чата в ней стоила
  `ensure_session` с этим самым бэклогом, reset и ожидания перерисовки от CLI — то есть ровно
  столько, сколько стоит переключение чата в режиме одного окна. Теперь xterm'ы живут в
  `terminals.ts`: панель берёт готовый и возвращает его на «парковку» (за экраном, но в
  документе — иначе рендерер и fit-аддон меряют ноль). Три последних отложенных ждут там со
  всеми подписками, дальше вытесняется самый давний. Возврат = refit плюс пинок на перерисовку.
- **События вместо поллинга (pty).** Вывод pty стримится событием `pty://output`; завершение —
  `pty://exit` (чат переходит в idle).
- **Лимиты аккаунтов — ноль токенов.** `claude/limits.rs` читает OAuth-токен из
  `<аккаунт>/.credentials.json` и дергает `api.anthropic.com/api/oauth/usage` — тот же эндпоинт,
  что у `/usage` внутри Claude Code; кэш CLI в `.claude.json` отвечает, пока свежий.
  `codex/limits.rs` читает `tokens.access_token`/`account_id` из `<аккаунт>/auth.json` (email и
  план — из claims `id_token`) и дергает `chatgpt.com/backend-api/wham/usage` — то же, что
  `/status` внутри Codex; окна `primary`/`secondary` плюс `additional_rate_limits` отдаются как
  список с подписями (300 мин → «5 hours», 10080 → «week»). Если сети или токена нет — последний
  `token_count.rate_limits` из rollout. Фронт поллит раз в 60с; ↻ в ряду аккаунта спрашивает сразу,
  мимо свежего кэша CLI и cool-off. Cool-off после 429 общий (`throttle.rs`).
- **Рефреш токена Claude.** Access-токен живёт ~8 ч, и раньше аккаунт, который давно не открывали,
  висел на «waiting for token refresh» до первой сессии. Теперь `claude/oauth.rs` меняет refresh-токен
  сам — тот же `POST platform.claude.com/v1/oauth/token` с client_id и scopes CLI. Refresh-токены
  ротируются, поэтому только по правилам CLI: оба его mkdir-лока (`<config>/.oauth_refresh.lock` и
  `<config>.lock`, stale через 60 с), перечитать файл под локом, записать только если refresh-токен
  на диске всё ещё тот, что отправили. CLI, пришедший рефрешить после нас, видит на диске другой
  access-токен и берёт его (его собственная ветка «race resolved»). `invalid_grant` = нужен новый
  логин, аккаунт показывается signed out. Codex не трогаем: его токен живёт 10 дней, рефрешит он
  сам, а уже использованный refresh-токен сервер отвергает («refresh token was already used. Please
  log out and sign in again») — наш рефреш рядом с живым Codex, помнящим старый токен, его разлогинит.
- **Тайтлы чатов — от CLI.** Claude Code после первого сообщения генерирует название сессии (то же,
  что ставит на вкладку терминала) и пишет его в транскрипт строкой `ai-title`; `/rename` пишет
  `custom-title`. Обе перезаписываются в хвост по ходу сессии, так что читаются вместе с контекстом.
  Порядок: `custom-title` → имя registry с `nameSource: user` → `ai-title` → имя с `nameSource:
  auto` → первый промпт (только пока тайтла ещё нет). Для чатов без живой сессии `saved_title`
  один раз за запуск находит транскрипт по session id. У Codex — последняя строка с этим id в
  `<CODEX_HOME>/session_index.jsonl`.
- **Trust.** Ни один CLI не может показать свой trust-промпт так, как его запускает Luna (Claude
  Code отказывается под `--worktree`, Codex после него задаёт вопрос про Windows-песочницу),
  поэтому бит пишется до спавна: `hasTrustDialogAccepted` в `.claude.json` либо
  `[projects.'<native путь>'] trust_level = "trusted"` в `config.toml` (через `toml_edit`, чтобы
  не потерять комментарии пользователя; ключ сравнивается без регистра и с любым слэшем).
- **Занятость сессии измеряется, а не читается из одного статуса.** `activity.rs` раз в 5 с
  сводит три сигнала: ход (у Claude — hooks через `--settings`-файл с `curl.exe` на loopback
  `hub.rs`, сверенные с registry; у Codex — rollout плюс тишина экрана), потомки CLI
  (`procs.rs`: при спавне CLI кладётся в Job Object, потомки наследуют его; «занят» = прирост
  CPU/IO за окно либо процесс, родившийся после начала последнего хода и ещё живой — так
  ловится `sleep` в фоне, а MCP-серверы, поднявшиеся вместе с сессией, считаются мебелью) и
  вывод pty за последние 30 с. Hooks ничего не печатают: stdout hook'а с кодом 0 Claude Code
  добавляет в контекст, а молчание бесплатно. Регистр Claude бьёт «idle» от hook, hook
  «waiting» бьёт «busy» регистра (промпт разрешения — внутри хода). Ни одна метка не вечна:
  hook «busy» против «idle» регистра стареет за 15 с (пропущенный Stop), hook «waiting» — за
  60 с (промпт ушёл вместе с ходом по Esc, Stop не пришёл), «busy» регистра против hook «idle»
  — за 60 с без единого hook'а (ход, который начался или что-то сделал, hook'ом отметится).
  Без hooks (Codex, сессия, пережившая перезапуск Luna) регистр верится как есть.
- **Питание следует за занятостью** (`power.rs`). Keep-awake — `PowerSetRequest(SystemRequired)`
  с причиной, видимой в `powercfg /requests`, пока busy > 0, отпускается через 60 с тишины;
  `waiting` машину не держит (никто не отвечает), но блокирует выключение (работа не
  закончена). Взвод живёт до выхода Luna; при взводе hold держится всегда, иначе ПК уснёт в
  тихом окне. Автомат: busy = 0 и waiting = 0 непрерывно quiet_s → отсчёт 60 с с
  уведомлением → действие. Shutdown закрывает сессии через `shut_down` с grace 5 с (exit-hooks
  CLI) и `shutdown /s /t 0`; sleep/hibernate — `SetSuspendState`, сессии живут, взвод остаётся,
  а скачок часов при пробуждении сбрасывает тихое окно и отсчёт. Взведённое правило, которое
  не срабатывает, объясняется в `luna.log`: строка `armed, not firing: <chat> <почему> [hook …,
  registry …]` при смене причины (не чаще раза в минуту) и раз в 10 минут без перемен; в
  `luna.log` идут только WARN/ERROR, поэтому всё, что решает судьбу ночи, пишется этим уровнем.
- **Агенты — через MCP, а не через промпт.** Сессия с галочкой «Luna tools» получает
  `--mcp-config <data>/mcp/<chat>.json` (Claude) или `-c mcp_servers.luna.url=… -c
  mcp_servers.luna.bearer_token_env_var=LUNA_MCP_TOKEN` (Codex) — Luna-сервер на loopback
  `hub.rs`, токен на сессию, перевыпускается при каждом спавне. Инструкция агенту едет в
  `instructions` ответа `initialize`, поэтому системный промпт не трогается ни флагом, ни
  файлом; всё вместе ~2k токенов один раз. Чистые сессии не получают ни MCP, ни знания о Luna.
  Ядро чатов не знает, поэтому spawn — round trip: `agent://spawn` во фронт, тот делает чат
  (`parentId`, без посадки в панель) и `ensure_*` с промптом позиционным аргументом CLI,
  и отвечает `agent_spawned`; ядро ждёт на Condvar до 90 с. `read` — из транскрипта Claude
  (user/assistant, без tool_result) или rollout Codex (`user_message`/`agent_message`) с
  байтовым курсором; `wait(turn_done)` считает переходы busy→idle в `activity.rs`
  (`turns_ended`) и возвращает последний ответ, чтобы `read` был не нужен. Права: только
  потомки вызывающего; глубина инструментов 1; максимум 8 потомков. Дети переживают
  родителя (сироты с ↳); удаление ребёнком через `delete` убирает worktree, если попросили.
- **Тема** — токены в CSS custom properties (`[data-app][data-theme]`), переключение
  system/light/dark без перерисовки терминалов (xterm получает тему через MutationObserver).

## Сборка

- Локально: `pnpm install`, `pnpm build` (фронт), `pnpm tauri dev` (нужен Rust-тулчейн).
- Прод-сборка — в Docker, см. README.
