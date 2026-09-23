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
  claude/
    cli.rs      Source: downloads.claude.ai, latest + manifest.json, голый exe
    defaults.rs model/effortLevel/permissions.defaultMode из настроек Claude Code в его
                порядке: settings.json аккаунта → .claude/settings.json проекта →
                .claude/settings.local.json → machine-wide managed
    limits.rs   OAuth usage + кэш CLI (.claude.json)
    oauth.rs    рефреш протухшего access-токена так же, как это делает CLI (его локи, CAS)
    models.rs   окна контекста из Models API
    trust.rs    hasTrustDialogAccepted в .claude.json
    session.rs  registry <config>/sessions/<pid>.json + транскрипт projects/<cwd>/<sid>.jsonl
  codex/
    cli.rs      Source: GitHub Releases openai/codex, rust-v<ver>, package-тарбол + SHA256SUMS
    config.rs   config.toml через toml_edit (комментарии и формат сохраняются), seed
    defaults.rs model / model_reasoning_effort / approval_policy / sandbox_mode из
                config.toml аккаунта, затем .codex/config.toml проекта (только trusted)
    limits.rs   chatgpt.com/backend-api/wham/usage по auth.json; fallback — rate_limits
                из последнего token_count в rollout
    trust.rs    [projects.'<путь>'] trust_level = "trusted" в config.toml
    session.rs  rollout-файлы sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl: какой наш,
                статус, модель, контекст, первый промпт
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
- **Trust.** Ни один CLI не может показать свой trust-промпт так, как его запускает Luna (Claude
  Code отказывается под `--worktree`, Codex после него задаёт вопрос про Windows-песочницу),
  поэтому бит пишется до спавна: `hasTrustDialogAccepted` в `.claude.json` либо
  `[projects.'<native путь>'] trust_level = "trusted"` в `config.toml` (через `toml_edit`, чтобы
  не потерять комментарии пользователя; ключ сравнивается без регистра и с любым слэшем).
- **Тема** — токены в CSS custom properties (`[data-app][data-theme]`), переключение
  system/light/dark без перерисовки терминалов (xterm получает тему через MutationObserver).

## Сборка

- Локально: `pnpm install`, `pnpm build` (фронт), `pnpm tauri dev` (нужен Rust-тулчейн).
- Прод-сборка — в Docker, см. README.
