# Luna brand assets

The mark, the wordmark and every app icon the project ships or may ship, in
source (SVG) and rendered form. Whatever a platform needs is taken from
here; nothing under `src-tauri/icons` is hand-made.

## Colours

| Name      | Hex       | Where                                   |
| --------- | --------- | --------------------------------------- |
| Night     | `#16152B` | icon background (top of gradient `#211F3F`) |
| Moonlight | `#F4ECD8` | the moon                                |
| Satellite | `#F0A04B` | the dot on the orbit                    |
| Orbit     | `#4A4670` | the ring (`#CFCAE3` on a light ground)  |

Wordmark: Outfit Medium (SIL OFL), outlined to paths — no font needed.

## Layout

- `svg/` — the sources. `luna-mark.svg` (mark on Night), `luna-mark-on-light.svg`,
  `luna-mark-small.svg` (no orbit; for 32 px and below), `luna-mark-mono-*.svg`,
  `luna-wordmark.svg`, `luna-logo-horizontal-{dark,light}.svg` (mark + wordmark).
- `windows/` — `app.ico` (16–256; the small sizes use the simplified mark),
  `Assets/` for an MSIX package (tiles, targetsize, unplated), `svg/` the two
  Windows-grid sources.
- `macos/` — `AppIcon.icns`, the `AppIcon.iconset` it was made from
  (`iconutil -c icns AppIcon.iconset` rebuilds it), and the 1024 SVG on the
  macOS grid.
- `ios/` — `Assets.xcassets/AppIcon.appiconset` (1024 default / dark / tinted,
  Xcode 16 single-size) and `layers/` for Icon Composer.
- `android/` — `res/` drop-in: adaptive icon (vector foreground, background,
  monochrome) plus legacy mipmaps; `play-store-512.*`.
- `web/` — `favicon.svg` / `.ico`, `apple-touch-icon.png`, PWA 192 / 512.

## What the app uses today

`src-tauri/icons/` (Tauri: window, tray, installer, exe resource):

| File          | Source                                              |
| ------------- | --------------------------------------------------- |
| `icon.ico`    | `windows/app.ico`                                   |
| `icon.png`    | `web/icon-512.png`                                  |
| `128x128.png` | `macos/AppIcon.iconset/icon_128x128.png`            |
| `32x32.png`   | `windows/Assets/Square44x44Logo.targetsize-32.png` — the simplified mark; also drawn at 16 px in the title bar (`src/app/AppFrame.tsx`) |

To change the logo, replace the files here and copy the four above again.
