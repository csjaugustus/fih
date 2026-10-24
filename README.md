<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/fih-horizontal-white.svg">
  <img src="images/fih-horizontal-black.svg" alt="fih" width="360">
</picture>

# why many monitor when one do trick

**fih put all window in one long strip, slide strip, window come fast like fih, no lag**

**no second monitor, no third, max focus**

</div>

---

## why name fih

one time go oceanarium, see fih swim

fih fast, fih bendy, fih no get distracted, fih know where going

so app tile like fih: fast, smooth, no animation lag, default zero animation, window just there

| many monitor | fih |
|---|---|
| buy monitor, buy another, buy arm, dock, cable | one screen, strip go forever |
| turn neck to find window | window come to you |
| open window, other window shrink | open window, nothing shrink, strip grow |
| unplug laptop, layout explode | layout no care |
| need more room, buy more monitor | need more room, stack more strip, free |

## how work

window sit side by side on strip, strip go right forever

screen is little hole you look through, swipe four finger or press key, strip slide, window you want slide into hole

left right = slide in workspace, up down = switch workspace, go round and round

each workspace own strip, is like many monitor but cost zero

have laptop and monitor? workspace on both share one number line, keyboard go everywhere

need one window always there? pin it, it hold one column at left or right, rest of strip slide beside it

## strip map

move, little map show at bottom, one icon per window

go up down, map show all workspace, one row each

## get

ready app go in [Releases](https://github.com/csjaugustus/fih/releases)

1. open DMG, drag fih into Applications
2. open fih, Mac say no because fih not notarized, go System Settings → Privacy & Security, scroll down, click **Open Anyway**
3. Mac ask for Accessibility, say yes, fih need it to move window

fih live in menu bar now, start by itself at login, new version? replace app, open again

or brew: `brew install csjaugustus/fih/fih`, then `fih install && fih start`, Mac ask for Accessibility, say yes, new version? `brew upgrade fih && fih restart`

how delete: `/Applications/fih.app/Contents/MacOS/fih uninstall`, then app to Bin

### if you nerd want tinker

```sh
git clone https://github.com/csjaugustus/fih.git
cd fih
cargo run                     # try
cargo install --path .        # keep
```

macOS ask for Accessibility, say yes (System Settings → Privacy & Security), fih need it to move window

run all time: `fih install && fih start`

## how control

hold ⌃⌥ (control + option), add ⇧ to move window, not just look

| key | do |
|---|---|
| ⌃⌥ ← → | go to next window, on floating window: put it away, back to strip |
| ⌃⌥ ↑ ↓ | go to next workspace with window in it |
| ⌃⌥⇧ ← → | move window |
| ⌥ 1–9 | go to workspace 1–9, ⇧ = take window there |
| ⌃⌥ R | one column or full |
| ⌃⌥ F | fill screen |
| ⌃⌥ C | center |
| ⌃⌥ P / ⌃⌥⇧ P | pin left / right, again = unpin |
| ⌃⌥ T | float, again = back in strip |
| ⌃⌥ Q | quit |

can whitelist app to float, not go in strip, e.g. Telegram

want change any control or setting? open config file, not know how change? ask AI

## where from

fih is fork of [Paneru](https://github.com/karinushka/paneru), Paneru do hard part, fih add pin, snap, strip map, up-and-down strip, column, auto fill, and fish

README talk like [caveman](https://github.com/juliusbrussee/caveman), good repo, make AI talk less too

MIT license
