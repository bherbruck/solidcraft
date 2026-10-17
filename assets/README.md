# Application resources

`assets/` is the design resource home for SolidCraft.
[ATTRIBUTION.md](../ATTRIBUTION.md) is the file inventory and provenance record.
Keep one editable master per resource and place shipped derivatives in `exports/`.

```text
assets/
  icons/
    app/
      shared/source/                 # original Rust mark and executable ICO encoder
      macos/source/SolidCraft.icon/   # approved Composer project and linked SVGs
      macos/exports/                 # paired Assets.car and icon.icns
      windows/exports/               # executable DIB ICO and installer PNG ICO
      linux/exports/hicolor/         # Linux/FreeBSD package images, 16–512 px
    ui/design/svg/                   # authoritative original UI icon masters
    ui/design/tools/                 # authoring bases, checks and included review app
    ui/source/                       # Rust viewport glyphs, markers and navigation
  branding/web/source/og.png          # shared social preview card
  styles/ui/palettes.rs               # light/dark defaults
  styles/web/loading.css             # web loading/canvas styles
  appearances/library.rs             # built-in visual presets
  materials/library.rs               # physical defaults and colours
  shaders/viewport.wgsl               # original viewport shader
  fonts/{liberation-sans,biz-udp-gothic}/ # unmodified fonts and OFL licences
  locales/{en,de,ja,ko}.json          # shared offline catalogs
  platforms/{macos,linux,windows}/    # bundle, freedesktop and WiX templates
  archive/icons/macos-v1/
    source/icon.vectorcraft          # earlier editable work, retained
                                     # redundant historical exports are retired
```

## Editing and regeneration

[UI artwork](icons/ui/README.md) identifies the drawing masters and their consumers.
Read the mandatory [UI style guide](icons/ui/STYLE_GUIDE.md) before editing an icon.
Edit SVG geometry and its command/UI assignments together; inspect both themes
and small sizes. Temporary media and generated reports belong in ignored target/. Palettes and model presets are also editable Rust data.

The current macOS master is `icons/app/macos/source/SolidCraft.icon/`, including
its linked SVG layers and appearance settings. The earlier VectorCraft document
is preserved in the archive. No other native design projects were available for
the current UI set or preview card. Keep any future `.vectorcraft`, `.ai` or other
native work and linked inputs under that resource's `source/work/`, with an
attribution row; retain the editable document alongside its exports.

Regenerate all exports of the shared Windows/Linux/FreeBSD mark together:

```sh
cargo xtask app-icons
```

For macOS, use Xcode with Icon Composer support:

```sh
mkdir -p target/resources/macos-icon
xcrun actool assets/icons/app/macos/source/SolidCraft.icon \
  --compile target/resources/macos-icon --platform macosx \
  --minimum-deployment-target 11.0 --app-icon SolidCraft \
  --output-partial-info-plist target/resources/macos-icon/icon-info.plist \
  --output-format human-readable-text
cp target/resources/macos-icon/Assets.car assets/icons/app/macos/exports/Assets.car
cp target/resources/macos-icon/SolidCraft.icns assets/icons/app/macos/exports/icon.icns
```

Review and commit source changes and required exports together. Temporary renders,
tool caches and package staging belong under ignored `target/` or `dist/`.
Normal builds use the committed exports and require no design applications.
The CLI still supports arbitrary PNG/ICO exports with `solidcraft-cli icon`.

## Platform bindings

| Platform | Resources and output paths |
| --- | --- |
| macOS | Composer master and paired exports; bundle installs `Assets.car` and `SolidCraft.icns` |
| Windows executable | `solidcraft-executable.ico`: original DIB/mask representations at 16/24/32/48/64/128/256 px |
| Windows installer | `solidcraft.ico`: PNG representations at 16/20/24/32/40/48/64/128/256 px; `SOLIDCRAFT_ICONS` override retained |
| Linux, Nix, deb/rpm, tar, Flatpak, Snap | Copy canonical hicolor images; icon name stays `ai.storyteller.solidcraft` |
| AppImage | Uses the Linux 256 px image and `.DirIcon` alias |
| FreeBSD | Shares Linux hicolor images and freedesktop metadata |
| Web | HTML/Trunk and `manifest.webmanifest` stay in `apps/solidcraft-web/`; copies Japanese font/licence to `fonts/`, uses the collected CSS, and copies the Linux 32/256/512 px images to `icons/` for the favicon and installed app |
| Docs and link previews | `xtask book` copies the shared card to the existing `/solidcraft/og.png` public path |

## Code and provenance

Resources remain compiled by their owning layer through existing APIs:
`ui-egui::icons::paint` and `Tokens` at L5, model presets in L2 `doc`, and sketch
fonts at L1. State, projection, interaction, commands and GPU bindings stay in
their current modules. Embedded data uses `CARGO_MANIFEST_DIR`; application code
needs no runtime resource registry or repository lookup.

`cargo xtask assets` requires an exact attribution row for every collected file
except Markdown documentation, and continues checking file assets elsewhere in
the repository. This includes native work, Rust masters, catalogs, shaders and
platform templates. Packaging selects explicit current files and fails when
required inputs are absent. Archived artwork has no production consumers.

Dependency-owned egui fonts and vendored renderer shaders stay with their
libraries; their licences remain in `THIRD-PARTY-LICENSES.txt` and `ATTRIBUTION.md`.
System fonts, optional packs, user fonts/canvases/decals/materials and native OS
controls retain external ownership. Root licence/notice texts stay canonical.
