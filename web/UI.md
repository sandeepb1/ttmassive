# UI guide

How the web app is styled and which parts to build with. Read this before adding or changing UI.

The app is vanilla TypeScript and Vite, with no UI framework. Styling is **Tailwind v4** plus a small
typed **component kit** in `src/ui/`, in the style of shadcn (the look and the semantic token names),
without React. The visual direction is **soft / iOS**: generous radii, filled surfaces instead of borders,
48px touch targets, built for the tablet and phone first.

## The rules

1. **Colours come from tokens.** Use `bg-card`, `text-muted-foreground`, `var(--fg)`. No hex literals
   (the guard test enforces it). The one place colours are defined is `src/theme.ts`.
2. **Sizes come from the scale.** Radii, control heights and text sizes are tokens (below). No
   `rounded-[9px]` or `p-[6px]`. Sizes are in `rem` so the UI-scale setting (`scale.ts`) applies.
3. **A plain button, chip, row, input, select, switch, menu or toast comes from `src/ui`.** Do not
   write `el('button', ...)` for them. A composite with its own structure and a CSS hook (a session
   row, a tab with a badge) may stay a raw element. The guard keeps a per-file count that can only go down.
4. **CSS that hides a kit element says `!important`.** See "Layers" below.
5. **New icons go through `icon(name)`** in `src/icons.ts` (Lucide, imported per icon so unused ones
   are tree-shaken). Add the name to `IconName` and the `NODES` map.

`npm test` runs `src/ui/guard.test.ts`, which checks rules 1 to 4 and fails with what to do instead.

## Tokens

`theme.ts` keeps the nine palettes and sets a few base variables on the root (`--bg`, `--fg`,
`--accent-base`, `--ok`, `--warn`, `--danger`, `--claude`, `--codex`, `--on-accent`). `ui/app.css`
derives the rest with `color-mix`, and `ui/tailwind.css` maps shadcn's vocabulary onto them:

| Tailwind colour | Meaning |
| --- | --- |
| `background` / `foreground` | the page and its text |
| `card` | a surface one step off the page (sheets, the composer) |
| `popover` | menus and toasts |
| `muted` / `secondary` | filled controls and rows, one and two steps stronger |
| `muted-foreground`, `soft`, `faint` | secondary text, from strong to weak |
| `primary` / `primary-foreground` | the action colour and the text on it |
| `accent`, `accent-text`, `accent-bg`, `accent-border` | the **brand** colour (it follows the open agent) |
| `ok` / `warn` / `destructive` / `danger-*` (+ `-text`, `-bg`) | status colours |
| `border`, `input`, `ring` | lines and focus |

`accent` here is the brand colour, **not** shadcn's hover surface. Use `bg-muted` for hovers.

Scale (`ui/tailwind.css`, `@theme static`, so plain CSS can use the variables too):

- Radius: `rounded-sm` .5rem, `-md` .75rem, `-lg` 1rem, `-xl` 1.5rem. Plain CSS: `var(--radius-md)`.
- Control height: `h-ctl-xs` 1.75rem, `h-ctl-sm` 2.25rem, `h-ctl` 3rem (also `w-`, `size-`).
- Text: `text-2xs` .6875rem, `-xs`, `-sm`, `-md` .875rem, `-base`, `-lg`. Shadow: `shadow-pop`.

## The kit (`src/ui/`)

| File | What it gives you |
| --- | --- |
| `button.ts` | `button(props, ...children)`; variants `default secondary outline ghost destructive soft-danger`, sizes `md sm icon icon-sm`. `buttonStyles()` for a link that should look like one |
| `chip.ts` | `chip` (toggle or tone), `badge` (static), `setChipOn`, `setChipTone`, `chipClass` |
| `row.ts` | `row(label, {hint, mark, current, danger})`, `setRowCurrent` |
| `input.ts` | `input`, `textarea`, `select` (native, so the platform picker is used) |
| `switch.ts`, `stepper.ts` | `toggleSwitch`; a minus / value / plus control |
| `menu.ts` | `openMenu(anchor, items)`: popover plus Floating UI. `src/menu.ts` re-exports it |
| `sheet.ts` | `sheetShell(title, onDismiss)`: the bottom sheet. The `Sheet` base class in `src/sheet.ts` uses it |
| `sheet-parts.ts` | `sheetLabel`, `sheetHint`, `sheetList`, `sheetBlock`, `sheetMsg`, `sheetActions` |
| `toast.ts` | `toast(host, message, {ms, cls, action})`; the action gets its button and a dismiss function |
| `cn.ts` | `cn()` and `tv()`: class merging that knows our custom scale names (`h-ctl`, `text-md`) |

Variants are `tailwind-variants` recipes. Add a variant to the recipe rather than passing one-off
classes, and use `cls` only for layout (`w-full`, `min-w-0`) or a hook class that CSS looks for.

Things that are easy to get wrong:

- **Sheets are not `showModal()`.** A sheet covers one pane of a split view, so it is a non-modal
  `<dialog>` positioned inside its host. The shell supplies Escape, focus return and drag-to-dismiss,
  and does not take focus on open (that would drop the on-screen keyboard). To ask "is a sheet
  open?", use `SHEET_SELECTOR`.
- **Buttons must not take focus from the terminal.** Anything pressed while the soft keyboard is up
  needs `pointerdown` `preventDefault()` (the sheet shell and the menu already do it).
