# t3-theme

Custom CSS is retained through theme imports, exports, saved and environment themes.
Imports enforce the TypeScript limit of 20,000 UTF-16 units; invalid stored CSS is ignored.
`css::theme_css_loads_remote_files` provides publication feedback. Browser hosts must
parse with CSSOM before applying `theme_css_value_fetches` or `strip_fetches`, then replace
or clear their adopted sheet when selection changes. Native token resolution does not
interpret browser selectors; mobile preserves the payload without applying CSS.
`node tools/extract-theme-css.mjs --check` verifies the generated upstream guide and hooks.

T3 Code's design system as plain data and pure functions, independent of any UI
framework. It compiles to `wasm32-unknown-unknown` (no I/O, threads or clocks) and replaces
the hand-copied subset in `t3-ui/src/tokens.rs` as the source of truth.

```
src/color        CSS colour model: parse, convert, gamut, color-mix, canonical OKLCH
src/tokens       every custom property of index.css + Tailwind's theme, and the cascade
  expr, cascade  var() / calc() / color-mix() / light-dark() evaluation; layers, specificity, inheritance
  surface        the --diffs-* values of diff and file containers
  effects        typed shadows and the glass / backdrop utilities
src/palette      roles, built-in themes, vivid generator, theme files, saved library, preferences
src/import       VS Code theme import (JSONC), Open VSX search and verified import, ZIP reader
src/syntax       Highlights palettes (pierre-dark, t3-light, pierre-light) and the t3-light derivation
src/fonts        font stacks, how font settings resolve, availability / monospace probes
src/motion       panel animation rules, sidebar list animation planner, "nothing repaints continuously"
src/resolved     ResolvedTheme: every token for an appearance, plus a JSON export
src/ui_data      project icon colours, theme preview tiles, file tree variables
src/css          publication/value fetch guards, normalized rule filtering, upstream guide/hooks
```

## Where the data comes from

Nothing is copied by hand. Scripts under `tools/` (Node 22+, run inside the monorepo with
dependencies installed) read the TypeScript and CSS sources and write `fixtures/`:

| Command | Writes | Kind |
| --- | --- | --- |
| `node tools/extract-palettes.mjs` | `fixtures/palettes.json`, `src/palette/roles.rs` | derived (role list, built-in themes) |
| `node tools/extract-tokens.mjs` | `fixtures/tokens.json` | derived (all token declarations) |
| `node tools/extract-syntax.mjs` | `fixtures/syntax.json` | derived (Highlights palettes) |
| `node tools/extract-misc.mjs` | `fixtures/misc.json` | derived (icons, previews, tree CSS) |
| `node tools/sources.mjs` | `fixtures/sources.json` | hashes of every upstream file |
| `node tools/color-oracle.mjs` | `fixtures/color-corpus.json` | oracle: culori and Chrome |
| `node tools/palette-oracle.mjs` | `fixtures/palette-oracle.json` | oracle: the real `themePalette.ts` |
| `node tools/import-oracle.mjs` | `fixtures/import-oracle.json` | oracle: the real importers |
| `node tools/chrome-cascade.mjs` | `fixtures/chrome-tokens.json` | oracle: Chrome running the real `index.css` |

`node tools/regenerate.mjs` runs all of them (`--check` verifies the derived ones). The TypeScript
runs unmodified under Node: `tools/ts-loader.mjs` only resolves extensionless imports and stubs
the atom registry. `fixtures/resolved-default-themes.json` is the one fixture the Rust code
writes (`T3_THEME_UPDATE_FIXTURES=1 cargo test -p t3-theme`); the cascade it exports is verified
against Chrome by `cascade_chrome`.

**Drift:** `tests/tokens_api.rs::sources_have_not_drifted` hashes every upstream file listed in
`fixtures/sources.json`. Changing `index.css`, a palette, an importer, the diff CSS, the settings
contract or the mobile theme files without regenerating fails that test.

## Tokens

668 token names, 1,114 declarations (Tailwind default theme + `index.css`), 19 keyframes,
18 `@utility` bodies, 4 diff-surface rules.

