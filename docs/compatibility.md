# Compatibility with the SolveCraft name

SolidCraft was first called SolveCraft, and builds up to 0.4 kept that name in their
identifiers after the displayed name changed. 0.5.0 renamed the binaries, release assets and
packages; the release after it renames everything else: crates `solidcraft-*`, binaries `solidcraft` and `solidcraft-cli`, `.solidcraft`
designs, the `"format": "solidcraft/N"` tag, `SOLIDCRAFT_*` environment variables, settings
and recovery folders, the MCP server name, docs, CI and packaging.

The old name survives **only** in the shims below (plus their tests, this file, and changelog /
release notes). `git grep -i solvecraft` must list nothing else. When you add or remove a
shim, update this file in the same PR.

## 1. `.solvecraft` design files

New designs are saved as `.solidcraft`. Files with the old extension still open everywhere:

| Path in | Where |
|---|---|
| Extension → design format | `crates/io/src/lib.rs` (`LEGACY_DESIGN_EXT`, format detection) |
| Open dialog filters (desktop) | `apps/solidcraft/src/main.rs` (`services`) |
| Drag-drop, recent files, `doc.open`, MCP `open`, CLI `eval`/`run` | go through the same format detection in `solidcraft_io` |
| Browser file picker | `apps/solidcraft-web/src/files.rs` (`accept`) |
| Windows | `assets/platforms/windows/solidcraft.wxs`: the `.solvecraft` extension on the design ProgId and in `SupportedTypes` |
| macOS | `assets/platforms/macos/Info.plist.in`: `solvecraft` in the design document type's extensions |
| Linux / FreeBSD / Flatpak | `assets/platforms/linux/ai.storyteller.solidcraft.mime.xml`: `*.solvecraft` glob and the `application/x-solvecraft` alias |

Recent-files entries that point at a `.solvecraft` file keep working: they are plain paths and
open through the same detection. The browser app also lists designs it stored under the old
extension, and keyboard-shortcut files exported as `solvecraft-shortcuts/1` still import
(`crates/ui-egui/src/toolbar_custom.rs`). Tests: `crates/io` (`Format::from_name`,
`is_design_path`), `legacy_extension_and_format_tag_still_open` (CLI, `run_results.rs`) and
`legacy_design_files_and_resource_uris` (MCP `open`).

## 2. The `solvecraft/N` format tag

Designs record their format as `"format": "solidcraft/N"`. Files saved by builds up to 0.5 say
`"format": "solvecraft/N"`; `crates/doc/src/format.rs` (`version_of`) reads both prefixes and
upgrades them as usual (the file is re-tagged `solidcraft/N` when saved).

Not backward-compatible: builds up to 0.5 don't know the new tag and refuse files saved by
later builds. Test: `version_of` accepts both prefixes; legacy-tagged designs load and are
re-tagged on save.

## 3. `SOLVECRAFT_*` environment variables

Every runtime variable is read through one helper, `solidcraft_geom::env` (`var`, `var_os`):
it reads `SOLIDCRAFT_<NAME>` and falls back to `SOLVECRAFT_<NAME>` when the new one is unset.
The shell entry points do the same: `packaging/linux/AppRun`
(`SOLIDCRAFT_NO_DESKTOP_INTEGRATION`) and `packaging/env.sh` (`SOLIDCRAFT_VERSION`,
`SOLIDCRAFT_BUILD_DATE`). Tests: the helper's unit tests (new wins, old is the fallback,
neither → unset).

## 4. Settings, preferences, autosave and recovery folders

New folders use the SolidCraft name:

| | Linux | macOS | Windows |
|---|---|---|---|
| Preferences, graphics settings | `~/.local/share/solidcraft` | `~/Library/Application Support/SolidCraft` | `%APPDATA%\SolidCraft\data` |
| Recovery (autosave) | `…/solidcraft/recovery` | `…/SolidCraft/recovery` | `%LOCALAPPDATA%\SolidCraft\recovery` |

