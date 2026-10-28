# Wheel styles and skins

Radlet's wheel has two separate settings, both in **Settings → Wheel**:

- **Style** decides how the wheel is laid out.
- **Skin** decides how it looks: colours, rim, type and motion.

Any skin works with any style.

## Styles

| Style | How it works | Best for |
| --- | --- | --- |
| **Dial** (default) | One ring of slots. Point at a slot and let go. | Everyone. Eight slots put every one on a quick direction. |
| **Orbit** (advanced) | The Dial plus an outer ring. Behind each slot sit two of its presets, so pushing further out gets more specific: JPEG, then *Smart* or *Best*. Push into a slot with more and its ring widens over the neighbours to show up to four. | People who use the same presets all the time. |
| **Bloom** (advanced) | The Dial. Rest on a slot (about a quarter of a second, adjustable) or flick past it, and up to four presets fan out, centred on the slot. Flicking straight out picks the first. | Reaching more presets without a busier wheel. |

Pick a style in **Settings → Wheel**: Orbit and Bloom are listed under **Advanced**. Slots stay in the same places in every style, so switching never means relearning where things are.

In both, a slot's presets stay out once they're showing, for as long as the drag goes on, until you rest on another slot of the inner ring. Overshooting, crossing the centre, briefly leaving the wheel, or heading for a side preset over a neighbouring slot (the outer edge of the ring under an open fan belongs to it) keeps them. Letting go past the ends of the presets does nothing.

Some things work the same in every style:

- The four slots up, right, down and left are the quickest to reach, so the most useful actions go there first; "More…", when needed, takes the last diagonal.
- A slot's position never changes because another one got used more. Usage only decides which slots stay on the wheel when they don't all fit.
- Letting go in the centre cancels.
- Each border is a little sticky, so a hand resting on the line between two slots doesn't make the highlight flicker.
- With **Haptic feedback** on, a Force Touch trackpad gives a light tap each time you cross into a slot.
- With Reduce Motion on (System Settings → Accessibility → Display), the wheel fades instead of springing.

**Slots** sets how many segments the ring has: 6, 8 (the default), 10 or 12. Past eight the slices get narrow, which makes slots easier to miss, so Orbit and Bloom are the better way to fit more on the wheel.

### Presets

The engine sends a slot's presets with the wheel (`variants` in the `inspect` reply). Presets always run straight away, never through a window:

| Slot | Presets |
| --- | --- |
| JPEG, WebP | Smart (the smallest that looks the same), Best (quality 95), Small (quality 60) |
| HEIC | Best (92), Small (60) |
| PDF, from two or more images | One PDF |
| Resize… | 50 %, 1920 px, 25 %, 1280 px |
| Compress… | Photos: Smart (JPEG and WebP only), 1 MB, 5 MB, Strong. Video: WhatsApp (15 MB), Discord (19 MB), Email (14 MB), Strong; the size limits need FFprobe. Others: Email, Strong |
| Rotate right | 180° |
| Save stills | 5 stills, 30 stills |
| Split… | Every page, Pairs |
| Watermark… | Draft, Copy, Sample |
| Remove background | And crop |
| Privacy check… | Remove all |
| More… | The first slots behind it |

## Skins

Radlet comes with six skins. Each is original; the game-flavoured ones nod to a genre without borrowing any game's art, names or fonts.

| Skin | Look |
| --- | --- |
| **Glass** (default) | The plain system look: see-through, in the Mac's own colours, accent and font, following light and dark mode, with icons. |
| **Instrument** | Graphite, a machined rim with tick marks, one lime accent, monospaced labels. |
| **Tactical** | Field kit: olive drab, amber readouts, flat edges, condensed capitals, no bounce. |
| **Neon** | Magenta and cyan on ink, faceted, glowing, with scanlines and a springier highlight. |
| **Sunset** | Hot pink and teal over plum, a warm rim, chunky capitals and icons. |
| **Porcelain** | Light: warm white, a steel rim and a blue accent. |

## Make your own

A skin is a small CSS file of custom properties in the Skins folder (`~/Library/Application Support/Radlet/Skins`). The quickest start:

1. In **Settings → Wheel**, choose the skin closest to what you want.
2. Click **New Skin from This**. Radlet writes a complete copy, with a comment on every line, selects it and shows it in Finder.
3. Open it in any text editor, change values and save. Radlet picks it up the next time the wheel opens (or click **Reload**).

A skin only needs what it changes. Everything else comes from its `--base`, or from Glass if it names none:

```css
/* Neon, but green. */
:root {
  --base: neon;
  --name: "Green Neon";
  --accent: #00FF9C;
  --glow: rgba(0, 255, 156, 0.6);
}
```

`var()` works as in CSS, with an optional fallback, so a colour can be set once and reused:

```css
:root {
  --base: tactical;
  --accent: #7FD1FF;
  --ticks: var(--accent);
  --bezel-2: var(--not-set, #22252B);
}
```

If something is wrong (a typo, a value out of range), Radlet skips just that value, uses the rest, and lists the problems with their line numbers under the skin picker. A skin with problems is marked "(needs a fix)".

### The Wheel Lab

The Wheel Lab is a storybook for the wheel, for designing skins and for checking the wheel while working on it. From the repository:

```sh
swift run --package-path mac RadletLab
```

Add `--skins <folder>` to work on skins in a folder other than the Skins folder.

