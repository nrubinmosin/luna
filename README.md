# llm-desktop

Тонкая desktop-обёртка (Rust + Tauri) над Claude Code CLI: несколько сессий в панелях,
папки-проекты, аккаунты с изолированными конфигами. Использует **глобально установленный**
бинарник `claude`.

Подробности по устройству — в [ARCHITECTURE.md](ARCHITECTURE.md).

## Возможности

- Сетка 1/2/3/4 панелей, drag&drop чатов из сайдбара (⌘/Ctrl+1..4).
- Новый чат (⌘/Ctrl+N): папка, модель, effort (`low/medium/high/xhigh/max`, дефолт `medium`),
  permission mode, аккаунт, чекбокс **Git worktree** (по умолчанию включён → `claude --worktree`).
- Аккаунты: панель в статус-баре. «Add» создаёт `Documents/claude-accounts/<name>`,
  «✕» удаляет папку. Сессия чата стартует с `CLAUDE_CONFIG_DIR=<папка аккаунта>`.
- Живые лимиты аккаунтов (5h / week / model-weekly + reset) — без расхода токенов,
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
docker run --rm -v "${PWD}:/app" -w /app/src-tauri rust:1-bookworm bash -lc `
  "apt-get update -qq && apt-get install -y -qq libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev pkg-config build-essential libxdo-dev >/dev/null && cargo check"
```

## Windows-сборка через Docker

```powershell
./build-windows.ps1
```

Собирает toolchain-образ (rust + cargo-xwin + NSIS + node) и кросс-компилирует под
`x86_64-pc-windows-msvc`:

- портативный exe — `src-tauri\target\x86_64-pc-windows-msvc\release\llm-desktop.exe`
  (самодостаточен, нужен только системный WebView2);
- NSIS-инсталлятор — `...\release\bundle\nsis\llm-desktop_*_x64-setup.exe`.

Кэши (node_modules, Windows SDK, cargo registry) живут в named volumes — повторные
сборки быстрые.

## Автообновление и подпись

- Приложение при старте проверяет `latest.json` из GitHub Releases (эндпоинт в
  `src-tauri/tauri.conf.json` → `plugins.updater.endpoints`; замени `OWNER` на свой
  аккаунт/орг, когда появится репозиторий на GitHub).
- Ключ подписи апдейтов: `C:\Users\Nikita\claude-accounts\llm-desktop-updater.key`
  (приватный, без пароля — береги; `.pub` уже вшит в конфиг). Для подписанной
  сборки задай в окружении сборки `TAURI_SIGNING_PRIVATE_KEY_PATH` — тогда рядом с
  бандлом появятся `.sig`-файлы, их и `latest.json` заливаешь в Release.
- Authenticode-подпись exe (чтобы SmartScreen не ругался) требует сертификат:
  когда будет, добавь в `tauri.conf.json` → `bundle.windows.signCommand`.
