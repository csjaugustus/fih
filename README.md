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

## How work

Window sit side by side on strip. Strip go right forever. Screen is little hole you look through. Swipe four finger or press key, strip slide. Window you want slide into hole.

Need one window always there? Pin it. It hold left or right half. Rest of strip slide in other half.

## Get

Ready app go in [Releases](https://github.com/csjaugustus/fih/releases). Nothing yet. Fih still small.

### If you nerd

```sh
git clone https://github.com/csjaugustus/fih.git
cd fih
cargo run                     # try
cargo install --path .        # keep
cp fih.example.toml ~/.config/fih/fih.toml
```

macOS ask for Accessibility. Say yes (System Settings → Privacy & Security). Fih need it to move window.

Run all time: `fih install && fih start`.

### What that means?

No worry. Give to AI agent (Claude Code, Codex, any):

```text
Install fih for me from https://github.com/csjaugustus/fih. Install Rust with
rustup if I don't have it, build and install fih with cargo, copy
fih.example.toml to ~/.config/fih/fih.toml, then start it and walk me through
giving it Accessibility permission.
```

## How control

Hold ⌃⌥ (control + option). Add ⇧ to move window, not just look.

| Key | Do |
|---|---|
| ⌃⌥ ← → ↑ ↓ | Go to next window |
| ⌃⌥⇧ ← → ↑ ↓ | Move window |
| ⌥ 1–9 | Jump to window. 9 = last. ⇧ = move there |
| ⌃⌥ R | Half or full |
| ⌃⌥ = / − | Wider / thinner |
| ⌃⌥ F | Fill screen |
| ⌃⌥ C | Center |
| ⌃⌥ P / ⌃⌥⇧ P | Pin left / right. Again = unpin |
| ⌃⌥ T | Float. Again = back in strip |
| ⌃⌥ Q | Quit |

Want other key? Change `~/.config/fih/fih.toml`. Fih see change by itself. No know how? Ask AI.

## Where from

Fih is fork of [Paneru](https://github.com/karinushka/paneru). Paneru do hard part. Fih add pin, snap, step resize, and fish.

README talk like [caveman](https://github.com/juliusbrussee/caveman). Good repo. Make AI talk less too.

MIT license.