- **`@floating-ui` positions menus; scrolling the page closes them.** That is deliberate.

## Stylesheets

`ui/tailwind.css` is the entry (imported by `main.ts`). It sets the layer order
`theme, base, components, utilities`, imports Tailwind's theme and reset, then the component sheets, then
utilities, and holds the token mapping and scale.

| File | Contents |
| --- | --- |
| `app.css` | the shell (sidebars, scrim, overlays), `html`/`body`, the first-paint fallback palette, the derived colour variables, login |
| `panes.css` | pane headers, the view toggle, tiles and the empty-pane picker |
| `chat.css` | messages, markdown, tool cards, diffs, the question card, suggestions, agent cards |
| `sheets.css` | new-session folder list, the file viewer, the helper-agent view |
| `sidebar.css` | session rows, group headings, status dots, tags, the split-view rail |
| `rightbar.css` | the details tabs, search, file diffs, to-dos, images, usage meters |
| `tabs.css` | the memory and Team Claude tabs, the theme picker |
| `terminal.css` | terminal overlays: progress, turn marks, chips, search panel, keybar |
| `viewer.css`, `command.css` | the image viewer; the command bar |

Rule of thumb: **controls are kit parts; structure is component CSS.** Content typography and
composite widgets (a markdown block, a tool card, a tab strip, a key-value row) are component CSS using
tokens, because turning them into long class strings in TypeScript adds risk and no uniformity. If a
composite starts to repeat, promote it to a kit part.

### Layers and `!important`

Utilities (`utilities` layer) beat component CSS (`components` layer) whatever the specificity. A kit
button carries `inline-flex`, so a component rule such as `.p-split { display: none }` **loses**, and
the button shows. Rules that hide a kit element therefore use `display: none !important`. Important
declarations beat all normal ones whatever the layer. `[hidden]` already works (Tailwind's reset marks it
important). The guard knows the hook classes of the kit buttons that CSS hides (`KIT_HOOKS` in
`guard.test.ts`); add to it when you hide another.

### iOS 26 and the top edge

An installed web app on iOS 26 gets a blur that fades the top of the page (the usage strip). No CSS turns it off.
WebKit is meant to skip it when a fixed, opaque, full-width box spans the top edge, so `index.html` has `#edge`, a fixed
strip as tall as `--safe-top`. On a real iPhone that alone did not stop the blur, so the page also stays out of it:
in an installed iOS app `--safe-top` is the status-bar inset **plus 32px** of "ramp", and `#edge` paints the whole
area in the usage strip's colour (`--surface-1`), where a blur over a flat colour is invisible. Everything that has to
clear the top (the shell, overlay sidebars, command bar, image viewer) uses `--safe-top`, not `env(safe-area-inset-top)`.
It is not reproducible in a headless browser; check on a device. Making `#app` itself fixed would also suppress the
blur but risks the soft-keyboard layout, so it was not used.

In an installed iOS app `dvh` and `innerHeight` come back short by the top inset (894 of 956 on a 16 Pro Max) even though
the page draws from the top of the screen, so `--app-h` is `100vh` there and the shell uses `var(--vvh, var(--app-h))`.
This was found by reading `innerHeight`, `visualViewport`, the viewport units and the insets off the phone (a temporary read-out, since removed); do that again if a layout looks short on a device.

The app also follows `visualViewport.height` (`--vvh`) only while the keyboard has shrunk it by more than 120px;
otherwise it uses the normal height, because an installed iOS app can report a visual height short of the screen.

Also: Tailwind generates only the utilities it finds in source, scanned across the project, so a class
must appear as a whole string in a `.ts` file (do not build `bg-${x}` at runtime).

## Checking a change

No visual-regression harness runs in CI; the scripts in `web/scripts/` take screenshots you compare by
eye or by pixel diff. They use `playwright-core` (not a dependency of the repo) and a headless Chromium.

1. Start a throwaway server on another port so the real one is untouched:
   `TTM_PASSWORD=<12+ chars> ttm serve --config /tmp/ttmt/config.toml` with `bind = "127.0.0.1:7699"`,
   a scratch `data_dir`, and a short `socket` path. It shares the `-L ttm` tmux server with the real
   one, so create no sessions you do not delete.
2. `TTM_DEV_PORT=7699 npx vite --port 5199` (the dev proxy follows `TTM_DEV_PORT`).
3. `TTM_PW=<password> node scripts/<name>.mjs <outdir>`; set `CHROMIUM` if the browser is not at the
   default path.

| Script | Shows |
| --- | --- |
| `shots.mjs` | login, app, sidebar, new session and settings at phone, tablet and desktop in two themes |
| `chat-shots.mjs`, `sheet-shots.mjs`, `rightbar-shots.mjs`, `overlay-shots.mjs` | the chat view, the chat sheets, the right-bar tabs, and the prompt bar, image viewer, command bar and terminal toast, all over a canned Claude conversation |
| `sidebar-shots.mjs` | the sidebar with needs-you, stopped, helper-agent, tagged and pinned rows |
| `chat-fixture.mjs` | the conversation itself, served by intercepting two API calls; no agent session needed |

Take a set before the change and one after, then diff them. Live text (clocks, usage percentages)
differs by a few hundredths of a percent; anything more deserves a look. Check both a light and a dark
theme, and the phone width.
