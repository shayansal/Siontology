# Siontology brand guidelines

Siontology is an open-source neo-blockchain published by ColdAI. Its product
is verified knowledge, not ordered transactions, and the brand has the same
job as the network: make proofs easy to read. It is black and white, it has
one red that means a refusal, it sets statements in a wide grotesque and
anything you can type in a monospace, and it moves only to explain a
mechanism.

Everything here is open source with the project. The files in this folder
are the source of truth; the website's press kit (`site/press/`) is a copy.

## Contents

1. [The name](#1-the-name)
2. [The mark](#2-the-mark)
3. [Colour](#3-colour)
4. [Typography](#4-typography)
5. [Motion](#5-motion)
6. [Iconography](#6-iconography)
7. [Diagrams](#7-diagrams)
8. [Voice and tone](#8-voice-and-tone)
9. [Do and don't](#9-do-and-dont)
10. [Press kit](#10-press-kit)

## 1. The name

| Write | Not |
|---|---|
| Siontology | SIONTOLOGY, SionTology, Sion, Sion-tology |
| `sion` (the command-line tool, in code style) | Sion CLI, SION |
| neo-blockchain (the category, lower case, hyphenated) | Neo-Blockchain, NeoBlockchain |
| ColdAI (the publisher) | Cold AI, COLDAI, Coldai |

- **Siontology** is one word with a capital S and the rest in lower case, in
  headlines, running text, slides and alt text alike. Never set it in
  capitals in running text; the wordmark is not in capitals either, so
  there is no case where capitals are right.
- Don't shorten it to *Sion*. `sion` exists only as the name of the
  command, the crates (`sion-core`, `sion-exec` and so on) and the
  environment variables (`SION_NODE`, `SION_CHAIN`), always in code style.
- *Neo-blockchain* is a description, not a name: "Siontology is an
  open-source neo-blockchain".
- Mechanisms are capitalised as names: Pulse, the Fold, World Proof, Linear
  Lanes, Escrow Lanes, Contention Consensus, the Assurance Algebra,
  Verifiable Clearing, Agent Charters. Forced inclusion and
  data-availability sampling stay in lower case.
- The site is siontology.org, the documentation is at siontology.org/docs,
  and the source is github.com/ColdAI-org/siontology.

## 2. The mark

The mark is the Fold drawn as a partition of a square: a full binary tree of
proofs, with eight leaf proofs at the bottom joined pairwise into four, then
two, then the one World Proof that spans the top. It is the same picture as
the hero animation on the website, stopped.

### Construction

The master mark is drawn on a 69 × 69 unit grid with rectangles only.

| Row | Cells | Cell width | Row height |
|---|---|---|---|
| World Proof | 1 | 69 | 23 |
| Two proofs | 2 | 33 | 17 |
| Four proofs | 4 | 15 | 12 |
| Eight leaf proofs | 8 | 6 | 8 |

Every gap, between cells and between rows, is 3 units. Each row is about
1/√2 the height of the row above, so the tree reads as folding upward.

The small mark is for 16 to 32 pixels: three rows of one, two and four cells
on a 22 × 22 unit grid, with 2-unit gaps and every edge on an even unit, so
it renders sharply at 16 pixels. Its rows are 8, 6 and 4 units high.

### Sizes and versions

| File | Use |
|---|---|
| `logo/siontology-mark-on-dark.svg`, `-on-light.svg` | The mark, from 40 px up |
| `logo/siontology-mark-small-on-dark.svg`, `-on-light.svg` | The mark below 40 px, down to 16 px |
| `logo/siontology-wordmark-on-dark.svg`, `-on-light.svg` | The name alone |
| `logo/siontology-lockup-on-dark.svg`, `-on-light.svg` | Mark and name together: the default sign-off |
| `favicon/favicon.svg`, `favicon-32.png` | Browser tabs: the small mark on a black tile |
| `favicon/icon.svg`, `icon-512.png`, `apple-touch-icon-180.png` | App and touch icons: the mark on a black tile |

- **Wordmark.** "Siontology" in Archivo at width 115 and weight 540, tracked
  −8/1000 em, converted to outlines. It is a drawing, not live text: use the
  files.
- **Lockup.** The mark is 1.1 times the wordmark's cap height and sits on its
  baseline; the gap between them is 0.42 times the mark's width. Use the
  lockup files rather than rebuilding it.
- **Clear space.** Keep at least the height of the mark's top row, a third
  of the mark, clear on every side. For the lockup, measure it from the
  mark.
- **Minimum size.** The small mark down to 16 px; the master mark and the
  lockup from 40 px high. Below that, as in the website's header, set the
  small mark beside the name.
- **Colour.** Solid white on black, or solid black on white. On a photograph
  or a colour, put it on a black or white panel instead.

## 3. Colour

The palette is achromatic on purpose: in a diagram, meaning is carried by
shape, not hue, so the figures survive greyscale printing and colour-vision
differences. Dark is the signature look; light is its exact inverse.

The one hue, **signal**, means a refusal: a conflicting spend, a withheld
share, a payment the ledger refused, a forged signature. It is never
decoration, never emphasis, never a highlight and never a button.

| Token | Dark | Light | Role |
|---|---|---|---|
| `--color-bg` | `#000000` | `#ffffff` | Page background |
| `--color-bg-raised` | `#0e0e0e` | `#f4f4f4` | Code blocks, raised panels |
| `--color-ink` | `#f5f5f5` | `#000000` | Text, the mark, accepted and certified states |
| `--color-ink-2` | `#b8b8b8` | `#3d3d3d` | Secondary text |
| `--color-ink-3` | `#848484` | `#676767` | Tertiary text, pending states |
| `--color-rule` | `#262626` | `#dcdcdc` | Hairlines and grid lines |
| `--color-rule-strong` | `#4d4d4d` | `#a3a3a3` | Emphasised rules |
| `--color-signal` | `#ff5a4c` | `#c4271b` | Refusal only |

`--color-ink-3` has a contrast of 5.6:1 on the background in both themes;
signal has 6.8:1 on black and 5.7:1 on white, so a refusal is always legible
as text.

The tokens are in `tokens.css` (the source of truth, also used by the
website) and `tokens.json`. Light values sit on `:root`; dark values apply
under `prefers-color-scheme: dark` unless the page sets
`data-theme="light"`, and always under `data-theme="dark"`.

## 4. Typography

| Role | Face | Setting |
|---|---|---|
| Display | Archivo | Width 125, weight 480, tracking −0.022 em, leading 1.02 |
| Statement | Archivo | Width 118, weight 500, tracking −0.02 em, leading 1.04 |
| Heading | Archivo | Width 106 to 112, weight 560 to 600 |
| Text | Archivo | Width 100, weight 400, 17 px on 1.58, measure about 66 characters |
| Code, commands, identifiers, formulas | Fragment Mono | Weight 400 only |
| Greek letters, arrows, logic operators | Siontology Symbols | Fallback inside both stacks |

- **One family carries the voice.** Archivo's width axis does the work that
  weight usually does: statements are wide rather than bold.
- **The monospace is for things you can type or compute**: commands,
  identifiers, hashes, formulas. Never for labels or decoration.
- **Siontology Symbols** is a 9 KB subset of Noto Sans Math with the Greek
  letters, arrows, and logic and comparison operators the text faces lack
  (∧ ∨ ∞ κ δ φ ≥ ≤ √ →). It sits in both font stacks, so formulas set
  correctly anywhere.
- **Sentence case everywhere.** No labels in capitals, no tracked-out
  eyebrows. Use tabular figures where numbers line up in columns.
- **Licences.** All three faces are under the SIL Open Font License 1.1.
  They come from the npm packages `@fontsource-variable/archivo`,
  `@fontsource/fragment-mono` and `@fontsource/noto-sans-math` (5.3.0) and
  are served by the website itself (`site/fonts/`, with their licence
  files), never from a font service.

## 5. Motion

Motion explains a mechanism or it does not happen.

- **One Pulse is one second.** The Pulse is the cadence of every protocol
  animation (`--duration-pulse: 1000ms`).
- **Folding** two cells into one takes 720 ms on `--ease-fold`
  (`cubic-bezier(0.65, 0, 0.35, 1)`). Interface changes take 160 to 240 ms
  on `--ease-standard` (`cubic-bezier(0.2, 0, 0, 1)`).
- **Faithful.** Every animation is a run of one mechanism, with numbers from
  a demo, the specification or the architecture paper, and a caption that
  says what it shows, what to run to see it for real, and its status in
  the reference implementation (built, simplified or simulated).
- **Calm.** Animations loop slowly, never flash, never use parallax or
  scroll-jacking, and pause when they leave the screen.
- **In the reader's control.** Every animation has a Pause button. With
  `prefers-reduced-motion: reduce`, each shows one static, labelled frame,
  chosen to carry the whole idea, and offers Play.
- **Faces, the one exception.** Where a page shows agents to people rather
  than a protocol at work, an agent's diamond may have a face: eyes of
  solid cells, a mouth of lines, and beside it a sign of its work. A face
  may breathe, blink, look about and follow the pointer, slowly, to show
  that the agent is at work, though it explains no mechanism. It never
  flashes, stops with a Pause button, and starts paused with reduced
  motion.

## 6. Iconography

Shapes keep one meaning everywhere, in diagrams, icons and status markers.

| Shape | Means |
|---|---|
| Solid cell | A linear object, a certificate, proof stack A: something real and final; status *built* |
| Outline cell | A claim in the knowledge log, or proof stack B |
| Dashed outline | A tombstone, a withheld share, or something simulated; status *simulated* |
| Half-filled square | Status *simplified* |
| A dash | Status *not built* |
| Hatched cell | A contended batch, ordered by consensus |
| Tally strokes | Approvals, one stroke per validator |
| Rounded rectangle | A phone running a light client: the only rounded shape |
| Diamond | An AI agent; with a face, an agent shown at work |
| Check | Accepted or verified |
| Cross, in signal red | Refused, conflicting or withheld |

Icons are drawn on a 32-unit grid with strokes of 1.5 to 1.8 units and no
fills other than solid cells.

## 7. Diagrams

- **Drawn, not illustrated.** Square cells on a grid, 1-pixel hairlines, no
  gradients, shadows, glows or 3D.
- **Labels** in sentence case, set in Archivo, horizontal, next to what they
  name; values in tabular figures.
- **Every number is sourced.** A diagram names the demo, specification
  section or code behind it, and says whether a figure was measured or is
  a design target.
- **Status is shown, not implied.** Simulated parts are dashed and labelled
  as simulated.
- **Both themes.** Diagrams take their colours from the tokens, so they
  follow the reader's theme.

## 8. Voice and tone

Plain, exact and honest about status.

- Say what something does, in the active voice, in sentence case.
- Give every number its unit and its source, and say whether it was
  measured or is a design target.
- Say what is simulated or missing before anyone has to ask.
- Prefer a sentence a reader can check to an adjective they have to trust.
- Never discuss price, returns or investment. Asked about a token, say only:
  "Siontology has no token and nothing is on offer. Fees are paid in
  stablecoins."

| Write | Not |
|---|---|
| Designed so that a phone can check the whole history. Today the proofs are checked by replay. | Verify the entire chain on your phone, instantly. |
| Design target: 100,000 operations a second on a testnet. | 100k TPS. |
| The ledger refuses the payment: payee not allowed. | Our AI keeps your agents safe. |
| Post-quantum signatures from two unrelated families. | Quantum-proof. Unhackable. |

## 9. Do and don't

**Do**

- Use the supplied files, and scale them proportionally.
- Put the white mark on black and the black mark on white.
- Keep a third of the mark clear on every side.
- Use the small mark below 40 pixels.
- Label every number as measured or as a design target.
- Name the demo, specification or code behind a diagram.

**Don't**

- Recolour, outline, stretch or rotate the mark, or add shadows, gradients
  or effects.
- Rearrange, redraw or add cells.
- Set the name in capitals or split it in two.
- Use signal red for decoration, emphasis or buttons.
- Put the mark on a busy photograph or a colour.
- Use the mark to suggest that ColdAI endorses another project or product.
- Imitate another company's identity: no borrowed logos, layouts or
  slogans.
- Discuss price, returns or investment.

## 10. Press kit

The website's [brand page](https://siontology.org/brand.html) offers these
files for download; the originals are in this folder.

| File | Format |
|---|---|
| Mark, on dark and on light | SVG |
| Small mark, on dark and on light | SVG, for 16 to 32 px |
| Wordmark, on dark and on light | SVG |
| Lockup, on dark and on light | SVG |
| App icon | SVG; PNG, 512 px |
| Favicon | SVG; PNG, 32 px; PNG, 180 px |
| Social image | PNG, 1200 × 630; SVG |
| Design tokens | CSS; JSON |

**About Siontology** (for press use): Siontology is an open-source
neo-blockchain published by ColdAI. Its product is verified knowledge rather
than ordered transactions: every claim, whether about a payment, a
computation, an AI output or an event in the world, carries a proof or
priced evidence, and every Pulse the proofs fold into one World Proof. The
reference implementation is written in Rust and licensed Apache-2.0 OR MIT.
It runs as a devnet: its succinct proofs are simulated, its validators run
in one process, and there is no mainnet and no token.