| Category | Names | | Category | Names |
| --- | ---: | --- | --- | ---: |
| colour palette (Tailwind scale + zinc-25/white/black) | 289 | | spacing | 14 |
| semantic colours | 85 | | radius | 10 |
| `--color-*` utility aliases | 54 | | layout (containers, heights, widths) | 26 |
| stage art | 24 | | typography: family / size / weight | 5 / 23 / 9 |
| contrast colours | 14 | | typography: line height / letter spacing | 22 / 6 |
| border colours | 12 | | shadows | 20 |
| appearance (contrast percentages) | 4 | | blur / effect | 8 / 2 |
| motion (easing, animate, transition) | 16 | | breakpoints | 5 |
| z-index (`--z-sheet`) | 1 | | texture / image / other | 2 / 1 / 16 |

There are no border-width or opacity tokens in the stylesheet: borders are colours, and opacities
live in `--glass-opacity`, `--assistant-citation-highlight-opacity` and mix percentages. Other
z-indexes are literals in rules (100, 101, 120 for the theme inspector) and are listed in
`TokenTable::z_index`; the app layer order (45 drawer, 46 sheet, 47-49 preview player, 50+ dialogs)
is only documented in a comment.

### How the cascade is resolved

`index.css` declares `--background` and friends in an early `:root` block, again in a dark variant,
again inside `[data-app-sidebar]`, and finally, later and more specifically, from the `--app-theme-*`
roles under `html[data-theme-id]:not([data-theme-id=""])`. `tools/extract-tokens.mjs` records each
declaration with its selector (as structured attribute/class tests), specificity, `@layer`, source
order, `@variant` conditions (`dark`, `light`, `sm`, `wco`) and whether it came from `@theme inline`.
`tokens::compute` then orders by `!important`, layer (unlayered wins), specificity, source order;
computes `<html>` first, then the sidebar element, which inherits `<html>`'s *computed* values
unless it redeclares them; substitutes `var()` at the element that declares the property; and treats
cycles and invalid references as unset. Inline values (`--app-theme-*`, `--appearance-contrast-*`,
fonts) outrank every rule, as `style.setProperty` does. Tailwind's `reference` theme blocks emit
nothing and are skipped; `@theme inline` values resolve per element, so they are declared for both.

`cascade_chrome` loads the real `index.css` (Tailwind syntax lowered to native CSS) in Chrome 157
and compares every colour and length token in 19 page states (stock and every built-in theme in
every mode, contrast 70/130/140, blue-orange diff scheme, wide viewport, window controls overlay):
20,970 comparisons, all within two 8-bit levels for colours and exact for lengths. The diff
container's 20 `--diffs-*` values match in 8 states.

### Colour

`color::Color` keeps the CSS space a value was written in (OKLCH stays OKLCH). Conversions follow
CSS Color 4; OKLab to sRGB uses culori's matrices, the reference implementation of the TypeScript.
Checked by `color_oracle` on 1,980 strings (every named colour, hex, rgb, hsl, hwb, lab, lch, oklab,
oklch, `color()` in eight spaces, 400 `color-mix()` cases):

- against culori: OKLab, OKLCH, Lab and sRGB agree to 2e-6 (Lab 1e-4 relative);
- against Chrome's painted pixels: within one 8-bit level for opaque colours (more than 70% exact),
  within two for translucent ones.

**Gamut mapping:** Chrome does not map: it clips each sRGB channel, and for translucent colours
floors channels at zero, blends and clamps. `to_rgba8` does the same. `map_oklch_to_srgb` implements
the CSS Color 4 chroma-reduction algorithm for hosts that want it, and `palette::math` carries the
theme code's own chroma bisection (which produces the stored hex values) bit for bit.
Chrome's raster path differs from exact arithmetic by up to one level; that is the accuracy limit.

## Theme logic ported

