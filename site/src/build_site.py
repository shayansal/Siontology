"""Build the pages of site/ from the fragments in site/src/pages/.

Usage, from anywhere, with Python 3.8 or later and nothing else:

    python3 site/src/build_site.py          # write the pages
    python3 site/src/build_site.py --check  # report pages that differ from a fresh build

Each fragment starts with a JSON header in an HTML comment:

    <!--{"file": "index.html", "title": "...", "description": "...",
         "nav": "technology", "scenes": ["fold-hero"]}-->

Optional header keys: "noindex": true keeps a page out of search engines and
the sitemap; "base": "/" makes every local link absolute (the 404 page, which
is served at any address).

Placeholders in a fragment: {{fig:<id>}} or {{fig:<id>:<number>}} for a
figure and {{code:<id>}} for a code block, both defined in figures.py;
{{REPO}} and {{DOCS}} for the repository and documentation addresses;
{{toggle}} for a figure's Pause button; {{icon:<id>}} for a pictogram from
figures.py.

Besides the pages, the build writes sitemap.xml and robots.txt.
"""
import html
import json
import os
import re
import sys

SRC = os.path.dirname(os.path.abspath(__file__))
PAGES = os.path.join(SRC, "pages")
SITE = os.path.dirname(SRC)
sys.path.insert(0, SRC)
sys.dont_write_bytecode = True  # keep site/ free of __pycache__
from figures import FIGURES, CODE, ICONS  # noqa: E402

ORIGIN = "https://siontology.org/"
REPO = "https://github.com/ColdAI-org/siontology"
DOCS = ORIGIN + "docs/"

SMALL_MARK = ('<svg viewBox="0 0 22 22" width="22" height="22" aria-hidden="true" focusable="false" fill="currentColor">'
              '<rect x="0" y="0" width="22" height="8"/><rect x="0" y="10" width="10" height="6"/>'
              '<rect x="12" y="10" width="10" height="6"/><rect x="0" y="18" width="4" height="4"/>'
              '<rect x="6" y="18" width="4" height="4"/><rect x="12" y="18" width="4" height="4"/>'
              '<rect x="18" y="18" width="4" height="4"/></svg>')

NAV = [
    ("uses", "use-cases.html", "Use cases"),
    ("technology", "technology.html", "Technology"),
    ("developers", "developers.html", "Developers"),
    ("open-source", "open-source.html", "Open source"),
    ("security", "security.html", "Security"),
    ("papers", "whitepapers.html", "Papers"),
]

# Order of the sitemap; any other indexable page follows, sorted.
SITEMAP_ORDER = ["index.html", "use-cases.html", "technology.html", "developers.html", "open-source.html",
                 "security.html", "whitepapers.html", "brand.html", "legal.html"]

THEME_ICON = ('<svg viewBox="0 0 18 18" aria-hidden="true" focusable="false">'
              '<rect x="1.75" y="1.75" width="14.5" height="14.5" fill="none" stroke="currentColor" stroke-width="1.5"/>'
              '<path d="M2.5 15.5 15.5 2.5V15.5Z" fill="currentColor"/></svg>')

HEAD_SCRIPT = ("<script>(function(){var d=document.documentElement;d.classList.add('js');"
               "try{var t=localStorage.getItem('siontology-theme');"
               "if(t==='light'||t==='dark')d.setAttribute('data-theme',t)}catch(e){}})();</script>")


def header(nav_key, base):
    items = []
    for key, href, label in NAV:
        cur = ' aria-current="page"' if key == nav_key else ""
        items.append(f'<li><a href="{base}{href}"{cur}>{label}</a></li>')
    items.append(f'<li><a href="{REPO}">GitHub</a></li>')
    return f'''<a class="skip-link" href="#main">Skip to content</a>
<header class="site-header">
  <div class="wrap">
    <a class="brand" href="{base}index.html" aria-label="Siontology, home">{SMALL_MARK}<span class="brand-name">Siontology</span></a>
    <button class="icon-button menu-button" type="button" data-menu-toggle aria-expanded="false" aria-controls="site-nav">Menu</button>
    <nav class="site-nav" id="site-nav" aria-label="Main">
      <ul>
        {"".join(items)}
      </ul>
    </nav>
    <div class="header-tools">
      <button class="icon-button theme-button" type="button" data-theme-toggle aria-label="Switch theme">{THEME_ICON}</button>
    </div>
  </div>
</header>'''


