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

Fih not only go side to side. Fih go up and down too. Strip stack on strip, one per workspace: code strip, chat strip, play strip. ⌃⌥ ↑ ↓ jump to next strip with window in it. Empty one? Fih skip. No new one made by accident. Is like many monitor, but monitor stack in air and cost zero.

Need one window always there? Pin it. It hold left or right half. Rest of strip slide in other half.

One window, fih make it fill screen. Open second, both take half. Fih no make you drag edge.

## Strip map

Move, little map show at bottom. One icon per window. Teal = where you are. Faded = off screen. Lock = pinned. Number at left = which workspace.

Go up or down, map grow tall: every strip stack in map, one row each, yours glow teal. You see whole tower at once, then map fade away. Click icon in map, fih take you there.

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

### What that means?

No worry. Give to AI agent (Claude Code, Codex, any):

```text
Install fih for me from https://github.com/csjaugustus/fih. Install Rust with
rustup if I don't have it, build and install fih with cargo, then start it and
walk me through giving it Accessibility permission.
```

## How control

Hold ⌃⌥ (control + option). Add ⇧ to move window, not just look.

| Key | Do |
|---|---|
| ⌃⌥ ← → | Go to next window |
| ⌃⌥ ↑ ↓ | Go to next workspace with window in it |
| ⌃⌥⇧ ← → | Move window |
| ⌥ 1–9 | Go to workspace 1–9. ⇧ = take window there |
| ⌃⌥ R | Half or full |
| ⌃⌥ F | Fill screen |
| ⌃⌥ C | Center |
| ⌃⌥ P / ⌃⌥⇧ P | Pin left / right. Again = unpin |
| ⌃⌥ T | Float. Again = back in strip |
| ⌃⌥ Q | Quit |

Chat and settings app (WeChat, Telegram, System Settings, QuickTime, Affinity Publisher) float, not go in strip.

Want other key? Fih make `~/.config/fih/fih.toml` first time. Change it, fih see change by itself. Menu bar fish → **Open Config File** to find it. No know how? Ask AI.

## Where from

Fih is fork of [Paneru](https://github.com/karinushka/paneru). Paneru do hard part. Fih add pin, snap, strip map, up-and-down strip, auto fill, and fish.

README talk like [caveman](https://github.com/juliusbrussee/caveman). Good repo. Make AI talk less too.

MIT license.
