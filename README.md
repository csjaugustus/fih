<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/fih-horizontal-white.svg">
  <img src="images/fih-horizontal-black.svg" alt="fih" width="360">
</picture>

# why many monitor when one do trick

**fih put all window in one long strip. Slide strip. Window come, quick like fih. No second monitor. No third. Minimalmaxxing.**

</div>

---

| Many monitor | fih |
|---|---|
| Buy monitor. Buy another. Buy arm, dock, cable | One screen. Strip go forever |
| Turn neck to find window | Window come to you |
| Open window, other window shrink | Open window, nothing shrink. Strip grow |
| Unplug laptop, layout explode | Layout no care |
| Need more room, buy more monitor | Need more room, stack more strip. Free |

## How work

Window sit side by side on strip. Strip go right forever. Screen is little hole you look through. Swipe four finger or press key, strip slide. Window you want slide into hole.

Left right = slide in workspace. Up down = switch workspace, go round and round. Each workspace own strip. Is like many monitor, but cost zero.

Have laptop and monitor? Workspace on both share one number line. Keyboard go everywhere.

Screen cut in 2 or 3 column (laptop 2, monitor 3, change in menu bar). Window fill the column, fill empty space. One window = full. Close one, rest grow. Open one, rest shrink. Fih no make you drag edge. Make window full on purpose? Fih leave it full.

Need one window always there? Pin it. It hold one column at left or right. Rest of strip slide beside it.

Teal line = window you on. Orange line = pinned window. Mouse come with you when you move by keyboard.

## Strip map

Move, little map show at bottom. One icon per window. Teal = where you are. Faded = off screen. Lock = pinned. Number at left = which workspace.

Go up down, map show all workspace, one row each.

## Menu bar

Fish show workspace number. Click fish: see what open where, pick 2 or 3 column, reset width, open config.

## Get

Ready app go in [Releases](https://github.com/csjaugustus/fih/releases).

1. Open DMG. Drag fih into Applications.
2. Open fih. Mac say no, fih not notarized. Go System Settings → Privacy & Security, scroll down, click **Open Anyway**.
3. Mac ask for Accessibility. Say yes. Fih need it to move window.

Fih live in menu bar now. Start by itself at login. New version? Replace app, open again.

Go away: `/Applications/fih.app/Contents/MacOS/fih uninstall`, then app to Bin.

### If you nerd

```sh
git clone https://github.com/csjaugustus/fih.git
cd fih
cargo run                     # try
cargo install --path .        # keep
```

macOS ask for Accessibility. Say yes (System Settings → Privacy & Security). Fih need it to move window.

Run all time: `fih install && fih start`.

## How control

Hold ⌃⌥ (control + option). Add ⇧ to move window, not just look.

| Key | Do |
|---|---|
| ⌃⌥ ← → | Go to next window. On floating window: put it away, back to strip |
| ⌃⌥ ↑ ↓ | Go to next workspace with window in it |
| ⌃⌥⇧ ← → | Move window |
| ⌥ 1–9 | Go to workspace 1–9. ⇧ = take window there |
| ⌃⌥ R | One column or full |
| ⌃⌥ F | Fill screen |
| ⌃⌥ C | Center |
| ⌃⌥ P / ⌃⌥⇧ P | Pin left / right. Again = unpin |
| ⌃⌥ T | Float. Again = back in strip |
| ⌃⌥ Q | Quit |

Chat and settings app (WeChat, Telegram, System Settings, QuickTime, Affinity Publisher) float, not go in strip.

Want other key? Fih make `~/.config/fih/fih.toml` first time. Change it, fih see change by itself. Menu bar fish → **Open Config File** to find it. No know how? Ask AI.

## Where from

Fih is fork of [Paneru](https://github.com/karinushka/paneru). Paneru do hard part. Fih add pin, snap, strip map, up-and-down strip, column, auto fill, and fish.

README talk like [caveman](https://github.com/juliusbrussee/caveman). Good repo. Make AI talk less too.

MIT license.
