<p align="center">
  <img src="assets/pinglive-256.png" width="128" alt="PingLive icon">
</p>

<h1 align="center">PingLive</h1>

<p align="center">
  A tiny always-on-top ping overlay for Windows games: live latency, a rolling graph,<br>
  a timeout counter and a beep the moment your connection drops.
</p>

<p align="center">
  <a href="https://github.com/dh3niel/PingLive/actions/workflows/build.yml"><img src="https://github.com/dh3niel/PingLive/actions/workflows/build.yml/badge.svg" alt="Build"></a>
  <a href="https://github.com/dh3niel/PingLive/releases/latest"><img src="https://img.shields.io/github/v/release/dh3niel/PingLive" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-blue" alt="Windows 10 | 11">
</p>

```
 8.8.8.8                    23 ms
 avg 24  jit 2  21/48   TO 3  loss 1%
 ▁▂▁▂▃▂▁▁█▁▂▂▁▃▂▁▁▂▁▂▁▂▃▂▂▁▁▂▁▂▁▁▂▁▂
                        ↑ red bar = timeout
```

Built for Dota 2, but it works over any game or app in windowed or borderless
mode. It sits in a corner like the Discord overlay, lets clicks pass straight
through to the game, and keeps a full history so you can see *when* your
connection usually goes bad. Written in Rust, a single ~4 MB exe, negligible
CPU/GPU cost.

## Quick start

