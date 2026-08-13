# Luna

*[English version](README.md)*

Тонкая desktop-обёртка (Rust + Tauri) над Claude Code CLI: несколько сессий в панелях,
папки-проекты, аккаунты с изолированными конфигами. Использует **глобально установленный**
бинарник `claude`.

Подробности по устройству — в [ARCHITECTURE.md](ARCHITECTURE.md).

## Возможности

- Оболочка под Windows XP (xp.css, Luna Blue): вместо системной рамки — своя, с
  титлбаром и кнопками свернуть/развернуть/закрыть; шрифт интерфейса Tahoma.
  Масштаб всего интерфейса задаётся одной переменной `--ui` в `src/app/theme.css`.
- Сетка 1/2/3/4 панелей, drag&drop чатов из сайдбара (⌘/Ctrl+1..4).
- Группы окон I/II/III/IV — вкладки под селектором раскладки. Каждая группа помнит свои
  четыре доски (по одной на раскладку) и их сплиты, у каждой своя кнопка сброса (↺ на
  вкладке): она чистит только расстановку, чаты остаются на месте.
- Новый чат (⌘/Ctrl+N): папка, модель, effort (`low/medium/high/xhigh/max`, дефолт
  `medium`), permission mode, аккаунт, чекбокс **Git worktree** (по умолчанию включён →
  `claude --worktree`).
- Аккаунты: панель в статус-баре. «Add» создаёт `Documents/claude-accounts/<name>`,
  «✕» удаляет папку. Сессия чата стартует с `CLAUDE_CONFIG_DIR=<папка аккаунта>`.
- Метка чата: ★ в начале строки, ставится и снимается кликом. Смысл — какой сам
  придумаешь, приложение её нигде не читает; на непомеченных строках она появляется
  только под курсором.
- Живые лимиты аккаунтов (5h / week / model-weekly + время сброса) — без расхода токенов,
  через OAuth usage-эндпоинт; обновление раз в минуту.
- Закрытие окна прячет приложение в трей, сессии продолжают работать в фоне.
  Выход — через меню трея (Quit).
- Темы light/dark/system.

## Разработка

```sh
pnpm install
pnpm dev          # только фронт в браузере (tauri-команды — no-op)
pnpm tauri dev    # полное приложение (нужен Rust-тулчейн)
```

## Проверка Rust-части без локального тулчейна (Docker)

```powershell
docker run --rm -v "${PWD}:/app" -w /app/src-tauri rust:1-bookworm bash -c `
  "apt-get update -qq && apt-get install -y -qq libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev pkg-config build-essential libxdo-dev >/dev/null && cargo check"
```

## Windows-сборка через Docker

```powershell
./build-windows.ps1
```

Собирает toolchain-образ (rust + cargo-xwin + NSIS + node) и кросс-компилирует под
`x86_64-pc-windows-msvc`:

- портативный exe — `src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe`
  (самодостаточен, нужен только системный WebView2);
- NSIS-инсталлятор — `...\release\bundle\nsis\Luna_*_x64-setup.exe`.

Кэши (node_modules, Windows SDK, cargo registry) живут в named volumes — повторные
сборки быстрые.

## Релизы

```powershell
./release.ps1              # выпустить версию из tauri.conf.json
./release.ps1 -Notes "..." # со своим текстом релиза
./release.ps1 -DryRun      # собрать и составить манифест, ничего не заливая
```

Версия поднимается правкой `src-tauri/tauri.conf.json` и `package.json` вместе; скрипт
откажется работать, если они расходятся, если дерево грязное, если HEAD впереди
`origin/master` или если тег уже есть — релиз обязан воспроизводиться из запушенного
коммита, иначе штамп билда внутри бинаря ничего не стоит.

Скрипт собирает подписанный инсталлятор, пишет `latest.json` для автообновления и
заливает портативный exe, setup, его `.sig` и манифест в релиз `v<версия>`.

## Автообновление и подпись

- Приложение при старте проверяет `latest.json` из GitHub Releases (эндпоинт в
  `src-tauri/tauri.conf.json` → `plugins.updater.endpoints`).
- Ключ подписи апдейтов — `C:\Users\Nikita\claude-accounts\llm-desktop-updater.key`:
  приватный, без пароля, береги его; `.pub` уже вшит в конфиг. Имя файла осталось с
  прежнего бренда — переименуешь, поправь путь в `build-windows.ps1`. Для подписанной
  сборки задай в окружении сборки `TAURI_SIGNING_PRIVATE_KEY_PATH` — тогда рядом с
  бандлом появятся `.sig`-файлы, их и `latest.json` заливаешь в Release.
- Authenticode-подпись exe (чтобы SmartScreen не ругался) требует сертификат: когда
  будет, добавь в `tauri.conf.json` → `bundle.windows.signCommand`.