def footer(base):
    return f'''<footer class="site-footer">
  <div class="wrap">
    <div class="footer-grid">
      <div class="footer-brand">
        <a class="brand" href="{base}index.html" aria-label="Siontology, home">{SMALL_MARK}<span class="brand-name">Siontology</span></a>
        <p>An open-source neo-blockchain whose product is verified knowledge, not ordered transactions.</p>
      </div>
      <nav aria-labelledby="footer-project">
        <h2 id="footer-project">Project</h2>
        <ul>
          <li><a href="{base}use-cases.html">Use cases</a></li>
          <li><a href="{base}technology.html">Technology</a></li>
          <li><a href="{base}developers.html">Developers</a></li>
          <li><a href="{base}open-source.html">Open source</a></li>
          <li><a href="{base}security.html">Security</a></li>
          <li><a href="{base}whitepapers.html">Papers</a></li>
        </ul>
      </nav>
      <nav aria-labelledby="footer-resources">
        <h2 id="footer-resources">Resources</h2>
        <ul>
          <li><a href="{DOCS}">Documentation</a></li>
          <li><a href="{REPO}">Source code</a></li>
          <li><a href="{base}brand.html">Brand and press kit</a></li>
          <li><a href="{base}legal.html">Imprint and privacy</a></li>
        </ul>
      </nav>
    </div>
    <div class="footer-base">
      <p>&copy; ColdAI. Code licensed Apache-2.0 OR MIT.</p>
      <p>Status: a devnet-scale reference implementation. Proofs are simulated; there is no mainnet and no token.</p>
    </div>
  </div>
</footer>'''


PAUSE = ('<svg class="icon-pause" viewBox="0 0 10 10" aria-hidden="true" focusable="false"><rect x="1" y="1" width="3" height="8" fill="currentColor"/><rect x="6" y="1" width="3" height="8" fill="currentColor"/></svg>'
         '<svg class="icon-play" viewBox="0 0 10 10" aria-hidden="true" focusable="false"><path d="M2 1 9 5 2 9Z" fill="currentColor"/></svg>')


def toggle():
    return f'<button class="plate-toggle" type="button" data-scene-toggle aria-pressed="false" hidden>{PAUSE}<span data-label>Pause</span></button>'


def figure(fid, number=None):
    f = FIGURES[fid]
    uid = f"fig-{fid}" + (f"-{number}" if number else "")
    num = f'<span class="fig">Fig. {number}</span>' if number else ""
    meta = []
    if f.get("run"):
        meta.append(f'<div><dt>Run it</dt><dd><code>{html.escape(f["run"])}</code></dd></div>')
    if f.get("spec"):
        label, href = f["spec"]
        meta.append(f'<div><dt>Source</dt><dd><a href="{href}">{label}</a></dd></div>')
    if f.get("status"):
        cls, label = f["status"]
        meta.append(f'<div><dt>Status</dt><dd><span class="status status--{cls}">{label}</span></dd></div>')
    meta_html = f'<dl class="plate-meta">{"".join(meta)}</dl>' if meta else ""
    return f'''<figure class="plate" aria-labelledby="{uid}-title">
  <canvas data-scene="{f["scene"]}" role="img" aria-label="{html.escape(f["label"])}" aria-describedby="{uid}-desc"></canvas>
  <figcaption>
    <div class="plate-title"><p class="plate-name" id="{uid}-title">{num}{f["title"]}</p>{toggle()}</div>
    <p class="plate-desc" id="{uid}-desc">{f["desc"]}</p>
    {meta_html}
  </figcaption>
</figure>'''


WRAP = {"verify-output", "demo-agents", "demo-x402", "demo-receipts", "demo-invoice",
        "demo-market", "demo-streams"}  # terminal output that reads better wrapped than scrolled


def code(cid):
    label, body = CODE[cid]
    pre_id = f"code-{cid}"
    cls = "code code--wrap" if cid in WRAP else "code"
    return f'''<div class="{cls}">
  <div class="code-label"><span>{label}</span><button class="copy-button" type="button" data-copy="{pre_id}">Copy</button></div>
  <pre id="{pre_id}"><code>{body}</code></pre>
</div>'''


def page(meta, body):
    base = meta.get("base", "")
    scenes = meta.get("scenes", [])
    scripts = ""
    if scenes:
        scripts = f'\n<script src="{base}js/scenes/core.js" defer></script>' + "".join(
            f'\n<script src="{base}js/scenes/{s}.js" defer></script>' for s in scenes)
    title = meta["title"]
    desc = html.escape(meta["description"])
    if meta.get("noindex"):
        index_meta = '<meta name="robots" content="noindex">'
    else:
        canonical = ORIGIN + ("" if meta["file"] == "index.html" else meta["file"])
        index_meta = f'''<link rel="canonical" href="{canonical}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Siontology">
<meta property="og:title" content="{html.escape(title)}">
<meta property="og:description" content="{desc}">
<meta property="og:url" content="{canonical}">
<meta property="og:image" content="{ORIGIN}img/siontology-social-1200x630.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:image:alt" content="Siontology: verified knowledge, not ordered transactions.">
<meta name="twitter:card" content="summary_large_image">'''
    return f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title>