1. Download `pinglive-vX.Y.Z-windows-x64.zip` from the
   [latest release](https://github.com/dh3niel/PingLive/releases/latest) and unzip it.
2. Run `pinglive.exe` to try it; the overlay appears in the top-left corner.
   Press `Ctrl+Alt+P` to drag it, `Ctrl+Alt+D` for the dashboard.
3. To keep it running, install it as a Windows service (asks for admin once):

   ```bash
   pinglive.exe --install
   ```

   It then starts at every sign-in, restarts itself if it crashes, and shows
   up in **Settings → Apps → Installed apps**, where you can uninstall it.
   See [Install as a Windows service](#install-as-a-windows-service).

## What it does

- **Live ping** to any host or IP (default `8.8.8.8`) using Windows ICMP —
  no admin rights needed.
- **Rolling bar graph** of the last N samples, colour-coded green / yellow /
  red against your own thresholds. Timeouts are drawn as full-height red bars,
  so a packet-loss burst is visible at a glance.
- **Stats**: current, average, min/max, jitter, loss %, and a running
  **timeout counter** since start.
- **Audio alert** on a **request timeout** and whenever a ping **reaches 300 ms**
  — nothing else makes a sound. A custom `.wav` if you point at one, otherwise
  a generated descending double beep, rate-limited by `cooldown_secs`.
- **Click-through by default** — the mouse goes straight to the game.
- **Global hotkeys** work while Dota has focus.
- **Full ping history on disk** — every sample, kept for `retention_days`.
- **Dashboard** — this hour / today / last 24 h, a per-minute heatmap of the
  last 24 hours, a day × hour heatmap for 7 or 30 days, a time-of-day profile
  ("when does it usually happen") and the worst minutes. Each heatmap can show
  timeouts (RTO), lag (≥ 300 ms) or average ping; hover any cell for details.
- **Settings** in the dashboard — target, interval, alerts, sound, colours,
  history, *Start with Windows*.
- **Tray icon** — a dot coloured by the live ping, with the reading in its
  tooltip. Double-click brings a hidden overlay back; right-click for
  Dashboard / Show overlay / Settings / Mute / Exit.

## Build from source

Requires the Rust toolchain (MSVC) with a C linker:

```bash
winget install --id Rustlang.Rustup -e --scope user
```

Then, in the project folder:

```bash
cargo build --release
```

The binary lands at `target\release\pinglive.exe` (no console window).

The icon (`assets/pinglive.ico`, pixel art drawn by `scripts/make-icon.py`)
and the version info (product, publisher from `authors` in `Cargo.toml`,
version, copyright) are embedded into the exe by `build.rs`, so they show in
Explorer, the file's Properties > Details and Installed apps. After changing
the icon script, run `python scripts/make-icon.py` and rebuild.

## Run

```bash
cargo run --release -- --target 8.8.8.8
```

First launch writes a config file at `%APPDATA%\PingLive\config.toml` and
remembers the window position from then on.

CLI flags override the config for that run and are saved back:
`--target <host>`, `--interval <ms>`, `--timeout <ms>`, `--show`
(start in interactive mode so you can drag it).

## Hotkeys (global — work inside the game)

| Keys | Action |
| --- | --- |
| `Ctrl+Alt+P` | Toggle interactive mode — a blue border appears, drag the overlay anywhere, **double-click it to open the dashboard**, press again to make it click-through |
| `Ctrl+Alt+D` | Open / close the dashboard |
| `Ctrl+Alt+M` | Mute / unmute the alerts |
| `Ctrl+Alt+G` | Toggle the graph (compact one-line mode) |
| `Ctrl+Alt+H` | Hide the overlay to the tray / show it again |
| `Ctrl+Alt+R` | Reset the stats and the timeout counter |
| `Ctrl+Alt+Q` | Quit |

## Config — `%APPDATA%\PingLive\config.toml`

```toml
target = "8.8.8.8"      # host or IP; use your Dota server IP for real match ping
label = ""              # short name to display instead of the target
interval_ms = 1000      # how often to ping
timeout_ms = 1000       # no reply within this = timeout
history = 120           # samples kept in the graph
retention_days = 365    # days of raw history kept on disk, 0 = forever

[window]
x = 40.0
y = 40.0
width = 240.0
height = 104.0
opacity = 0.45          # 0.0 invisible .. 1.0 solid background
click_through = true
always_on_top = true
show_graph = true
visible = true          # false = hidden to the tray (remembered across restarts)

[alert]
enabled = true
timeout_streak = 1      # consecutive timeouts before the sound fires
cooldown_secs = 3       # minimum gap between alerts
recovery_sound = false    # no chime on recovery; beep only for the two cases below
high_ping_ms = 300      # beep as soon as a ping reaches this...
high_ping_streak = 1    # ...on the very first sample that does
sound_file = ""         # absolute path to a .wav, or empty for built-in beeps
beep_freq_hz = 880
beep_ms = 120
beep_count = 2

[colors]
good_ms = 60            # <= green
warn_ms = 120           # <= yellow, above = red
```

Restart the overlay after editing `target`, `interval_ms` or `timeout_ms`.

### A custom alert sound

```toml
[alert]
sound_file = "C:\\Users\\Administrator\\Sounds\\ping-drop.wav"
```

Must be a `.wav` (Windows `PlaySound`), and it plays asynchronously so it
never stalls the overlay. Leave it empty to use the built-in beeps, which
need no audio file and go through the motherboard/system beep path.

## Pinging the actual Dota 2 server

`8.8.8.8` tells you whether *your line* is healthy, which is what you usually
want when you are looking for the cause of a lag spike. To watch the real game
server instead, find its IP while in a match:

```bash
powershell -ExecutionPolicy Bypass -File scripts\dota-server-ip.ps1
```

then set that IP as `target`. Note that Valve's relays de-prioritise or drop
ICMP, so the number can read higher than Dota's own in-game ping — treat the
*shape* of the graph as the signal, not the absolute value.

## Ping history

Stored in `%APPDATA%\PingLive\history\YYYY-MM-DD.bin`, one file per local day,
6 bytes per ping: `u32` LE unix seconds + `u16` LE round trip in ms
(`0xFFFF` = timeout, `0xFFFE` = name did not resolve) — about 0.5 MB a day at
one ping a second. Writes are batched every 30 s. The last 31 days are folded
into per-minute buckets at startup, which is all the dashboard ever reads.
Changing the target keeps writing to the same files, so the history is
"whatever you were pinging at the time".

## Install as a Windows service

```bash
target\release\pinglive.exe --install
```

This asks for administrator rights once, then:

- copies the exe to `C:\Program Files\PingLive\pinglive.exe`;
- registers an auto-start **PingLive** service ("PingLive Ping Overlay" in
  `services.msc`) that the Service Control Manager also restarts if it fails;
- adds **PingLive** to **Settings → Apps → Installed apps** (and the old
  *Programs and Features*), with a working **Uninstall** button;
- turns off the per-user *Start with Windows* entry, since the service does
  that job now, and starts the service.

A service runs in session 0 and can never draw on your desktop, so the
service does not show the overlay itself. It supervises it: at every sign-in
it starts `pinglive.exe` in your session, as you, and restarts it if it
crashes (up to 5 times in 10 minutes). If you quit the overlay yourself
(`Ctrl+Alt+Q` or the tray) it stays closed until the next sign-in — start it
from the exe again or restart the service. Stopping the service closes the
overlay cleanly, with its history flushed.

Running `--install` again with a newer build upgrades in place. To remove it,
use **Uninstall** in Installed apps, or:

```bash
"C:\Program Files\PingLive\pinglive.exe" --uninstall
```

That stops and deletes the service, the Installed-apps entry and the Program
Files folder. Your settings and ping history in `%APPDATA%\PingLive` are kept.

### Without admin rights

Tick **Settings → Start PingLive when I sign in to Windows** (a per-user `Run`
entry), or register a logon **Scheduled Task**, which also restarts the app if
it crashes:

```bash
powershell -ExecutionPolicy Bypass -File scripts\install-autostart.ps1
```

Remove it again with:

```bash
powershell -ExecutionPolicy Bypass -File scripts\uninstall-autostart.ps1
```

The app takes a named mutex, so a second copy started by hand exits
immediately instead of stacking a second overlay on screen.

## Dota 2 display mode

Overlays can only be drawn over a game that is **Windowed** or **Borderless
Window** (Dota 2: Settings → Video → Display Mode). In exclusive **Fullscreen**,
Windows gives the game the whole swap chain and no top-most window shows
through — this is the same limitation the Discord and Steam overlays have.

## Notes

- Windows ICMP reports RTT in whole milliseconds, so sub-millisecond LAN
  latencies show as `0 ms`.
- CPU/GPU cost is negligible: vsync is off, the UI only repaints a few times
  a second, and nothing is drawn while the overlay is hidden.
- If a hotkey is already owned by another app, it is skipped with a message on
  stderr and the rest still register.
- `ViewportBuilder::with_decorations(false)` is ignored by the winit build that
  eframe 0.29 pulls in on Windows 11 - the window keeps `WS_CAPTION` and DWM
  paints a title bar over the readout. `src/win.rs` converts the window to a
  `WS_POPUP` overlay after the first frames and then nudges the size by a pixel
  and back, which is what makes the GL surface reconfigure; without that nudge
  the window renders black.

## License

[MIT](LICENSE): free to use, copy, change, share and build on, as long as
the copyright notice and license stay with the code. Provided as is, with no
warranty.