It draws the wheel with the app's own code (`mac/Sources/RadletUI`), so what you see is what ships.

- **Stories.** The sidebar lists each style's states: opening, resting, a slot or preset picked, Orbit's presets brightening, Bloom's petals, long tool names, loading and empty. Hover a wheel to drive it with the app's own pointer tracking, including Bloom's rest-to-open; click to see a choice made; move away and the story comes back.
- **Stage.** Show one wheel, or the story in every skin side by side. Choose 6, 8, 10 or 12 slots, a busy wallpaper (for Glass), a plain or checked backdrop, light or dark, and the zoom. Reduce Motion draws the wheel as the app does with that setting on; Slow Motion makes the springs four times slower; Tour moves the highlight round on its own; ⌘R replays the opening.
- **Skin editor.** Every token, grouped as in the table below, with a colour well, slider or picker beside the CSS value. Changes show on the wheel as you make them, a dot marks each changed token, and the panel underneath shows them as a short skin file: the name, `--base`, and only what differs from the base. **Copy** puts that on the clipboard; **Save as New Skin** writes it to the Skins folder, where Radlet's Settings lists it. Files in the folder reload in the lab as soon as they change, so you can also edit in a text editor and watch.

### Tokens

**About**

| Token | Value |
| --- | --- |
| `--name` | Shown in Settings. Defaults to the file's name. |
| `--author` | You. |
| `--base` | The built-in skin to start from: `glass` (the default, used when a skin names none), `instrument`, `tactical`, `neon`, `sunset` or `porcelain`. |

**Colours.** `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, `hsl()`, `hsla()`, `transparent`, `black`, `white`, or one of the Mac's own colours, which follow light and dark mode and the accent colour: `system-accent`, `label`, `secondary-label`, `tertiary-label`, `separator`, `window`, `control`.

| Token | What it colours |
| --- | --- |
| `--accent` | The highlight. |
| `--accent-text` | Text on the accent (with `--highlight: fill`, and on Bloom's chosen preset). |
| `--text`, `--text-dim` | Labels, and secondary text. |
| `--ground`, `--ground-2` | The disc, and the hub and the disc's sheen. |
| `--segment` | Each slot's fill. |
| `--divider` | Lines between slots. |
| `--bezel`, `--bezel-2` | The rim's light and dark sides. |
| `--ticks` | Tick marks on the rim. |
| `--glow` | Glow around the highlight. |
| `--shadow` | Shadow under the wheel. |

**Look**

| Token | Value |
| --- | --- |
| `--material` | `solid`, or `glass` (see-through; give `--ground` some transparency). |
| `--shape` | `round`, or `faceted`: one flat edge per slot (wheels with fewer than six slots stay round). |
| `--highlight` | `wedge` (tinted slot with a bright edge), `fill` (solid accent) or `rim` (bright edge only). |
| `--labels` | `text`, or `icons` (a symbol above the text). |
| `--label-case` | `upper` or `none`. |
| `--scanlines` | `on` or `off`. |
| `--tick-count` | 0–120 marks round the rim; 0 for none. |
| `--bezel-width` | 0–24px. |
| `--segment-gap` | 0–6deg between slots. |
| `--glow-radius` | 0–40px. |
| `--highlight-opacity` | 0–1 (or a percentage): the wedge's tint. |

**Type**

| Token | Value |
| --- | --- |
| `--font` | `system`, `system-rounded`, `system-serif`, `system-mono`, or a font family installed on the Mac in quotes, such as `"Avenir Next Condensed"`. A font that isn't installed falls back to the system's. |
| `--label-size` | 9–20px. |
| `--label-weight` | 100–900, or `thin`, `light`, `normal`, `medium`, `semibold`, `bold`, `heavy`, `black`. |
| `--letter-spacing` | -0.1–0.5em. |

**Motion**

| Token | Value |
| --- | --- |
| `--spring-response` | 0.1–1.2s: how long the highlight takes to glide to a new slot. |
| `--spring-damping` | 0.3–1: 1 settles without bouncing; lower values bounce. |
| `--stagger` | 0–60ms between labels appearing as the wheel opens. |

### Rules

- Radlet reads only `:root { … }` (`.wheel { … }` works too). Other selectors are skipped and listed.
- Comments (`/* … */`) and `!important` are allowed; `!important` changes nothing.
- Files over 64 KB, files that aren't UTF-8 text, and anything that isn't a regular file are skipped. A `var()` that loops, names nothing, or grows past a kilobyte is reported and skipped.
- Skins hold only colours, sizes, keywords and font names: no paths, URLs, images or code. A skin from anywhere is safe to drop in.
- Please don't ship skins that copy a game's logos, artwork or licensed fonts. A genre's mood is fair game; someone else's trademarks aren't.

## Sharing skins

Choose one of your skins in **Settings → Wheel**, then **Share**:

- **Copy Link** puts a `radlet://add/skin?…` link on the clipboard. Anyone with Radlet who opens it sees the skin on a live wheel and can add it.
- **Save as File…** saves a `.radletskin` file, which opens in Radlet the same way.

Either way, what's shared is the skin's name, author and base, plus only the tokens that differ from that base. Adding a shared skin writes it afresh into the Skins folder with Radlet's own tokens only; lines Radlet doesn't understand are left out, and a name you already have gets a number. Links hold the skin itself, so they work offline and never fetch anything.