<meta name="description" content="{desc}">
<meta name="color-scheme" content="light dark">
<meta name="theme-color" content="#000000" media="(prefers-color-scheme: dark)">
<meta name="theme-color" content="#ffffff" media="(prefers-color-scheme: light)">
<link rel="icon" href="{base}favicon.svg" type="image/svg+xml">
<link rel="icon" href="{base}img/favicon-32.png" sizes="32x32" type="image/png">
<link rel="apple-touch-icon" href="{base}img/apple-touch-icon.png">
{index_meta}
<link rel="stylesheet" href="{base}css/tokens.css">
<link rel="stylesheet" href="{base}css/site.css">
{HEAD_SCRIPT}
<script src="{base}js/site.js" defer></script>{scripts}
</head>
<body>
{header(meta.get("nav"), base)}
<main id="main">
{body.strip()}
</main>
{footer(base)}
</body>
</html>
'''


def strip_tags(text):
    return html.unescape(re.sub(r"<[^>]+>", "", text)).strip()


def label_table(match):
    """Give every body cell of a table with three or more columns a data-label
    naming its column, so that on phones, where rows stack, each value keeps
    its heading. Cells that already carry a data-label keep it."""
    table = match.group(0)
    if "table--api" in table.split(">", 1)[0]:
        return table
    head = re.search(r"<thead>(.*?)</thead>", table, re.S)
    if not head:
        return table
    labels = [strip_tags(h) for h in re.findall(r"<th[^>]*>(.*?)</th>", head.group(1), re.S)]
    if len(labels) < 3:
        return table

    def label_row(row):
        col = 0
        out = []
        pos = 0
        for cell in re.finditer(r"<(td|th)(\s[^>]*)?>", row.group(0)):
            attrs = cell.group(2) or ""
            span = re.search(r'colspan="(\d+)"', attrs)
            width = int(span.group(1)) if span else 1
            tag = cell.group(0)
            is_row_header = cell.group(1) == "th"
            if "data-label" not in attrs and not is_row_header and col < len(labels):
                name = labels[col] if width == 1 else " and ".join(labels[col:col + width])
                tag = tag[:-1] + f' data-label="{html.escape(name)}">'
            out.append(row.group(0)[pos:cell.start()] + tag)
            pos = cell.end()
            col += width
        out.append(row.group(0)[pos:])
        return "".join(out)

    body = re.search(r"<tbody>(.*?)</tbody>", table, re.S)
    if not body:
        return table
    labelled = re.sub(r"<tr>.*?</tr>", label_row, body.group(1), flags=re.S)
    return table[:body.start(1)] + labelled + table[body.end(1):]


def icon(iid):
    return ('<svg class="pict" viewBox="0 0 32 32" width="32" height="32" aria-hidden="true" '
            'focusable="false">' + ICONS[iid] + "</svg>")


def expand(body):
    body = re.sub(r"\{\{icon:([a-z0-9-]+)\}\}", lambda m: icon(m.group(1)), body)
    body = re.sub(r"\{\{fig:([a-z0-9-]+)(?::(\d+))?\}\}", lambda m: figure(m.group(1), m.group(2)), body)
    body = re.sub(r"\{\{code:([a-z0-9-]+)\}\}", lambda m: code(m.group(1)), body)
    body = body.replace("{{REPO}}", REPO).replace("{{DOCS}}", DOCS).replace("{{toggle}}", toggle())
    body = re.sub(r'<table class="table[^"]*">.*?</table>', label_table, body, flags=re.S)
    return body


def sitemap(files):
    urls = "".join(f"  <url><loc>{ORIGIN}{'' if f == 'index.html' else f}</loc></url>\n" for f in files)
    return ('<?xml version="1.0" encoding="UTF-8"?>\n'
            '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
            f"{urls}</urlset>\n")


ROBOTS = f"""User-agent: *
Disallow: /src/

Sitemap: {ORIGIN}sitemap.xml
"""


def build():
    """Return {path inside site/: content} for every generated file."""
    out = {}
    indexed = []
    for name in sorted(os.listdir(PAGES)):
        if not name.endswith(".html"):
            continue
        with open(os.path.join(PAGES, name), encoding="utf-8") as f:
            raw = f.read()
        m = re.match(r"\s*<!--(\{.*?\})-->", raw, re.S)
        meta = json.loads(m.group(1))
        out[meta["file"]] = page(meta, expand(raw[m.end():]))
        if not meta.get("noindex"):
            indexed.append(meta["file"])
    ordered = [f for f in SITEMAP_ORDER if f in indexed] + sorted(f for f in indexed if f not in SITEMAP_ORDER)
    out["sitemap.xml"] = sitemap(ordered)
    out["robots.txt"] = ROBOTS
    return out


def main():
    check = "--check" in sys.argv[1:]
    stale = []
    for name, content in build().items():
        path = os.path.join(SITE, name)
        if check:
            try:
                with open(path, encoding="utf-8", newline="") as f:
                    same = f.read() == content
            except FileNotFoundError:
                same = False
            if not same:
                stale.append(name)
        else:
            with open(path, "w", encoding="utf-8", newline="\n") as f:
                f.write(content)
            print("wrote", name)
    if check:
        if stale:
            print("out of date:", ", ".join(stale))
            sys.exit(1)
        print("all generated files match a fresh build")


if __name__ == "__main__":
    main()
