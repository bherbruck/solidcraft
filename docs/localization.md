# UI localization

SolidCraft embeds its translations and works offline. English, Japanese, German, French, Galician, Korean, Malay,
Brazilian Portuguese, Russian, Ukrainian, Turkish and Chinese are currently registered in `crates/ui-egui/src/i18n.rs`. Adding a language requires an
embedded catalog and one registration there; the preferences selector and validation use the same
registry.

## Messages

`assets/locales/en.json` defines stable message IDs and English fallback text.
Keep IDs when changing wording. Other catalogs use the same IDs; missing messages fall back
to English. Use `app.tr("ui.preferences")` for static messages and
`Locale::format` for full sentences with named placeholders. Translate the entire sentence,
including punctuation, so a translator can reorder its arguments. Placeholder values are
inserted literally, even if a document name contains braces.

The English-label adapter (`app.tr_label`) supports existing engine and UI tables. It maps
display text to a message ID, then uses the same catalog lookup. New UI messages should use
stable IDs directly. The adapter does not change command IDs, model names, or scenario handles.
When a table's English wording changes, update the corresponding English catalog entry too.
Generated browser labels use named templates while their underlying model names stay intact.

Catalogs are parsed once and looked up through ordered maps. This is offline localization,
not a claim of zero runtime overhead. The catalog tests check registered locale IDs,
translation keys, and placeholder consistency. Run `cargo xtask ci` before submitting changes.

## Coverage

A message a locale never received falls back to English silently, so a gap shows up only as a row
that looks untranslated. Two guards catch that in the build:

- `every_complete_catalog_carries_every_english_message` — adding a key to `en.json`
  without giving it to a complete catalog fails the tests. English is the source catalog,
  Japanese, Malay and Chinese are delivered complete; German, Galician, Korean, Brazilian Portuguese, Russian, French, Ukrainian and
  Turkish are partial — their gaps fall back to English and are reported by `cargo xtask i18n` without
  failing the build. A new label needs en, ja, ms, zh, de and ko; Russian, French and Ukrainian may
  follow later.
- `every_toolbar_tab_and_panel_is_in_the_english_catalog` — tabs and panels reach the UI as bare
  source strings, resolved only by the English-label adapter, so a heading missing from `en.json`
  can never translate.

Command labels are the same case on a larger scale: every label in the engine's registry now has
an entry under `cmd.<command id>`, and `every_command_label_is_in_the_english_catalog` fails if a
command is added without one.

The context menu and the browser are the same case again, and they carry the most labels of all:
`every_menu_label_is_in_the_english_catalog` opens every menu the app can reach, flips the state
each conditional label branches on (origin shown, bodies locked, bodies hidden) so both branches
are seen, and fails on any label the catalog has never seen. Unit symbols are left out of the
sweep: `mm` is `mm` in every language, so they are not prose to translate. The sweep also asserts
it keeps reaching a broad set of labels, so it cannot pass by finding nothing.

`cargo xtask i18n` prints the same picture across every locale — which messages each one is
missing and which command labels English cannot translate. `--check` turns it into a non-zero exit
for CI. `--add-missing` writes each uncovered label into `en.json` under `cmd.<command id>`, with
the English label as its text, so the label adapter can find it and only the translations are left
to do. One key per distinct label: two commands that share a label (`Trim`, `Extend`) must not both
claim it, or the English-text lookup would silently pick one.

## Rendering and coverage

One immutable `Locale` type handles all translations. Table widgets without an app reference
use a render scope that restores the previous locale when it exits, including nested renders.
Standalone translated panels enter the same scope. Measure rendered text to size custom
widgets; UTF-8 byte lengths do not describe their displayed width.

The desktop host tries system CJK fonts across Windows, macOS and Linux, as well as an optional
`CRAFT_FONTS_DIR` font pack, and skips files that cannot be parsed. Chinese uses its own fallback
chain after the general CJK font: many Japanese CJK fonts (Hiragino Gothic, Meiryo, Yu Gothic)
match the CJK font search but lack simplified characters, so a second fallback found by the
Chinese font-name list (`PingFang`, `Microsoft YaHei`, `Noto Sans CJK SC`, …) covers them per
glyph. No fonts are redistributed in native desktop distribution packages; native builds rely
entirely on existing operating system fonts or craft-fonts.

In the browser, `solidcraft-web` bundles a self-hosted Japanese font (`BIZUDPGothic-Regular.ttf`, licensed
under SIL OFL 1.1) within the web distribution artifacts only. The font is loaded asynchronously from the same
origin (`./fonts/BIZUDPGothic-Regular.ttf`) on-demand when the interface language is a CJK one, or, with
any other interface language, the first time Japanese text (kana, kanji or full-width forms in a design name,
file name, body name or typed text) is drawn, without contacting any external CDNs or third-party servers.
A session that never shows such text downloads no font. If font loading fails with a CJK interface, the web
interface shows English for that session only to prevent missing glyphs (tofu); the saved language
preference is kept, so the next visit tries again. The bundled font specifically targets Japanese; it also
renders the kanji Chinese shares with Japanese, while Korean (Hangul) and simplified-only Chinese characters
are not covered, so a self-hosted SC/Korean font bundle remains follow-up work for full web coverage.

The current messages support named substitution, not locale-specific plural selection,
number/date formatting, or right-to-left layout and shaping. Add those capabilities when
introducing messages or languages that need them; use an established message formatter for
plural rules instead of adding language branches to panels. The catalogs cover the UI surfaces
introduced by this PR, not every engine error or every newer command dialog.
