# Getting started

The desktop executable is `solidcraft`, the CLI is `solidcraft-cli`, and designs are saved as
`.solidcraft`. Designs, settings and setups from builds released under the previous product name keep
working (see the [compatibility notes](https://github.com/bherbruck/solidcraft/blob/main/docs/compatibility.md)).

## Desktop

Download the installer for your system from the
[Download section of the README](https://github.com/bherbruck/solidcraft#download) or the
[releases page](https://github.com/bherbruck/solidcraft/releases), or build it yourself with
stable Rust (1.95 or newer):

```sh
git clone https://github.com/bherbruck/solidcraft && cd solidcraft
cargo run --release -p solidcraft -- --sample   # opens with a sample part
```

`solidcraft part.step` opens a STEP, IGES, 3MF, STL or OBJ file as a new design, and
`solidcraft design.solidcraft` opens a saved design. Windows builds cross-compile from Linux with
`cargo xwin build --release --target x86_64-pc-windows-msvc -p solidcraft`.

### macOS

Open the release `.dmg` and drag `SolidCraft.app` onto Applications.

When updating from a build released under the previous product name, follow the
[macOS upgrade notes](https://github.com/bherbruck/solidcraft/blob/main/docs/compatibility.md#macos) so you replace it rather than install a second copy.

### Linux AppImage

The first time you open the AppImage it adds a menu entry and icons under `~/.local/share`
(`applications/ai.storyteller.solidcraft.desktop`, `icons/hicolor`), so the desktop shows the
right name and icon. It adds nothing if [appimaged](https://github.com/probonopd/go-appimage)
already made an entry for the file, or if a launcher such as Gear Lever or AppImageLauncher
started it (and removes the entry SolidCraft added before). To
turn it off for good, create an empty file `~/.local/share/solidcraft/no-desktop-integration`, or
start the AppImage with `SOLIDCRAFT_NO_DESKTOP_INTEGRATION=1`.

### Linux Flatpak

Install Flatpak using your distribution's package manager. For an installation that receives
updates, add Flathub for the required Freedesktop runtime and the SolidCraft application feed
to the same user installation:

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak remote-add --user --if-not-exists solidcraft https://docs.solidcraft.io/flatpak/solidcraft.flatpakrepo
flatpak install --user solidcraft ai.storyteller.solidcraft
flatpak run ai.storyteller.solidcraft
```

A SolidCraft remote added before the project moved to `docs.solidcraft.io` (from
`bherbruck.github.io`) no longer finds updates. Point it at the new address, then update:
`flatpak remote-modify --user --url=https://docs.solidcraft.io/flatpak/repo/ solidcraft` and
`flatpak update --user`.

Releases up to 0.7 used the application ID `ai.storyteller.solvecraft`. If you installed one
of those from the SolidCraft remote, `flatpak update --user` reports it as end-of-life and offers
to replace it with `ai.storyteller.solidcraft`; accept, and your settings and autosaves move with
it. If you installed a `.flatpak` bundle, install the new bundle, launch it once (it copies your
settings), then run `flatpak uninstall --user ai.storyteller.solvecraft` (see the
[compatibility notes](https://github.com/bherbruck/solidcraft/blob/main/docs/compatibility.md#linux-app-id)).

Alternatively, download the release's `.flatpak` file for your architecture and run
`flatpak install --user ./solidcraft-VERSION-linux-ARCH.flatpak`. The bundle supplies the
Flathub runtime repository address; installation still needs internet access to fetch any
missing runtime. Launch with `flatpak run ai.storyteller.solidcraft`, rather than looking for
a host `solidcraft` binary. Files chosen through Open/Save use the desktop file-chooser portal;
command-line paths outside Documents may need a specific additional filesystem permission.

### Linux Snap

On Ubuntu and other distributions with snapd:

```sh
sudo snap install solidcraft
solidcraft                # or open SolidCraft from the app menu
solidcraft.cli --version  # the CLI, for scripts and `solidcraft.cli mcp`
```

The snap can open designs in your home folder (not hidden folders) and, through the file
chooser, anywhere else. For USB drives and other mounted disks, run
`sudo snap connect solidcraft:removable-media` once. A release's `.snap` file installs without
the store with `sudo snap install --dangerous ./solidcraft-VERSION-linux-ARCH.snap`; it then
doesn't update automatically.

## Web

The same app runs in the browser: [open SolidCraft on the web](../app/) (`?sample` opens the sample
part, `?webgl` forces WebGL2 where WebGPU is missing or misbehaves; the browser remembers it until
`?webgpu`, and an app installed from Chrome or Edge starts with it too). Designs are saved in the
browser's private file storage, which needs HTTPS or localhost.
Planned worker and tiered-storage integration is
described in [web-runtime.md](https://github.com/bherbruck/solidcraft/blob/main/docs/web-runtime.md)
(design; not yet wired in the shipped wasm build).

After the first visit the web app works offline: the browser keeps a copy of it. Online, each
launch gets the latest published build (the browser downloads it once when it changes); offline,
when the network doesn't answer within a few seconds, or when a new build can't be downloaded
whole, the kept copy starts (and the download carries on for the next launch).

Browsers that can install web apps add **Install app…** to the File menu: it installs SolidCraft
as an app with its own window and launcher icon, working offline like the page. In Chrome, Edge
and other Chromium browsers it shows the browser's install prompt, and once SolidCraft is
installed the item is gone. Safari has no such prompt, so the item shows its steps instead:
Share › Add to Home Screen on iPhone and iPad, File › Add to Dock on a Mac (Safari 17 or later).

To build it yourself:

```sh
rustup target add wasm32-unknown-unknown
cd apps/solidcraft-web && trunk serve --release --no-autoreload
```

Offline use needs `--no-autoreload`: Trunk's live reload changes the page as it serves it, so it
no longer matches the build the browser would keep. Debug builds (`trunk serve`) never keep a copy.

### Self-hosting with Docker

The web app is also a container image, `ghcr.io/tangency-labs/solidcraft-web`, for Linux on x86_64
and ARM64: `latest` is the latest release, `0.14.0` or `0.14` a release, `edge` the newest `main`.
It serves the app with nginx on port 8080 and keeps nothing on the server: designs stay in each
visitor's browser.

```sh
docker run -d --name solidcraft -p 8080:8080 --restart unless-stopped ghcr.io/tangency-labs/solidcraft-web:latest
```

Or with Docker Compose, using
[`packaging/docker/compose.yaml`](https://github.com/tangency-labs/solidcraft/blob/main/packaging/docker/compose.yaml)
(`SOLIDCRAFT_PORT` picks another port):

```sh
docker compose -f packaging/docker/compose.yaml up -d      # start
docker compose -f packaging/docker/compose.yaml pull && docker compose -f packaging/docker/compose.yaml up -d   # update
docker compose -f packaging/docker/compose.yaml up -d --build   # build the image from your checkout instead
```

Then open `http://localhost:8080/`. On another machine the browser only saves designs over HTTPS,
so put a reverse proxy with a certificate in front (Caddy, Traefik, nginx); the app works under
any URL path, such as `https://example.com/cad/`. To build the image without Compose, from the
repository root: `docker build -f packaging/docker/Dockerfile -t solidcraft-web .`

### 3D mouse in the browser

The web app reads a 3Dconnexion SpaceMouse or SpaceNavigator through the browser's Gamepad API,
with the same switch and speed in Preferences › Navigation as the desktop app. Edge and Chrome
read the device as it is. A browser shows a gamepad to a page only once it has been moved there,
so if nothing happens at first, push the cap firmly once. 3DxWare also turns the cap into
mouse-wheel zoom in browsers; while the cap moves, the web app ignores the wheel and reads the
cap itself. Before the browser shows the cap to the page, that zoom still gets through, so the
first push after opening the page may zoom a little.

Firefox doesn't list 3D mice ([Mozilla bug 1226575](https://bugzilla.mozilla.org/show_bug.cgi?id=1226575)).
On Windows it does list 3DxWare's virtual joystick, which can carry the cap instead. Save this
as `%APPDATA%\3Dconnexion\3DxWare\Cfg\Firefox.xml`, then restart Firefox (and 3DxWare, if the cap
still only zooms). It changes Firefox only: there 3DxWare sends the cap's six axes to the
joystick instead of the mouse wheel. Delete the file to undo it.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<AppCfg Default="false" xmlns="" CfgFormatVersion="1.3" ThisFileVersion="1.1">
  <AppInfo>
    <Signature>
      <Name>STR_FIREFOX</Name>
      <ExecutableName>FIREFOX.EXE</ExecutableName>
      <Transport>KMJ</Transport>
    </Signature>
    <Options/>
  </AppInfo>
  <CfgProperties>
    <ID>ID_STR_FIREFOX</ID>
    <Name>STR_FIREFOX</Name>
    <InheritsFromID>ID_STR_FIREFOX</InheritsFromID>
  </CfgProperties>
  <Devices>
    <Device>
      <ID>ID_Standard_3D_Mouse</ID>
      <AxisBank>
        <Name>STR_DEFAULT_BANK</Name>
        <ID>Default</ID>
        <Axis><Enabled>true</Enabled><Input><ActionID>HIDMultiAxis_X</ActionID><Min>-512</Min><Max>511</Max><Deadband>0</Deadband></Input><Output><ActionID>HIDJoystick_X</ActionID></Output></Axis>
        <Axis><Enabled>true</Enabled><Input><ActionID>HIDMultiAxis_Y</ActionID><Min>-512</Min><Max>511</Max><Deadband>0</Deadband></Input><Output><ActionID>HIDJoystick_Y</ActionID></Output></Axis>
        <Axis><Enabled>true</Enabled><Input><ActionID>HIDMultiAxis_Z</ActionID><Min>-512</Min><Max>511</Max><Deadband>0</Deadband></Input><Output><ActionID>HIDJoystick_Z</ActionID></Output></Axis>
        <Axis><Enabled>true</Enabled><Input><ActionID>HIDMultiAxis_Rx</ActionID><Min>-512</Min><Max>511</Max><Deadband>0</Deadband></Input><Output><ActionID>HIDJoystick_Rx</ActionID></Output></Axis>
        <Axis><Enabled>true</Enabled><Input><ActionID>HIDMultiAxis_Ry</ActionID><Min>-512</Min><Max>511</Max><Deadband>0</Deadband></Input><Output><ActionID>HIDJoystick_Ry</ActionID></Output></Axis>
        <Axis><Enabled>true</Enabled><Input><ActionID>HIDMultiAxis_Rz</ActionID><Min>-512</Min><Max>511</Max><Deadband>0</Deadband></Input><Output><ActionID>HIDJoystick_Rz</ActionID></Output></Axis>
      </AxisBank>
    </Device>
  </Devices>
</AppCfg>
```

## CLI

`solidcraft-cli` runs the same commands without a window:

```sh
cargo run --release -p solidcraft-cli -- commands                       # every command, as JSON
cargo run --release -p solidcraft-cli -- run examples/bracket.json --out bracket.step
cargo run --release -p solidcraft-cli -- eval examples/bracket.json     # volume, area, faces…
cargo run --release -p solidcraft-cli -- snapshot examples/bracket.json --out bracket.png
cargo run --release -p solidcraft-cli -- exec solid.box '{"length": 20, "width": 10, "height": 5}'
cargo run --release -p solidcraft-cli -- run mechanism.json --in arm.solidcraft  # with a motion.export_animation step: a GIF
```

A script is a list of commands:

```json
{"commands": [
  {"command": "sketch.create", "params": {"plane": "XY"}},
  {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [40, 30]}},
  {"command": "sketch.finish", "params": {}},
  {"command": "solid.extrude", "params": {"distance": 20}},
  {"command": "solid.fillet", "params": {"edges": [[0, 0, 10]], "radius": 3}}
]}
```

![The bracket example, rendered by solidcraft-cli snapshot](../generated/bracket.png)

`snapshot` looks at the model from the iso view. `--view NAME` takes another standard view (`front`, `back`, `top`, `bottom`, `left`, `right`) or the name of a view saved in the design with `view.save`. `--camera JSON` takes any camera, as the MCP `screenshot` tool does (its conventions are in [the MCP page](mcp.md#camera-and-close-up-recipe)). A script that applies a view itself with `view.apply` sets the camera too. The ground grid lies on Z 0, so it cuts through a part that straddles it: `--no-grid` leaves it out of the picture, and `--no-axes` the origin's axes. The picture is drawn on the GPU with the viewport's own pipelines, so it shows what the window would (a surface analysis such as `inspect.zebra` included); a machine without a GPU uses a software one (Mesa's lavapipe on Linux, WARP on Windows), and with neither (a Mac without usable Metal, a Linux box without Mesa's Vulkan) the CPU rasterizer draws it, without surface analysis colours, and the reply's `renderer` says `cpu` with a warning. `SOLIDCRAFT_GPU=auto|hardware|software|cpu` chooses (`hardware` and `software` fail rather than fall to the CPU), and `solidcraft-cli gpu` lists the adapters and the tier a snapshot would use.

```sh
solidcraft-cli snapshot part.solidcraft --out photo.png --view Photo
solidcraft-cli snapshot part.solidcraft --out close-up.png --no-grid --no-axes --camera '{"target": [30, -20, 0], "yaw": 0.6, "pitch": 0.25, "distance": 60}'
```

Windows PowerShell 5.1 drops the double quotes inside an argument it passes to a program: write them as `\"` there.

A command that fails stops the script. A feature can also fail later, when a parameter change
rebuilds it; `run` then exits with status 1, lists the failing features and writes none of
its `--out` and `--save` files, because the bodies lack what those features should have made.
`--allow-errors` writes them anyway.

`run` prints each command's result with `--results` (otherwise the measurement), and `eval`
prints the measurement, as JSON on stdout. `--json-out FILE` writes that JSON to a file
instead. The file is plain UTF-8, which is not what a redirect gives you everywhere: Windows
PowerShell's `>` re-encodes a program's output, as UTF-16 or as UTF-8 with a byte-order mark
depending on its settings, and strict JSON readers such as Python's `json.load` then refuse it.

```sh
cargo run --release -p solidcraft-cli -- run examples/bracket.json --results --json-out results.json
cargo run --release -p solidcraft-cli -- eval examples/bracket.json --json-out measures.json
```

To find the slow steps of a script, `--timings` prints how long each command took instead of
the measurement: `{"timings": [{"index", "command", "ms"}], "total_ms"}`, with `index` counted
from 0. `--profile` adds each command's `profile: {"boolean_ms", "tessellate_ms", "other_ms"}`,
its wall time in the kernel's booleans, in tessellation and in everything else. With
`--results` as well, the results go in the same object as `results`; `--results` alone still
prints a plain array.

```sh
cargo run --release -p solidcraft-cli -- run examples/bracket.json --profile
```

A design's history turns into a script that rebuilds it, which makes a bug that only shows
on one design a one-file reproduction:

```sh
cargo run --release -p solidcraft-cli -- script part.solidcraft --out repro.json
cargo run --release -p solidcraft-cli -- eval repro.json               # the same part
```

The script starts with the user parameters (`parameters.add`) and the design's units, sheet
metal and plastic rules and materials, then adds each timeline feature with `timeline.add`,
exactly as the design file holds it: same ids, names and parameter names, so later steps
still find the sketches and bodies they use, and a feature that failed in the design fails
the same way. Edit a step's values to try a change, or delete the steps after a feature to
see the design as it was there. Appearances, folders, named views and configurations are
left out (the CLI notes which), and designs with components or joints are not supported
yet. In the app and over MCP the same script comes from `timeline.export_script`.

Units are millimetres and degrees unless an expression says otherwise (`"1 in"`, `"30 deg"`),
and Z is up.

## Name and compatibility

SolidCraft was renamed from an earlier working name. Since 0.5.0 everything uses the new
name: the `solidcraft` and `solidcraft-cli` programs, `.solidcraft` designs, `SOLIDCRAFT_*`
environment variables, settings folders and the `solidcraft` MCP server. Upgrading keeps
working: designs, settings, recovery files, environment variables and CLI/MCP setups from
earlier builds carry over. Every compatibility shim is listed in the
[compatibility notes](https://github.com/bherbruck/solidcraft/blob/main/docs/compatibility.md).

Use the SolidCraft documentation and web-app URLs. GitHub redirects links to the renamed
repository; old-name Pages and Flatpak URLs are not maintained as permanent aliases.