| TypeScript | Rust | Acceptance |
| --- | --- | --- |
| `themePalette.ts` (colour maths, vivid generator, `updateThemeColorFamily`, chip solve, files, library, preferences, environment themes) | `palette` | `palette_oracle`: 1,607 colour strings, 103 generated palettes, 1,952 single-role edits, 57 chip sets, 30 theme files, 1,440 appearance resolutions, all equal to the real TS output; `palette_suite`: all 20 cases of `themePalette.test.ts` |
| `packages/shared/themePalettes.ts` | `palette::definition` + generated `roles.rs` | the 61 roles, five built-in themes, mobile ids |
| `vscodeThemeImport.ts` | `import::vscode`, `import::jsonc` | `import_oracle`: 189 workbench themes, 19 import texts, 6 pairing sets, 4 collision sets |
| `openVsxThemes.ts` | `import::openvsx`, `import::zip` | 20 packages built with JSZip (deflate, comment trick, cycles, escapes, checksums) equal to the TS result; search, unavailable, cancel and the ZIP limits ported from `openVsxThemes.test.ts` |
| `readableSyntaxTheme.ts`, `diffRendering.ts` theme names and diff CSS | `syntax`, `tokens::surface` | `syntax_oracle` (t3-light derived from pierre-light equals the one @pierre/diffs resolves), `cascade_chrome::diff_surfaces_match_chrome` |
| `appearanceFonts.ts` | `fonts` | unit tests (the two browser tests are covered by `tokens_api`: 17.5px composer, 16px touch floor, rem scaling) |
| `panelAnimations.ts`, `Sidebar.motion.ts` | `motion` | unit tests of the planner and the presence/suppression rules |
| `projectIconColors.ts`, `themePreview.ts`, `pierre-tree-theme.ts` | `ui_data` | `ui_data_oracle` (86 preview cases) |

Differences from the TypeScript, all deliberate: the saved library sits behind `ThemeStorage`;
Open VSX networking behind `OpenVsxHttp`; JSON syntax errors from native theme files report one
stable message instead of V8's; installed-font sorting is case-insensitive rather than ICU
collation; `LibraryError` replaces thrown `Error`s with identical messages.

**Not ported:** the inline boot script in `apps/web/index.html` (and `themeBoot.test.ts`): a
browser-only pre-mount splash that duplicates the palette data and calls `CSS.supports`; a Rust UI
resolves with `ThemeCatalog::resolve_appearance` and `resolve_theme` before it paints. The mobile `--color-*`
variable derivation (see below) is described, not ported.

## Motion

`motion::looping_animations` classifies every looping `--animate-*` token, and
`motion::looping_animation_uses` every `animation` the stylesheet applies (utilities and rules), as
`Finite`, `Stepped` (loops only through stepped timing or holds) or `Continuous`. The app's own
tokens (`skeleton`, `status-pulse`, `status-ping`) and utilities (`live-tool-shine`,
`live-activity-focus`, `live-activity-focus-counter`, all `steps(30)`) are stepped, and the
reduced-motion media rules switch them off. The only smooth loop is Tailwind's `animate-spin`
(bound by `visible-animate-spin`, whose play state follows `--visible-animation-state` and pauses
offscreen); components also use the plain `animate-spin` class for loading icons, which is a
component decision outside this crate. `tokens_api` enforces: every loop is stepped, or smooth and
visibility-gated.

## Web and mobile differ here

The rewrite has one theme system; these are the places the TypeScript mobile app diverges from
web/desktop (`apps/mobile/src/lib/mobileTheme*.ts`, `features/terminal/terminalTheme.ts`):

1. **Theme set.** Mobile adds `t3-code` (the stock palette as literal hex, `MOBILE_DEFAULT_THEME_ID`)
   and `material-you` (Android dynamic colour, which reuses the default variables). Web has no
   such ids; its stock look is "no theme id" painted by `index.css`.
2. **Stock palette values.** Mobile uses `T3_CODE_*_THEME_COLORS` directly. Web paints its own
   values: light message bubble `#ebebed` (not `#f4f4f5`), dark canvas `#000` and bubble `#1e1e1e`
   (`standard_painted_theme_colors`).
3. **Token vocabulary.** Mobile derives ~95 Uniwind variables (`--color-screen`, `--color-card`,
   `--color-grouped-card`, `--color-thread-selected`, ...) from the roles; web binds roles to the
   shadcn-style tokens (`--background`, `--card`, ...). The roles are the shared contract.
