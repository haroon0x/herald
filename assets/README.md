# Herald assets

Source files for the Herald identity and diagrams. Every asset is a
self-contained SVG: no external fonts, images, scripts, or network references.
The SVG is the source of truth; any PNG is a derived export.

## Index

| Asset | File | Size | viewBox | Used by |
| --- | --- | --- | --- | --- |
| logo-mark | [`logo-mark.svg`](logo-mark.svg) | 256 × 256 | `0 0 256 256` | Small marks, avatars, favicon exports |
| logo | [`logo.svg`](logo.svg) | 640 × 160 | `0 0 640 160` | README header, docs and project pages |
| banner | [`banner.svg`](banner.svg) | 1280 × 400 | `0 0 1280 400` | Project pages and release notes |
| social-preview | [`social-preview.svg`](social-preview.svg) | 1280 × 640 | `0 0 1280 640` | GitHub repository social preview |
| social-preview (PNG) | [`social-preview.png`](social-preview.png) | 1280 × 640 | — | Raster export for hosts that require PNG |
| architecture | [`architecture.svg`](architecture.svg) | 1200 × 760 | `0 0 1200 760` | [docs/architecture.md](../docs/architecture.md) |
| demo | [`demo.svg`](demo.svg) | 900 × 520 | `0 0 900 520` | Demo walkthroughs and CLI documentation |
| mascot | [`mascot.svg`](mascot.svg) | 512 × 512 | `0 0 512 512` | Project personality, docs, and social posts (the messenger owl) |

The architecture diagram is authored at 1200 × 760 and is referenced at 720 px
wide by `docs/architecture.md`. The demo is authored at 900 × 520 and remains
readable at 800 px wide.

## Palette

| Role | Hex | Use |
| --- | --- | --- |
| background | `#0B0F14` | Page and window background |
| panel | `#121821` | Tiers, panels, and cards |
| amber | `#F5B942` | Mark, prompts, approval gate, focal accents |
| cream | `#EDE7DA` | Primary text and the wordmark |
| slate | `#8A97A8` | Secondary text, notes, default connectors |
| success | `#5FD3A6` | Verified sends, succeeded states, receipts |
| danger | `#FF6B6B` | Failed states and no-auto-send warnings |

Raised cards use `#151D27`; hairlines and dividers are slate at low opacity.
Structural spacing follows an 8 px rhythm and corners use 12–16 px radii. Text
uses an Inter/system sans stack for prose and a `ui-monospace` stack for
commands, identifiers, and state names. The diagrams are drawn to be legible at
1×.

## Regenerating and converting

The SVGs are hand-authored; there is no build step. To edit an asset, edit the
SVG directly and keep the existing `viewBox`, `<title>`, and `<desc>` accurate.

If a raster copy is needed, either converter works:

```sh
# librsvg
rsvg-convert -w 1200 assets/architecture.svg -o /tmp/architecture.png
rsvg-convert -w 900  assets/demo.svg         -o /tmp/demo.png

# Inkscape (alternative)
inkscape assets/architecture.svg --export-type=png \
  --export-filename=/tmp/architecture.png --export-width=1200
```

Use `-w` / `--export-width` rather than scaling a rendered bitmap, so text stays
sharp. Browsers can open the SVGs directly, but browser screenshots are not a
substitute for a clean export. The social-preview PNG is committed because
hosts that require a raster cannot use the SVG; regenerate or replace it when
the banner changes.

## License

The assets are part of Herald and are licensed under AGPL-3.0-or-later, the same
license as the rest of the repository. See [`../LICENSE`](../LICENSE).

The Herald mark may be used to refer to the project (for example, in an article,
talk, or package that integrates with Herald). Do not use it in a way that
implies endorsement or that presents a fork as the official project. If you
modify an asset, keep the change distinguishable from the original.
