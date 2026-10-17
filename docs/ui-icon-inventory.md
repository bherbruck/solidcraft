# UI icon maintenance

The authoritative authoring specification is
[assets/icons/ui/STYLE_GUIDE.md](../assets/icons/ui/STYLE_GUIDE.md), with mandatory
agent instructions beside it. Start at
[the UI icon README](../assets/icons/ui/README.md) for the included review app,
canonical resource paths, new-icon scaffold, exports and checks.

Launch the repository-owned review tool with:

```sh
python3 -B assets/icons/ui/design/tools/icon_status.py --serve
```

Open http://127.0.0.1:8770/. Every discovered icon without a current acceptance is
Open. SVGs, aliases and source references are scanned automatically; unused masters
are included. Counts and CSV reports are derived at runtime, not maintained here.
`--csv` writes `target/icon-inventory.csv`.

The design and cleanup plan is [ui-icon-maintenance-plan.md](ui-icon-maintenance-plan.md).
The old proposal is superseded. SVG masters, palette, aliases, the original Rust
viewport glyphs and owner review records remain the application resources.