4. **Contrast.** Web has the `--appearance-contrast-*` setting and blends foregrounds in OKLab with
   `color-mix`. Mobile has no contrast setting; it fixes text contrast with `themeColorWithContrast`
   (sRGB blend toward black/white, 12 rounded bisection steps) and `themeAccentWithContrast`
   (OKLCH lightness stepping by 0.002 with maximum chroma). Web solves lightness with
   `solveOklchLightness` (18 bisection steps on OKLCH L, hue and chroma kept).
5. **Colour conversion at the boundary.** React Native cannot take OKLCH, so mobile converts to
   sRGB with the ten-digit OKLab constants and plain clipping (no gamut mapping), and only for
   `oklch(...)` strings; other formats pass through. Web hands OKLCH to the browser, and the TS
   hex export gamut-maps by chroma bisection.
6. **Per-theme and per-platform rules.** `getMobileThemeVariables` special-cases `t3-code` and
   `t3-chat` (grouped card, prominent action colours); `platformThemeVariables` re-frames the
   drawer and cards for iOS light and Android and keeps selected rows visible (contrast 1.2).
   Web has no equivalents.
7. **Custom themes.** Web installs, edits, imports (VS Code, Open VSX), pairs and publishes themes
   and can mix a light and a dark theme (`ThemeHalves`). Mobile has built-ins only, with separate
   `lightThemeId` / `darkThemeId` preferences and a legacy `themeId`.
8. **Terminal.** Mobile builds a Ghostty config from hard-coded Pierre ANSI colours
   (`#1F1F21 #ff2e3f #0dbe4e #ffca00 #009fff #c635e4 #08c0ef #c6c6c8`), which differ from the
   palettes Highlights resolves (`#1d1d1d … #bd2e90 … #bcbcbc` light, `#171717 … #e130ac …` dark,
   see `SyntaxTheme::style.terminal_ansi`); web feeds xterm from the `--terminal-*` tokens.
9. **Text size.** Mobile scales text through `resolveTextScaleVariables(baseFontSize)`; web sets the
   root font size (rem scaling) and separate prompt/code sizes.
10. **Previews.** Both draw theme tiles from `ThemePreviewColors`, but mobile substitutes the
    standard preview colours for `t3-code` and `material-you`.

## Applying a resolved theme

```rust
use t3_theme::palette::{Appearance, ThemeCatalog};
use t3_theme::resolved::{ThemeInput, resolve_theme};

let catalog = ThemeCatalog { custom: &installed, environment: &published };
let appearance = catalog.resolve_appearance(&preference, system_dark, follow, mode, halves.as_ref());
let theme = catalog.get(&preference);                       // None = the stock look
let resolved = resolve_theme(&ThemeInput::new(theme, appearance));
```

A UI layer then, per frame it draws:

1. reads colours from `resolved.color("--background")`, `.sidebar_color(...)` inside the sidebar
   subtree, `.px("--radius")` for lengths, and `resolved.tokens.root` for everything else
   (`Value::Color` carries the CSS space; call `to_rgba8()` for the browser-identical 8-bit value);
2. highlights code with `resolved.syntax` (`t3-light` or `pierre-dark`) on `resolved.code_surface`
   (`--diffs-bg`, `--diffs-bg-addition-override`, ...);
3. colours context chips from `resolved.chips`, file icons with `ui_data::project_icon_colors`;
4. re-resolves (cheap: a few thousand evaluations) when the preference, the system appearance,
   `ThemeInput::contrast`, the font settings (`fonts::font_variables` gives `ThemeInput::fonts`)
   or the environment's published themes change; nothing mutates in place, so a hover or pressed
   state needs no theme work;
5. animates with `motion` (panels snap when `panel_animation_settings(...).active` is false; the
   sidebar list uses `SidebarListMotion`), and never starts a looping animation that is not
   `Repaint::Stepped`.

Platform shells do not branch on theme: window chrome follows `ThemeCatalog::resolve_desktop_theme`.
