# Архитектура

Desktop-обёртка над Claude Code CLI. Принцип: **всё состояние сессии живёт в самом CLI**, приложение
только мультиплексирует терминалы, папки и аккаунты. Поэтому ядро на Rust маленькое, а вся логика
представления — на фронте.

## Стек

- **Tauri 2** — Rust-ядро + системный WebView (лёгкий бинарь, нативный доступ к pty/fs).
- **React 19 + TypeScript + Vite** — фронт.
- **Zustand** (+ `persist` в localStorage) — состояние UI: чаты/папки, раскладка панелей.
- **xterm.js + FitAddon** — терминалы в панелях.
- **portable-pty** (Rust) — ConPTY на Windows / openpty на Unix, один код на все платформы.

## Слои

```
src/                          фронт (feature-sliced)
  app/        App, theme.css (токены light/dark), keymap (⌘N, ⌘1..4)
  shared/     types, format, status
  features/
    chats/    сайдбар: папки + чаты (drag source), chats.store
    panes/    сетка 1/2/3/4, Pane (drop target), Terminal (xterm), panes.store
    new-chat/ диалог создания: папка, модель, effort (5 уровней), аккаунт,
              чекбокс "Git worktree" (по умолчанию вкл), permission mode
    accounts/ панель аккаунтов: лимиты + добавление/удаление, accounts.store
    status-bar/ часы, сводка, лимиты аккаунтов
  ipc/        commands.ts (invoke), events.ts (listen) — единственная граница с Rust

src-tauri/src/
  accounts.rs   list/create/delete папок в <accounts root>/<name>
  settings.rs   <data>/settings.json: accountsRoot (по умолчанию Documents/claude-accounts),
                меняется через ⚙ в панели аккаунтов
  cli.rs        собственная копия Claude Code CLI: скачивание с downloads.claude.ai
                (sha256 по manifest.json), versions/<ver>/, автообновление раз в 6ч
  paths.rs      data_dir(): рядом с exe (portable) либо LOCALAPPDATA\luna
  pty.rs        менеджер сессий: спавн `claude` в pty, scrollback-буфер,
                события pty://output и pty://exit, write/resize/kill
  lib.rs        Builder + generate_handler
```

## Ключевые решения

- **Тонкое Rust-ядро.** Rust не знает про «чаты» — только про pty-сессии по id и папки аккаунтов.
  Список чатов, раскладка, настройки — persisted-состояние фронта. Меньше IPC-поверхность,
  проще эволюция UI.
- **Свой бинарник CLI.** Luna не зависит от глобального `claude`: `cli.rs` держит копию в
  `<data>/claude-cli/versions/<ver>/claude.exe` (`<data>` — папка exe для портативной сборки,
  чтобы всё жило на одном диске), `current` указывает на активную версию. Версии никогда не
  перезаписываются (запущенный exe на Windows не заменить) — новая ложится рядом, указатель
  переключается, старые сметаются, когда их никто не держит. Сессиям ставится
  `DISABLE_AUTOUPDATER=1`, так что обновляет CLI только Luna. Пока копии нет — fallback на PATH.
- **Сессия = запуск `claude`** с аргументами:
  `--model <alias> --effort <low|medium|high|xhigh|max|ultracode> --permission-mode <mode> [--worktree]`,
  `cwd` = папка чата, `CLAUDE_CONFIG_DIR` = папка аккаунта (изоляция логина/настроек на аккаунт).
- **Аккаунт = папка** `<accounts root>/<name>` (по умолчанию `Documents/claude-accounts`,
  корень выбирается в настройках и хранится в Rust — его читает и фоновый поток `models.rs`).
  Создание — mkdir, удаление — rm -rf, список — readdir. Никакой собственной БД. Чаты помнят
  аккаунт по имени, поэтому смена корня не требует миграции — только перенести папки.
- **Scrollback в Rust.** Буфер вывода (2MB cap) живёт в ядре, чтобы перенос чата между панелями
  или пересоздание xterm восстанавливали экран (`ensure_session` возвращает бэклог).
- **События вместо поллинга (pty).** Вывод pty стримится событием `pty://output`; завершение —
  `pty://exit` (чат переходит в idle).
- **Лимиты аккаунтов — ноль токенов.** `limits.rs` читает OAuth-токен из
  `<аккаунт>/.credentials.json` и дергает `api.anthropic.com/api/oauth/usage` (+`/profile` для
  плана/email) — те же эндпоинты, что у `/usage` внутри Claude Code. Фронт поллит раз в 60с.
  Если токен протух — бары показывают «—» до первого запуска сессии (CLI сам рефрешит токен).
- **Тема** — токены в CSS custom properties (`[data-app][data-theme]`), переключение
  system/light/dark без перерисовки терминалов (xterm получает тему через MutationObserver).

## Сборка

- Локально: `pnpm install`, `pnpm build` (фронт), `pnpm tauri dev` (нужен Rust-тулчейн).
- Прод-сборка — в Docker, см. README.
