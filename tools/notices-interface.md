## Files of the interface

These files are in `web/`. The binary embeds them. The page never loads them from another place. The file `tests/page/origins.json` lists each copied file, its origin and its edits. The tool `tests/page/check_page.py` checks the hash of each byte copy.

| Component | Version | Licence | Where |
| --- | --- | --- | --- |
| Styles, scripts and components of Hune | Hune, commit `a55fff3` | MIT OR Apache-2.0 | `web/css/`, `web/js/`, `web/icons.svg`. Each file with edits names its origin in its first line. |
| Colour tokens of ClickHouse Dash | tag v1.3.1, commit `e970694e5e0b17b07cf187f3128af93fa1a82b76` | MIT | `web/css/vendor/chdash/` |
| Font Inter | 4.1 | SIL OFL-1.1 | `web/fonts/inter/`, with its `LICENSE` |
| Icons: the paths of `chevron-down`, `chevron-right`, `chevron-up` and the spinner | see the file | MIT (Tabler Icons) | `web/icons.svg` |

### ClickHouse Dash colour tokens

The file `web/css/vendor/chdash/00-tokens.css` is a byte copy of the file of ClickHouse Dash that Hune also holds. Its licence text is:

```text
MIT License

Copyright (c) 2026 clickhouse-dash contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Inter

The font file is `InterVariable.woff2`. It is a Font Software under the SIL Open Font License, Version 1.1. Pupitre does not sell the font by itself and does not change it. The licence text is the file `web/fonts/inter/LICENSE`:

```text
@@OFL@@
```

### Tabler Icons

Four icons of the sprite have the same paths as Tabler Icons (`chevron-down`, `chevron-right`, `chevron-up` and `loader-2`). Copyright (c) 2020-2025 Paweł Kuna. MIT licence: the text is the same as the MIT text above, with this copyright line.

