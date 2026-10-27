# Website sources

The HTML pages in `site/` are generated from the fragments in `pages/` by
`build_site.py`. Edit the fragments, `figures.py` or the build script, not the
generated files.

## Rebuild

Python 3.8 or later, standard library only, no network:

```bash
python3 site/src/build_site.py           # writes site/*.html, sitemap.xml and robots.txt
python3 site/src/build_site.py --check   # exits 1 if a generated file is out of date
```

A rebuild reproduces the committed files byte for byte. Run `--check` before
committing.

## What is where

| Path | What it holds |
|---|---|
| `pages/*.html` | One fragment per page: a JSON header (`file`, `title`, `description`, `nav`, `scenes`, and optionally `noindex` and `base`) followed by the page's main content |
| `figures.py` | Each figure's title, caption, alt text, demo command, source link and implementation status, and the code blocks |
| `build_site.py` | The shared head, header and footer, the placeholders, column labels for tables on phones, the sitemap and `robots.txt` |
| `../js/scenes/` | The animations, one file per figure; `core.js` runs them |
| `../css/tokens.css` | A copy of `brand/tokens.css`: change the brand file first, then copy it here |

Placeholders in fragments: `{{fig:<id>}}` or `{{fig:<id>:<number>}}` for a
figure, `{{code:<id>}}` for a code block, `{{REPO}}` and `{{DOCS}}` for the
repository and documentation addresses, and `{{toggle}}` for a figure's Pause
button.

The 404 page sets `"base": "/"`, so its links work at any address. `robots.txt`
keeps `/src/` out of search engines.

## Rules for content

- No claim may go further than `docs/implementation-status.md`. Label every
  figure's status built, simplified or simulated as that file does, and every
  number as measured or a design target.
- Name the demo, specification section or code behind every figure.
- Siontology has no token and nothing is on offer. Fees are paid in
  stablecoins. Use that wording and nothing more.
