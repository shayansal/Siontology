# A browser and a desktop

An agent can be given a browser of its own (`--browser`), to read and use
web pages as a person does, and a desktop of its own (`--computer`), seen
in screenshots and used with a pointer and a keyboard. Both run in a
sandbox, reach the network only through a proxy that refuses this machine
and its network, and bring back content from outside, which taints the
session ([Your agent](index.md)).

Every output below is from a real run on a local devnet, with a scripted
model, against docs.rs as it was then (long identifiers and paths
shortened with `…`).

## A browser

`--browser` gives a session Chromium or Chrome, found on this machine (or
named by `SION_BROWSER`), with a profile made for the session and removed
after it: none of your cookies, history or passwords. On Linux it runs in
bubblewrap, which shows it the system's files read-only, its profile, and
the machine's managed browser policies (an administrator's certificates,
say), and nothing else of yours. It has no network of its own: every
connection goes through the egress proxy, to public addresses only, or
only to the hosts `--browser-host` names; never to this machine, its local
network or a cloud's metadata address.

The model sees a page as its accessibility tree: each element's role,
name and value, and a reference (`e12`) for each one it can act on. A
frame of the page's own site is shown where it is; a frame of another site
is named, not shown. Here an agent looks a crate up on docs.rs (each
result is shown cut to a screenful, as the command prints it):

```text
$ sion agent run coder 0f3d94ab…e058 --task 'What is the serde_json crate for? Look on docs.rs.' --model script:browse.json --browser --browser-host docs.rs --autonomy L5
session dbe45bdb119319ca for agent 0f3d94ab
  → browser_open {"url":"https://docs.rs/"}
  ← page https://docs.rs/
    title "Docs.rs"
    - RootWebArea "Docs.rs" focused
      - navigation "Main navigation" [ref=e1]
        - form [ref=e2]
          - link "Docs.rs" [ref=e3]
          - list [ref=e4]
            - listitem [ref=e5]
              - link "docs.rs" [ref=e6]
                - StaticText "▼"
          - list [ref=e7]
            - listitem [ref=e8]
    …
  → browser_type {"ref":"e11","submit":true,"text":"serde_json"}
  ← typed into e11
    page https://docs.rs/releases/search?query=serde_json
    title "Releases - Docs.rs"
    - RootWebArea "Releases - Docs.rs" focused
      - navigation "Main navigation" [ref=e1]
        - form [ref=e2]
          - link "Docs.rs" [ref=e3]
          - list [ref=e4]
            - listitem [ref=e5]
              - link "docs.rs" [ref=e6]
                - StaticText "▼"
          - list [ref=e7]
    …
  → browser_click {"ref":"e23"}
  ← clicked e23
    page https://docs.rs/serde_json/latest/serde_json/
    title "serde_json - Rust"
    - RootWebArea "serde_json - Rust" focused
      - navigation "Main navigation" [ref=e1]
        - form [ref=e2]
          - link "Docs.rs" [ref=e3]
          - list [ref=e4]
            - listitem [ref=e5]
              - link "serde_json-1.0.154 ▼" [ref=e6]
            - listitem [ref=e7]
              - link "Platform" [ref=e8]
    …
  → browser_screenshot {}
  ← a picture of https://docs.rs/serde_json/latest/serde_json/ ("serde_json - Rust")
    [a picture]
agent: serde_json is a JSON serialization file format, built on Serde; I took a picture of its page.
session dbe45bdb119319ca: done
```

The tools:

| Tool | What it does | Effect |
|---|---|---|
| `browser_open` | Opens a page (http or https) and returns its tree | external |
| `browser_snapshot` | The page as it is now | read |
| `browser_click` | Clicks an element, then returns the page | external |
| `browser_type` | Types into a field, replacing what it holds; with `submit`, presses Enter after | external |
| `browser_press` | Presses a key: Enter, Tab, Escape, Backspace, Delete, Space, the arrows, PageUp, PageDown, Home, End | external |
| `browser_scroll` | Scrolls about a screen, down or up | external |
| `browser_back` | Goes back a page | external |
| `browser_screenshot` | A picture of the page | read |
| `browser_fill_login` | Fills one of your logins into a field ([below](#logins)) | external |

The browser keeps one page: a link or a form aimed at another window, and
`window.open`, go to that page. A JavaScript dialog (`alert`, `confirm`)
is dismissed as it opens, and the next tree says what it asked. Downloads
are refused.

### Where it acts on its own

What a page says is content from outside, and an action on a page can
carry what the agent knows to a site, in an address or a form. So by
default, as with fetching a web page, a session that has read a page waits
for your approval before its next external call:

```text
$ sion agent run coder 7799dcea…2bce --task 'Search docs.rs for tiny_http.' --model script:ask.json --browser --autonomy L5
session 5bbf5b05e6b0d4f7 for agent 7799dcea
  → browser_open {"url":"https://docs.rs/"}
  ← page https://docs.rs/
    title "Docs.rs"
    - RootWebArea "Docs.rs" focused
      - navigation "Main navigation" [ref=e1]
        - form [ref=e2]
          - link "Docs.rs" [ref=e3]
          - list [ref=e4]
            - listitem [ref=e5]
              - link "docs.rs" [ref=e6]
                - StaticText "▼"
          - list [ref=e7]
            - listitem [ref=e8]
    …
  → browser_type {"ref":"e11","submit":true,"text":"tiny_http"}
  ← waiting for a person's approval
session 5bbf5b05e6b0d4f7: waiting for you: browser_type {"ref":"e11","submit":true,"text":"tiny_http"} (the session has read content from outside; external calls and spending need approval until the taint expires; request e6757544). Answer with `sion agent continue 5bbf5b05e6b0d4f7 --approve` or `--decline <why>`.
$ sion agent continue 5bbf5b05e6b0d4f7 --approve
  → browser_type {"ref":"e11","submit":true,"text":"tiny_http"} (approved by cli:operator)
  ← typed into e11
    page https://docs.rs/releases/search?query=tiny_http
    title "Releases - Docs.rs"
    - RootWebArea "Releases - Docs.rs" focused
      - navigation "Main navigation" [ref=e1]
        - form [ref=e2]
          - link "Docs.rs" [ref=e3]
          - list [ref=e4]
            - listitem [ref=e5]
              - link "docs.rs" [ref=e6]
                - StaticText "▼"
          - list [ref=e7]
    …
agent: The search for tiny_http found it.
session 5bbf5b05e6b0d4f7: done
```

Name hosts (`--browser-host docs.rs`; repeat for more; `*` for every
public host) and the browser reaches those alone, and acts on their pages
on its own, as the autonomy level allows, as in the first session above;
the session's other external calls (a payment, a message, a fetch) still
wait (`agent:browser-hosts`). A host it may not open is refused by the
proxy:

```text
$ sion agent run coder 0f3d94ab…e058 --task 'Open example.com.' --model script:away.json --browser --browser-host docs.rs --autonomy L5
session 1d84a75b58466df8 for agent 0f3d94ab
  → browser_open {"url":"https://example.com/"}
  ✗ https://example.com/ could not be reached: the egress proxy refused it (a host that is not allowed, or an address that is not public)
agent: example.com is not among the sites I may open.
session 1d84a75b58466df8: done
```

A session continued by a process of its own, as `sion agent continue` is
above, gets a new browser, which goes back to the page the session was on;
the references from before are kept only where they still name an element
of the same role and name, so that an approved click acts on what was
approved, or is refused. A server (`sion agent serve`, a bridge, a
gateway's host, the scheduler) keeps a session's browser open while the
session waits for an approval.

## Logins

Your logins are kept encrypted on this machine, beside the secrets and
under their key, each bound to one origin; the password is typed without
being shown, piped in, or taken from a variable:

```text
$ sion agent login add shop --origin https://shop.example.com/login --username ana --password-env SHOP_PASSWORD
added login shop for https://shop.example.com as ana, kept encrypted
$ sion agent login list
shop  https://shop.example.com  as ana
```

`--login shop` gives a session the login `shop` and the tool
`browser_fill_login`, whose description names the logins and their
origins. The browser fills a user name or a password only into a field
whose own frame is of the login's origin, as the browser knows it (not as
the page says): a field framed from another site, in a page of the right
one, is refused. The model never sees the password: it is cut out of all
the browser returns, the address a form sends it to included
(`agent:browser-logins`). The tests fill a login into a page on this
machine and check both.

## A desktop of its own

`--computer` gives a session a desktop: an X server (Xvfb) on a display of
its own, used with xdotool and seen with ImageMagick, and the programs
`--computer-program` names, which the agent may start. With bubblewrap
(the default), the server and every program run in the sandbox, with the
workspace as their home and the network only through the egress proxy, to
the hosts `--browser-host` names; with
`--computer docker:<image>@sha256:<digest>`, one container holds the
desktop, made from an image with those programs, on the Docker network
`--computer-network` names.

The tool is `computer`, with the actions computer-use models are trained
on: `screenshot`; `left_click`, `right_click`, `middle_click`,
`double_click`, `triple_click` and `mouse_move` at a coordinate;
`left_click_drag`; `type`; `key` (`ctrl+l`, `Return`); `scroll`; `wait`;
`cursor_position`; and `launch`, which starts an allowed program. Every
action answers with a screenshot. Here the desktop's program is Chrome,
which the agent opens on serde_json's page:

```text
$ sion agent run coder 0f3d94ab…e058 --task 'Open serde_json's page on docs.rs, on your desktop.' --model script:desktop.json --computer --workspace …/desk --computer-program '…/chrome --no-sandbox --no-first-run --user-data-dir=/workspace/.chrome --window-size=1280,800 --window-position=0,0 about:blank' --browser-host docs.rs --autonomy L5 --no-reflect
session acb082eb9506efde for agent 0f3d94ab
  → computer {"action":"screenshot"}
  ← the screen
    [a picture]
  → computer {"action":"launch","program":"chrome"}
  ← started chrome
    [a picture]
  → computer {"action":"wait","duration":4}
  ← waited 4 s
    [a picture]
  → computer {"action":"left_click","coordinate":[640,400]}
  ← left_click at (640, 400)
    [a picture]
  → computer {"action":"key","text":"ctrl+l"}
  ← pressed ctrl+l
    [a picture]
  → computer {"action":"type","text":"docs.rs/serde_json\n"}
  ← typed 19 characters
    [a picture]
  → computer {"action":"wait","duration":3}
  ← waited 3 s
    [a picture]
  → computer {"action":"cursor_position"}
  ← the pointer is at (640, 400)
    [a picture]
agent: The desktop's browser shows serde_json's page on docs.rs.
session acb082eb9506efde: done
```

Its last screenshot, as the model saw it:

![The agent's desktop: Chrome on serde_json's page on docs.rs](../guides/agent-desktop.png)

## Pictures

A screenshot reaches the model as a picture: in the tool's result for
Anthropic's models, and in a message after the results for
OpenAI-compatible ones, whose tool results hold text only. The model is
shown the last three pictures of the conversation; the session keeps the
bytes of the last six, and the conversation every picture's digest, so
that letting a picture go changes nothing a session's trajectory digest
covers.

## What this does not do yet

- The browser has one page at a time, and does not download or upload
  files.
- A desktop lives as long as the process that runs its session: a session
  continued by a process of its own gets a new one, with none of the
  programs it had started.
- Under bubblewrap, the address space of the browser and of the desktop's
  programs is not bounded: a browser reserves far more than it uses.
