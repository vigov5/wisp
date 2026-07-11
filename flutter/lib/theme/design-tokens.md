# Wisp design tokens

Single source of truth for the app's visual language. Native (Flutter) reads
these from `wisp_theme.dart`; the browser receiver mirrors the same values in
`web/style.css` (CSS custom properties). Change a token here and in both
renderers so all three platforms — Android, desktop, web — stay one app.

> This file is documentation, not code. The authoritative values live in
> `flutter/lib/theme/wisp_theme.dart`. It is intentionally **not** part of the
> published `docs/` Pages site.

## Colors

Neutrals flip between light and dark via the `WispColors` `ThemeExtension`
(read through `context.wc.<field>`). Accents are compile-time `const` and stay
constant across both themes.

### Neutrals (`context.wc.*`)

| Token | Light | Dark | Use |
|---|---|---|---|
| `bg` | `#F3F4F4` | `#0F1213` | app / scaffold background |
| `surface` | `#FFFFFF` | `#171A1B` | cards, tiles, fields |
| `surface2` | `#FAFBFB` | `#1D2122` | nested / secondary panels |
| `fill` | `#EEF0F0` | `#23282A` | inert chips / wells |
| `border` | `#DDE2E3` | `#2E3436` | hairlines, dividers |
| `ink` | `#141414` | `#ECEEEE` | primary text |
| `muted` | `#8A8A8A` | `#9AA1A2` | secondary text / icons |
| `subtle` | `#BBBBBB` | `#64696B` | hints / disabled |
| `codeBg` | `#191919` | `#0A0C0D` | mono / code surfaces |
| `accentFg` | `#0891B2` | `#06B6D4` | accent used as text/icon on a surface |

### Accents & semantics (`const`, constant across themes)

| Token | Hex | Use |
|---|---|---|
| `kAccentCyan` | `#06B6D4` | bright cyan — inline text actions |
| `kAccentCyanStrong` | `#0891B2` | strong cyan — filled primary CTA |
| `kAccentCyanHover` / `Pressed` | `#06B6D4` @ 12% / 20% | overlay states |
| `kAccentWarm` | `#F2E7BA` | warm secondary surfaces |
| `kAccentDirect` | `#4DA372` | direct-connection green |
| `kAccentRelay` | `#C78F2A` | relay-connection amber |
| `kDanger` | `#B34A4A` | destructive actions (Decline / Cancel / Delete) |
| `kError` | `#CC3333` | invalid input / error borders |

> The **two-cyan rule**: a *filled* CTA uses `kAccentCyanStrong`; an *inline
> text* action uses the brighter `kAccentCyan`. Don't mix them.

Known follow-up leaks to fold into tokens over time (still raw hex at some call
sites): the status greens (`#49B36C`/`#5E9B70`/`#1F7A57`), warn ambers
(`#C0912C`/`#D4A824`), and the warm-banner set (`#FFF6E5`/`#E0B96A`/`#6B4D14`).

## Typography

Bundled `Noto Sans` (UI) and `Noto Sans Mono` (codes/IDs) via `wispSans()` /
`wispMono()`. The web falls back to a system stack when Noto isn't installed.

| Role | Size | Weight | Tracking |
|---|---|---|---|
| headlineLarge | 30 | 700 | -0.8 |
| headlineMedium | 22 | 700 | -0.5 |
| titleLarge | 17 | 600 | -0.2 |
| titleMedium | 14 | 600 | — |
| bodyLarge | 14 | 400 | — |
| bodyMedium | 13 | 400 | — |
| labelLarge | 13 | 500 | — |
| labelMedium | 11.5 | 500 | 0.1 |

## Spacing (`WispSpace`)

A 4-based spine plus fine adjustments. Prefer these named steps over bare
literals in new/edited code; the web mirrors them as `--space-*`.

| Name | px | Name | px |
|---|---|---|---|
| `hair` | 2 | `lg` | 16 |
| `xs` | 4 | `xl` | 20 |
| `tiny` | 6 | `xxl` | 24 |
| `sm` | 8 | `xxxl` | 32 |
| `md` | 12 (most common) | | |

Off-grid one-offs (`10`, `14`, `18`, …) still exist at many call sites; fold
them into the nearest step during incremental cleanup.

## Radius (`WispRadius`)

| Name | px | Use |
|---|---|---|
| `sm` | 6 | chips, small dialogs |
| `control` | 8 | buttons, inputs |
| `card` | 12 | cards, panels |
| `surface` | 16 | large surfaces, sheets |
| `sheet` | 24 | bottom sheets, drop zones |
| `pill` | 999 | pills / badges |

`14` is a de-facto duplicate of `12`/`16` and should collapse into one of them.

## Buttons — four styles

See the conventions block in `wisp_theme.dart` for the authoritative recipe.

1. **Primary** — dominant CTA (Accept / Send / Done): filled `kAccentCyanStrong`,
   white text.
2. **Soft-tint** — secondary/destructive that still reads as tappable:
   `foreground = <accent>`, `background = <accent> @ 8%`, `side = <accent> @ 15%`.
   Destructive uses `kDanger`; affirmative-secondary uses `kAccentCyan`.
3. **Neutral outline** — low-emphasis escape hatch: `ink` text, `border` side,
   no fill.
4. **Inline text action** — low-chrome action by a title: `kAccentCyan`, 13/500.

The web receiver implements these as `.btn-primary` / `.btn-danger` / `.btn-soft`
in `web/style.css`.
