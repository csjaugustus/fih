<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/fih-horizontal-white.svg">
  <img src="images/fih-horizontal-black.svg" alt="fih" width="360">
</picture>

# why many monitor when one do trick

**fih put all windows in one long strip. Slide strip. Window come, quick like fih. No second monitor. No third. No cable spaghetti.**

</div>

---

| Many monitor | fih |
|---|---|
| Buy monitor. Buy another. Buy arm, dock, cable | One screen. Strip go on forever |
| Turn neck to find window | Window come to you |
| Open window, other windows shrink | Open window, nothing shrink. Strip grow |
| Unplug laptop, layout explode | Layout no care |

## How work

Windows sit side by side on strip that goes right forever. Screen is little hole you look through. Swipe four fingers, or press key, strip slide. Window you want slide into hole.

Need one window always there? Pin it. It hold left or right half. Rest of strip slide in other half.

## Get

Ready-made app go in [Releases](https://github.com/csjaugustus/fih/releases). Nothing there yet. Fih still small.

### For nerds

```sh
git clone https://github.com/csjaugustus/fih.git
cd fih
cargo run                     # try it
cargo install --path .        # keep it
cp fih.example.toml ~/.config/fih/fih.toml
```

macOS ask for Accessibility. Say yes (System Settings → Privacy & Security). fih need it to move windows.

Run all the time: `fih install && fih start`.

### What that means?

No worry. Give this to your AI agent (Claude Code, Codex, any):

```text
Install fih for me from https://github.com/csjaugustus/fih. Install Rust with
rustup if I don't have it, build and install fih with cargo, copy
fih.example.toml to ~/.config/fih/fih.toml, then start it and walk me through
giving it Accessibility permission.
```

## Keys (from `fih.example.toml`)

All keys use ⌃⌥ (control + option). Add ⇧ to move window instead of focus.

| Key | Do |
|---|---|
| ⌃⌥ ← → ↑ ↓ | Focus next window |
| ⌃⌥⇧ ← → ↑ ↓ | Move window |
| ⌥ 1–9 | Jump to window 1–9 (9 = last). ⇧ to move there |
| ⌃⌥ R | Cycle width: half, full |
| ⌃⌥ = / − | Wider / narrower |
| ⌃⌥ F | Full width |
| ⌃⌥ C | Center |
| ⌃⌥ P / ⌃⌥⇧ P | Pin window left / right. Again to unpin |
| ⌃⌥ T | Float window, or tile it again |
| ⌃⌥ N | Send window to next display (if you have one, which you no need) |
| ⌃⌥ Q | Quit |

Change anything in `~/.config/fih/fih.toml`. fih reload it by itself. Every option: [CONFIGURATION.md](./CONFIGURATION.md).

## Where from

fih is fork of [Paneru](https://github.com/karinushka/paneru). Paneru do the hard part. fih add pin, snap, step resize, and fish.

README talk like [caveman](https://github.com/juliusbrussee/caveman). Good repo. Make AI talk less too.

MIT license.