On first launch, if the new folder doesn't exist but the old one (`solvecraft` / `SolveCraft`)
does, it is **copied** to the new location; the old folder is never deleted or changed. The
window and UI state that eframe kept under `ai.storyteller.solvecraft` is copied the same way.
Autosaves copied from the old folder (`<id>.solvecraft`) are listed and recovered like new ones.
The preferences key in eframe's storage is `solidcraft.prefs`; `solvecraft.prefs` is read when
it is missing (desktop and browser, `solidcraft_ui_egui::prefs::stored`). The browser app also
reads files and the autosave it kept under the old names in local storage and OPFS
(`apps/solidcraft-web/src/files.rs`). A 3DxWare profile the driver made for the old executable
name is read until it makes a new one (`apps/solidcraft/src/space_mouse.rs`).
In the Flatpak sandbox these folders live under `~/.var/app/<app id>/data`; for data kept under
the app id of releases before 0.8, see [Linux app ID](#linux-app-id).
The thumbnail cache is not migrated; it is rebuilt. The copy helper is
`solidcraft_io::adopt_legacy_dir`, used by the desktop app (`adopt_legacy_settings`) and
`solidcraft_engine::recovery::default_dir`. Tests: `legacy_dir` (copy once, never overwrite,
old folder intact), `first_launch_adopts_the_previous_release_settings`,
`autosave_from_a_legacy_folder_is_listed_and_loads`, `recovery_dirs_and_their_legacy_names`,
`stored_prefs_fall_back_to_the_legacy_key`.

## 5. `solvecraft-cli` alias

A tiny `solvecraft-cli` binary (`apps/solidcraft-cli/src/alias.rs`) ships in every package for
at least one release (since 0.5.0). It prints a one-line deprecation note to stderr and runs the
`solidcraft-cli` next to it with the same arguments, stdin/stdout and exit code, so scripts
and MCP client configs that call `solvecraft-cli` keep working. Test: `apps/solidcraft-cli/tests/legacy_alias.rs` (forwards `--version` and the exit status;
the note goes to stderr only, so stdout stays clean for MCP).

There is no alias for the desktop binary: launchers, file associations and Start-menu entries
installed by the packages point at `solidcraft`.

## 6. MCP

`serverInfo.name` is now `solidcraft`. The name a client shows is chosen by the user when the
server is added, so an existing setup such as

```sh
claude mcp add solvecraft -- solvecraft-cli mcp
```

keeps working (through the alias above). New setups use:

```sh
claude mcp add solidcraft -- solidcraft-cli mcp
```

Switch before the alias is removed. Resources are listed as `solidcraft://design` and
`solidcraft://commands`; the old `solvecraft://` URIs still read (`crates/mcp/src/server.rs`,
test `legacy_design_files_and_resource_uris`).

## Package IDs

| Package | Decision | Why |
|---|---|---|
| Flatpak app ID / Linux desktop ID | changed in 0.8 from `ai.storyteller.solvecraft` to `ai.storyteller.solidcraft` | The ID the ArtCraft launcher looks for (`ai.storyteller.<name>`). The `.desktop`, metainfo and MIME file names, the icon name, and the Wayland `app_id` / X11 class follow it. Existing installs are moved over; see [Linux app ID](#linux-app-id). |
| Flatpak remote file | `solidcraft.flatpakrepo`, plus a legacy `solvecraft.flatpakrepo` copy | Old links keep working; existing remotes point at the repo URL, which is unchanged. |
| macOS bundle ID | changed to `ai.storyteller.solidcraft` | No updater depends on it; the app is drag-installed. macOS may ask again on first launch (unsigned builds). |
| MSI `UpgradeCode` | **kept** | So the new MSI upgrades the old install in place. Product name is SolidCraft; the default folder becomes `Program Files\SolidCraft` (the major upgrade removes the old folder). A custom install folder isn't remembered across this one upgrade. |
| deb / rpm package name | renamed to `solidcraft`; deb `Replaces`/`Conflicts`/`Provides: solvecraft`, rpm `Obsoletes`/`Provides: solvecraft` | Installing the new package replaces the old one instead of installing side by side. |
| AppImage | `solidcraft-<version>-linux-<arch>.AppImage`; the release also carries a copy of its `.zsync` under the old `solvecraft-…` name | AppImages already installed look for the old-named `.zsync`, so AppImageUpdate keeps finding updates. |

## Linux app ID

Releases up to 0.7 used `ai.storyteller.solvecraft`; 0.8 uses `ai.storyteller.solidcraft`
everywhere (Flatpak, deb, rpm, AppImage, Snap, FreeBSD, Nix). The shims:

| Shim | Where | What it does |
|---|---|---|
| End-of-life rebase in the Flatpak repository | `packaging/flatpak-site.sh` | Each release's Pages deploy keeps the old `app/ai.storyteller.solvecraft/<arch>/stable` refs (mirrored from the previous deploy) and marks them once with `flatpak build-commit-from --end-of-life-rebase=ai.storyteller.solvecraft=ai.storyteller.solidcraft`. `flatpak update` then says the old app is end-of-life and offers to replace it with the new one; accepting installs `ai.storyteller.solidcraft`, uninstalls the old app, and Flatpak moves `~/.var/app/ai.storyteller.solvecraft` to the new id's folder (leaving a symlink at the old path) on the first launch, so settings and autosaves come along unchanged. |
| AppStream `<replaces>` / `<provides>` | `assets/platforms/linux/ai.storyteller.solidcraft.metainfo.xml.in` | Software centres treat the new id as the successor of the old one and of `ai.storyteller.solvecraft.desktop`. |
| `X-Flatpak-RenamedFrom` | `assets/platforms/linux/ai.storyteller.solidcraft.desktop` | Desktops that honour it (GNOME Shell) keep favourites and pinned launchers pointing at the renamed entry. |
| Read-only access to the old Flatpak data folder | `--filesystem=~/.var/app/ai.storyteller.solvecraft:ro` in `packaging/linux/flatpak/ai.storyteller.solidcraft.bundle.yml`; `solidcraft_io::legacy_flatpak_path`, used by `adopt_legacy_settings` and `recovery::default_dir` | When the new app was installed next to the old one (for example from a `.flatpak` bundle, which has no update feed) instead of through the rebase, its first launch copies settings and autosaves from the old app's folder, as in section 4. The old folder is not changed. |
| The old window `app_id` as eframe's legacy storage key | `adopt_legacy_settings` (`LEGACY_APP_ID`) | Window state that releases before the settings rename kept under `~/.local/share/ai.storyteller.solvecraft` is still copied (section 4). |
| Old AppImage menu entries | `packaging/linux/AppRun` | An AppImage's first run removes the `ai.storyteller.solvecraft.desktop` entry and icons that an AppImage up to 0.7 installed in `~/.local/share`, so the menu doesn't show the app twice. |

deb and rpm need no shim: the package (`solidcraft`) is unchanged, and an upgrade removes the old
`/usr/share/applications/ai.storyteller.solvecraft.desktop`, metainfo, MIME and icon files
because the new version doesn't ship them; the post-install script refreshes the desktop, MIME
and icon caches, so no stale menu entry is left. The Snap is unaffected apart from its
`common-id`: snapd names its menu entry after the snap.

Flatpak installed from a downloaded `.flatpak` bundle (no update feed): install the new bundle,
launch it once (settings and autosaves are copied), then
`flatpak uninstall --user ai.storyteller.solvecraft`.

**Removal:** keep the end-of-life refs in the Flatpak repository for at least six months after
0.8.0 (until 2027-04) and at least two minor releases; removing them strands any install that
hasn't run `flatpak update` since. The read-only filesystem grant and `legacy_flatpak_path` can go
at the same time. `<replaces>`, `<provides>` and `X-Flatpak-RenamedFrom` are harmless and can stay.
Tests: `flatpak_twin_maps_our_sandbox_data_to_the_old_app_id`,
`first_launch_adopts_the_previous_release_settings`.

## macOS

Releases before 0.4 were installed as `SolveCraft.app`. To update, quit the app, drag
`SolidCraft.app` from the DMG onto Applications, move `SolveCraft.app` to the Trash, and
replace any Dock shortcut with the old name (open `SolidCraft.app`, then
**Options > Keep in Dock**). Designs and settings are kept (see 4).

## Flatpak remotes added from the former website address

The feed has moved twice: from the SolveCraft address, then from `bherbruck.github.io/solidcraft`
when the project moved to `docs.solidcraft.io` (#1920). A remote added from either address no
longer finds updates. Point it at the current one:
`flatpak remote-modify --user --url=https://docs.solidcraft.io/flatpak/repo/ solidcraft`
(use your remote's name: `solvecraft` for one added before 0.4), then `flatpak update --user`.

## Draft design containers

The #303 draft used `solvecraft-container/1`. `crates/io/src/design.rs` accepts that
marker alongside `solidcraft-container/1`, and writes only the new marker. Optional
old previews are discarded; required document/blob validation is unchanged. The
`draft_container_marker_remains_readable` regression covers this shim.
