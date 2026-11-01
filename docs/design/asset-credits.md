# Emblem sources

All emblems are pre-Islamic Iranian artwork, sourced from Wikimedia Commons and
converted to white-on-transparent masks so they can be tinted to the active era
palette. No Islamic-era ornament is used anywhere in Pangan.

| File | Emblem | Era | Source | Author | License |
|------|--------|-----|--------|--------|---------|
| `cyrus.png` | Achaemenid Shahbaz (falcon standard) | Cyrus the Great | [File:Achaemenid Falcon.svg](https://commons.wikimedia.org/wiki/File:Achaemenid_Falcon.svg) | Sodacan | Public domain |
| `darius.png` | Faravahar (winged disc) | Darius / Persepolis | [File:Faravahar-Gold.svg](https://commons.wikimedia.org/wiki/File:Faravahar-Gold.svg) | Shaahin | Public domain |
| `parthian.png` | Derafsh Kaviani | Parthian | [File:Derafsh kaviani standard.svg](https://commons.wikimedia.org/wiki/File:Derafsh_kaviani_standard.svg) | Oneasy | Free Art License 1.3 |
| `sasanid.png` | Senmurv in pearl roundel | Sasanid | [File:Senmurv.svg](https://commons.wikimedia.org/wiki/File:Senmurv.svg) | Shaahin | Public domain |
| `boteh.png` | Boteh (Iranian paisley) | UI ornament | [File:Simple paisley.svg](https://commons.wikimedia.org/wiki/File:Simple_paisley.svg) | Aliraeisdanaei | CC0 |

## Attribution requirement

`parthian.png` is used under the **Free Art License 1.3**, which requires the
author (Oneasy) and the license to be credited wherever the work is distributed.
The rest are public domain or CC0 and carry no obligation.

If Pangan is distributed publicly, surface this credit in the About screen or the
app's license file.

## Notes on processing

Each source SVG was rasterised, then reduced to a white-on-transparent mask so
Slint's `colorize` can tint it to the active era palette. `boteh.png` is the
outline filled to a solid silhouette, because the original hairline stroke was
illegible below ~40px.
