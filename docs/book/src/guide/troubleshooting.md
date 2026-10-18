# Troubleshooting: SolidCraft won't start

## 0.10.0 closes a few seconds after it starts (Windows)

SolidCraft 0.10.0 crashes at startup on Windows PCs that draw with Direct3D 12 and the system
shader compiler. That covers older Intel graphics, PCs without a graphics driver (the
"Microsoft Basic Display Adapter"), virtual machines and Remote Desktop sessions. **Install
0.10.1 or later**, which fixes it. Versions before 0.10.0 don't have this problem.

## Start in safe mode

Safe mode starts with default settings (your own settings stay as they are, for the next normal
start), skips the "Recover unsaved design?" offer and draws with the software renderer.

- Windows: run `solidcraft.exe --safe-mode` from a terminal in the install folder, or add
  ` --safe-mode` to the end of a shortcut's *Target*.
- macOS: `open -a SolidCraft --args --safe-mode`
- Linux: `solidcraft --safe-mode` (Flatpak: `flatpak run ai.storyteller.solidcraft --safe-mode`)

Setting `SOLIDCRAFT_SAFE_MODE=1` does the same. From 0.10.2, if two launches in a row stop
before the window opens, the next one starts in safe mode by itself. `SOLIDCRAFT_RENDERER=cpu`
uses only the software renderer and keeps your settings.

If graphics setup fails, or graphics fail within ten seconds of the first presentation,
SolidCraft retries once in safe mode in a fresh process. Interacting with the app or having
unsaved designs disables this retry. That retry ignores `WGPU_BACKEND` and saved graphics
preferences so it can use the software renderer. If that also fails, it shows the error;
it does not keep restarting. The original panic is still saved in the crash folder.
The control channel starts after this startup settling period.

## Reset the settings

SolidCraft keeps its settings and its data in the folders below. All installed versions share
them. No version changes anything outside them: no system settings, drivers or registry
entries. Quit SolidCraft, then **rename** (don't delete) the folders. SolidCraft starts with
defaults and makes new ones. Rename them back to restore your settings, or send them to us
with a bug report.

| System | Settings (`app.ron`, `graphics.json`) | Data (`recovery`, `crashes`, `control`, thumbnails) |
|---|---|---|
| Windows | `%APPDATA%\SolidCraft` | `%LOCALAPPDATA%\SolidCraft` |
| macOS | `~/Library/Application Support/SolidCraft` | (the same folder) |
| Linux | `~/.local/share/solidcraft` | (the same folder); thumbnails in `$XDG_CACHE_HOME/SolidCraft` or `/tmp/SolidCraft` |
| Linux (Flatpak) | `~/.var/app/ai.storyteller.solidcraft/data/solidcraft` | (the same folder) |

On Windows, paste `%APPDATA%` or `%LOCALAPPDATA%` into the Explorer address bar to open the
folder. Releases before the rename used `SolveCraft` folders next to these; SolidCraft only
reads them, so you can leave them alone.

- The **`recovery`** folder holds autosaves of designs that were never saved. Rename the data
  folder rather than deleting it, so you can still open those autosaves (`.solidcraft` files)
  with File > Open.
- The **`crashes`** folder holds crash reports. Please attach the newest one to a bug report.

From 0.10.2, SolidCraft also checks the settings files at start. A file it can't read is renamed
to `app.ron.bad-<time>` or `graphics.json.bad-<time>`, and SolidCraft starts with defaults
instead.

## "No graphics adapter" or nothing happens at all

SolidCraft needs Direct3D 12 or Vulkan on Windows, Metal on macOS, and Vulkan or OpenGL on
Linux. Without a graphics driver it falls back to the system's software renderer (WARP on
Windows, llvmpipe or lavapipe on Linux), which is slow but works. From 0.10.2, if no window can
be opened at all, SolidCraft tries the software renderer once and otherwise shows a message
saying what failed (earlier versions closed without a word). Things to try:

- Install the graphics driver from the GPU maker (Intel, AMD, NVIDIA) rather than Windows Update.
- In a virtual machine, turn on 3D acceleration. Over Remote Desktop, try it on the machine itself.
- Run `solidcraft --crash-log` to list recent crash reports, and attach the newest one to an
  issue.

## The desktop stutters or freezes while SolidCraft builds bodies

Some graphics drivers hang on the shader SolidCraft uses to build bodies on the GPU, and the
system then resets the GPU. SolidCraft already builds bodies on the CPU on the AMD FirePro
D300/D500/D700 (Mac Pro 2013). On another GPU that does this, start SolidCraft with
`SOLIDCRAFT_CPU_TESS=1` set to build bodies on the CPU, and open an issue with the GPU's name.

## SolidCraft closes right away on Windows

A graphics driver can crash the app while it starts, before SolidCraft can show anything (an
old Intel Vulkan driver, `igvk64.dll`, did). Windows' Event Viewer names the driver under
*Windows Logs › Application* ("Faulting module name").

From 0.11.1, SolidCraft on Windows starts Direct3D 12 first and loads no other graphics API
when Direct3D 12 has a graphics card; Vulkan and then OpenGL are tried only on a computer
without one. Earlier versions started Vulkan, Direct3D 12 and OpenGL together and preferred
Vulkan. To choose the graphics API yourself, in any version, open a Command Prompt and start
SolidCraft from it:

```
set WGPU_BACKEND=dx12
"C:\Program Files\SolidCraft\solidcraft.exe"
```

`vulkan` and `gl` are the other choices. `setx WGPU_BACKEND dx12` keeps it for apps started
from the Start menu after you sign in again. From 0.11.1 the setting can also live in
`graphics.json` in the settings folder, as `{"msaa": 4, "backend": "dx12"}` (`auto` is the
order above). Safe mode ignores it and always uses the software renderer.

## A design saved by 0.10 won't open in an older version

Designs saved by a newer SolidCraft use a newer file format. Older versions refuse them with
"this design was saved by a newer SolidCraft" and don't change the file. Open them with the
current version.
