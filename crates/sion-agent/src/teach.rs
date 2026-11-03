//! Teaching an agent a task by doing it once: a person's browser recorded
//! while they do the task, and the recording written down as a draft skill.
//!
//! [`Teaching::start`] opens Chromium (or Chrome) in a window of its own,
//! with a profile made for the recording and removed after it, and speaks
//! the DevTools protocol with it over its pipe ([`crate::cdp`]): no port is
//! opened. Into every frame of every tab it puts a recorder, in a world of
//! its own that the page's scripts cannot reach, which says what the person
//! does: the element they click, by its role and name, as the agent's
//! snapshot will name it; what they type into a field, once they leave it
//! or press Enter; the option they choose, and the names of the files they
//! pick. The browser says which pages open, in which tabs, what the pages
//! ask and how the person answered, and which files download.
//!
//! **What is never recorded.** A password, and any field that looks secret
//! (a one-time code, a card's number or its code), is recorded as filled
//! and no more: its value never leaves the page. Nor is the user name of a
//! login form: the draft has the agent fill both from a login the operator
//! saved. Addresses lose their user names and passwords, the values of
//! parameters that look like tokens, codes, keys or signatures, and
//! fragments that hold values. Nothing is recorded keystroke by keystroke,
//! nor the pointer's path, nor what pages show.
//!
//! A recording lasts at most [`MAX_MINUTES`] minutes and [`MAX_STEPS`]
//! steps. [`draft`] writes it down as a `SKILL.md` for the agent's browser
//! tools: a step for each thing done, each element named by its role and
//! name, the values typed listed, so that the person can make them the
//! task's own before installing it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cdp::{Cdp, PIPE_SHELL};

/// The longest a recording lasts, in minutes.
pub const MAX_MINUTES: u64 = 10;

/// The most steps a recording keeps.
pub const MAX_STEPS: usize = 500;

/// The most characters of a name, a title or a message kept.
const MAX_TEXT: usize = 160;

/// The most characters of a value typed that are kept.
const MAX_VALUE: usize = 500;

/// Navigations this close after the one before are one: a redirect.
const REDIRECT_MS: u64 = 2_000;

/// A navigation this soon after a step on its tab is what the step led to.
const LED_MS: u64 = 5_000;

/// The world the recorder runs in, apart from the page's scripts.
const WORLD: &str = "sion-teach";

/// The function the recorder calls to say what the person did.
const BINDING: &str = "sionTeach";

/// The recorder, put into every frame. It says what the person did
/// (trusted events only) and never sends the value of a secret field or
/// of a login's user name.
const RECORDER: &str = r#"(() => {
  if (globalThis.__sionTeach) return;
  globalThis.__sionTeach = true;
  const send = (step) => { try { sionTeach(JSON.stringify(step)); } catch (_) {} };
  const text = (s) => String(s || '').replace(/\s+/g, ' ').trim().slice(0, 160);
  const NOT_TYPED = /^(button|submit|reset|checkbox|radio|file|image|range|color|hidden)$/i;
  const isField = (el) => !!el && el.nodeType === 1 && (
    (el.tagName === 'INPUT' && !NOT_TYPED.test(el.getAttribute('type') || 'text')) ||
    el.tagName === 'TEXTAREA' || el.isContentEditable);
  const SECRET_COMPLETE = /(password|one-time-code|cc-number|cc-csc|cc-exp)/i;
  // What a field's name, id, label, placeholder or description say of it,
  // its camelCase and separators made into words.
  const SECRET_WORDS = /\b(pass|password|passwort|passcode|passphrase|mot de passe|contrase(n|ñ)a|pwd|pw|pin|pins|pincode|otp|totp|mfa|2fa|one ?time|verification code|security code|auth code|authentication code|cvv|cvc|csc|card ?number|cc ?num|ssn|social security|secret|token|api ?key|private key|seed phrase|recovery (code|phrase))\b/i;
  const words = (s) => String(s || '').replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/[_\-.\[\]:]+/g, ' ');
  const described = (el) => words([el.getAttribute('name'), el.id, el.getAttribute('placeholder'),
    el.getAttribute('aria-label'), el.getAttribute('title'),
    el.labels ? [...el.labels].map((l) => l.textContent).join(' ') : ''].join(' '));
  const passwords = new WeakSet();
  const isPassword = (el) => {
    if (!el || el.tagName !== 'INPUT') return false;
    if ((el.getAttribute('type') || '').toLowerCase() === 'password') passwords.add(el);
    return passwords.has(el) || /password/i.test(el.getAttribute('autocomplete') || '');
  };
  // A field whose characters the page hides as a password's.
  const masked = (el) => {
    try {
      const style = getComputedStyle(el);
      return (style.webkitTextSecurity || style.getPropertyValue('-webkit-text-security') || 'none') !== 'none';
    } catch (_) { return false; }
  };
  const isSecret = (el) => isPassword(el) || masked(el) ||
    SECRET_COMPLETE.test(el.getAttribute('autocomplete') || '') ||
    SECRET_WORDS.test(described(el));
  const hasPassword = (scope) => {
    try { return !!scope && !!scope.querySelector && !!scope.querySelector('input[type=password]'); }
    catch (_) { return false; }
  };
  const inLogin = (el) => {
    if (el.tagName !== 'INPUT') return false;
    if (/username/i.test(el.getAttribute('autocomplete') || '')) return true;
    return /^(text|email|tel)$/i.test(el.getAttribute('type') || 'text') &&
      (hasPassword(el.form) || hasPassword(el.getRootNode()) || hasPassword(document));
  };
  // Password fields marked as they appear, and before anything shows what
  // they hold: a field shown as text later is still one.
  const markAll = (root) => {
    try { for (const el of root.querySelectorAll('input')) isPassword(el); } catch (_) {}
  };
  markAll(document);
  try {
    new MutationObserver((records) => {
      for (const record of records) {
        if (record.type === 'attributes' && record.target.tagName === 'INPUT') {
          if ((record.oldValue || '').toLowerCase() === 'password') passwords.add(record.target);
          isPassword(record.target);
        }
        for (const node of record.addedNodes || []) {
          if (node.nodeType !== 1) continue;
          if (node.tagName === 'INPUT') isPassword(node); else markAll(node);
        }
      }
    }).observe(document, {subtree: true, childList: true, attributes: true,
      attributeFilter: ['type'], attributeOldValue: true});
  } catch (_) {}
  const ROLES = {A: 'link', BUTTON: 'button', SELECT: 'combobox', TEXTAREA: 'textbox',
    SUMMARY: 'button', OPTION: 'option', IMG: 'image', H1: 'heading', H2: 'heading',
    H3: 'heading', H4: 'heading', H5: 'heading', H6: 'heading', LI: 'listitem', TD: 'cell',
    TH: 'columnheader', LABEL: 'label'};
  const INPUT_ROLES = {button: 'button', submit: 'button', reset: 'button', image: 'button',
    checkbox: 'checkbox', radio: 'radio', range: 'slider', search: 'searchbox',
    number: 'spinbutton', file: 'button'};
  const roleOf = (el) => {
    const given = text(el.getAttribute('role')).split(' ')[0];
    if (given) return given;
    if (el.tagName === 'A') return el.hasAttribute('href') ? 'link' : 'generic';
    if (el.tagName === 'INPUT') {
      const type = (el.getAttribute('type') || 'text').toLowerCase();
      return INPUT_ROLES[type] || (el.list ? 'combobox' : 'textbox');
    }
    if (el.tagName === 'SELECT') return el.multiple || el.size > 1 ? 'listbox' : 'combobox';
    if (el.isContentEditable) return 'textbox';
    return ROLES[el.tagName] || 'generic';
  };
  const nameOf = (el) => {
    const ids = el.getAttribute('aria-labelledby');
    if (ids) {
      const named = text(ids.split(/\s+/).map((id) => document.getElementById(id))
        .filter(Boolean).map((n) => n.textContent).join(' '));
      if (named) return named;
    }
    const aria = text(el.getAttribute('aria-label'));
    if (aria) return aria;
    if (el.labels && el.labels.length) {
      const labelled = text([...el.labels].map((l) => l.innerText || l.textContent).join(' '));
      if (labelled) return labelled;
    }
    if (el.tagName === 'INPUT') {
      const type = (el.getAttribute('type') || 'text').toLowerCase();
      if (type === 'submit') return text(el.value) || 'Submit';
      if (type === 'reset') return text(el.value) || 'Reset';
      if (type === 'button') return text(el.value);
      if (type === 'image') return text(el.alt);
    }
    if (isField(el) || el.tagName === 'SELECT') {
      return text(el.getAttribute('placeholder')) || text(el.title);
    }
    const said = text(el.innerText || el.textContent);
    if (said) return said;
    const img = el.querySelector && el.querySelector('img[alt]');
    return text(img && img.alt) || text(el.title);
  };
  const ACTIVE = 'a[href],button,input,select,textarea,summary,option,label,[role],[onclick],[contenteditable]';
  const outside = (node) => node === document.body || node === document.documentElement;
  // What was clicked: the innermost element that says it can be, or else
  // the element where the pointer cursor starts; none on a page's plain
  // text and space.
  const targetOf = (event) => {
    const path = (event.composedPath ? event.composedPath() : [event.target])
      .filter((node) => node && node.nodeType === 1);
    for (const node of path) {
      if (outside(node)) break;
      if (node.matches(ACTIVE) &&
          !/^(presentation|none|generic)$/i.test(node.getAttribute('role') || '-')) return node;
    }
    let found = null;
    for (const node of path) {
      if (outside(node) || getComputedStyle(node).cursor !== 'pointer') break;
      found = node;
    }
    return found;
  };
  const kept = new WeakMap();
  const fingerprint = (value) => {
    let h = 2166136261;
    for (let i = 0; i < value.length; i++) { h ^= value.charCodeAt(i); h = Math.imul(h, 16777619); }
    return value.length + ':' + (h >>> 0);
  };
  const typed = (el) => {
    const value = el.isContentEditable ? String(el.innerText || '') : String(el.value || '');
    const seen = fingerprint(value);
    if (kept.get(el) === seen) return;
    kept.set(el, seen);
    const secret = isSecret(el);
    const login = !secret && inLogin(el);
    const step = {act: 'type', role: roleOf(el), name: nameOf(el), secret,
      password: secret && isPassword(el), login, origin: location.origin};
    if (!secret && !login) step.value = value.slice(0, 500);
    send(step);
  };
  addEventListener('focusin', (event) => {
    const el = event.composedPath()[0];
    if (el && el.nodeType === 1) isPassword(el);
  }, true);
  // Enter in a field submits its form through the form's button, with a
  // click of its own: the press is the step, and that click is not.
  let entered = null;
  const submits = (el) => !!el.form && ((el.tagName === 'BUTTON' &&
    (el.getAttribute('type') || 'submit').toLowerCase() === 'submit') ||
    (el.tagName === 'INPUT' && /^(submit|image)$/i.test(el.getAttribute('type') || '')));
  addEventListener('click', (event) => {
    if (!event.isTrusted) return;
    const el = targetOf(event);
    if (!el || isField(el)) return;
    if (entered && submits(el) && el.form === entered.form && performance.now() - entered.at < 1000) {
      entered = null;
      return;
    }
    if (el.tagName === 'SELECT' || el.tagName === 'OPTION' || el.tagName === 'LABEL') return;
    if (el.tagName === 'INPUT' && /^(checkbox|radio|file)$/i.test(el.getAttribute('type') || '')) return;
    send({act: 'click', role: roleOf(el), name: nameOf(el)});
  }, true);
  addEventListener('change', (event) => {
    if (!event.isTrusted) return;
    const el = event.composedPath()[0];
    if (!el || el.nodeType !== 1) return;
    const type = (el.getAttribute('type') || '').toLowerCase();
    if (el.tagName === 'SELECT') {
      send({act: 'choose', role: roleOf(el), name: nameOf(el),
        option: text([...el.selectedOptions].map((o) => o.label || o.text).join(', '))});
    } else if (el.tagName === 'INPUT' && (type === 'checkbox' || type === 'radio')) {
      send({act: 'click', role: roleOf(el), name: nameOf(el), checked: !!el.checked});
    } else if (el.tagName === 'INPUT' && type === 'file') {
      send({act: 'files', role: 'button', name: nameOf(el),
        files: [...(el.files || [])].slice(0, 20).map((f) => text(f.name))});
    } else if (isField(el)) {
      typed(el);
    }
  }, true);
  addEventListener('keydown', (event) => {
    if (!event.isTrusted || event.isComposing) return;
    const el = event.composedPath()[0];
    if (!el || el.nodeType !== 1) return;
    if (event.key === 'Enter' && isField(el) && el.tagName === 'INPUT') {
      entered = {form: el.form, at: performance.now()};
      typed(el);
      send({act: 'press', key: 'Enter', role: roleOf(el), name: nameOf(el)});
    } else if (event.key === 'Escape') {
      send({act: 'press', key: 'Escape', role: roleOf(el), name: nameOf(el)});
    }
  }, true);
  addEventListener('focusout', (event) => {
    const el = event.composedPath()[0];
    if (event.isTrusted && el && el.nodeType === 1 && el.isContentEditable) typed(el);
  }, true);
  addEventListener('pagehide', () => {
    const el = document.activeElement;
    if (isField(el)) typed(el);
  }, true);
  // The page's title, once it has one: the browser says only its address.
  if (window === top) {
    const titled = () => send({act: 'title', title: text(document.title), url: location.href});
    if (document.readyState === 'loading') {
      addEventListener('DOMContentLoaded', titled, {once: true});
    } else {
      titled();
    }
    addEventListener('load', titled, {once: true});
  }
})();"#;

/// Parameters of an address whose values are not kept, by a part of their
/// names.
const SECRET_PARAMETERS: [&str; 24] = [
    "token",
    "code",
    "key",
    "sig",
    "secret",
    "auth",
    "session",
    "sid",
    "pass",
    "pwd",
    "otp",
    "state",
    "nonce",
    "ticket",
    "jwt",
    "credential",
    "hash",
    "email",
    "mail",
    "user",
    "login",
    "hint",
    "phone",
    "account",
];

static PROFILES: AtomicU64 = AtomicU64::new(0);

/// What the person did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// A page opened that no step led to: the start, or an address typed.
    Open {
        /// Its address.
        url: String,
        /// Its title.
        #[serde(default)]
        title: String,
    },
    /// The page the step before led to.
    Arrive {
        /// Its address.
        url: String,
        /// Its title.
        #[serde(default)]
        title: String,
    },
    /// A tab opened, by the page or the person.
    Tab {
        /// The address it opened with.
        url: String,
        /// Its title.
        #[serde(default)]
        title: String,
    },
    /// An element clicked, or a box checked.
    Click {
        /// Its role.
        role: String,
        /// Its name.
        name: String,
        /// For a checkbox or a radio button, whether it is checked after.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checked: Option<bool>,
    },
    /// What was typed into a field.
    Type {
        /// The field's role.
        role: String,
        /// Its name.
        name: String,
        /// What was typed, unless it was secret or a login's user name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        /// Whether the field looked secret: its value was not recorded.
        #[serde(default)]
        secret: bool,
        /// Whether it was a password field.
        #[serde(default)]
        password: bool,
        /// Whether it was the user name of a login form, not recorded.
        #[serde(default)]
        login: bool,
        /// The origin of the field's frame, which a saved login must be for.
        #[serde(default)]
        origin: String,
    },
    /// An option chosen in a list.
    Choose {
        /// The list's role.
        role: String,
        /// Its name.
        name: String,
        /// The option, as it reads.
        option: String,
    },
    /// Files picked for a field: their names only.
    Files {
        /// The field's role.
        role: String,
        /// Its name.
        name: String,
        /// The files' names.
        files: Vec<String>,
    },
    /// A key pressed: Enter in a field, or Escape.
    Press {
        /// The key.
        key: String,
        /// The role of what had the focus.
        role: String,
        /// Its name.
        name: String,
    },
    /// A question a page asked, and the answer.
    Dialog {
        /// alert, confirm or prompt.
        kind: String,
        /// What it asked.
        message: String,
        /// Whether it was answered OK.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accepted: Option<bool>,
        /// What was written in a prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
    },
    /// A file downloaded.
    Download {
        /// Its name.
        file: String,
    },
}

impl Act {
    /// Whether the person did it on the page (and a page may follow).
    pub fn is_action(&self) -> bool {
        matches!(
            self,
            Act::Click { .. }
                | Act::Type { .. }
                | Act::Choose { .. }
                | Act::Files { .. }
                | Act::Press { .. }
                | Act::Dialog { .. }
        )
    }
}

/// One thing the person did, when and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Milliseconds after the recording started.
    pub at_ms: u64,
    /// The tab it was in, from 1.
    pub tab: usize,
    /// What it was.
    #[serde(flatten)]
    pub act: Act,
}

/// A recording.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recording {
    /// When it started (RFC 3339, UTC).
    pub started: String,
    /// How long it lasted, in seconds.
    pub seconds: u64,
    /// Whether it was cut at its limit of time or steps.
    pub cut: bool,
    /// What the person did, in order.
    pub steps: Vec<Step>,
}

/// How the browser is started for a recording.
#[derive(Clone, Debug)]
pub struct Options {
    /// The Chromium (or Chrome) program.
    pub program: PathBuf,
    /// The page it opens first.
    pub start: Option<String>,
    /// How long the recording may last (at most [`MAX_MINUTES`]).
    pub limit: Duration,
    /// Without a window: where a program plays the person (tests, demos).
    pub headless: bool,
    /// Whether Chromium runs in its own sandbox, which does not start as
    /// root.
    pub own_sandbox: bool,
    /// The window's size, in pixels.
    pub size: (u32, u32),
}

/// A tab's page, or a frame of it from another site, as the browser
/// attached it.
#[derive(Clone, Debug)]
struct Attached {
    tab: usize,
    frame: bool,
    main_frame: Option<String>,
    url: String,
}

/// A recording under way.
pub struct Teaching {
    child: Child,
    cdp: Cdp,
    profile: PathBuf,
    attached: BTreeMap<String, Attached>,
    tabs: Vec<String>,
    steps: Vec<Step>,
    started: Instant,
    started_at: u64,
    limit: Duration,
    cut: bool,
    gone: bool,
}

impl std::fmt::Debug for Teaching {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Teaching")
            .field("steps", &self.steps.len())
            .finish_non_exhaustive()
    }
}

impl Drop for Teaching {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

fn running_as_root() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// `text` on one line, at most `most` characters, its email addresses left
/// out (a page's title or an account menu often holds the person's own).
fn cut(text: &str, most: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut kept = String::new();
    for word in flat.split(' ') {
        if !kept.is_empty() {
            kept.push(' ');
        }
        let core = word.trim_matches(|c: char| !c.is_alphanumeric());
        let email = core.split_once('@').is_some_and(|(user, host)| {
            !user.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
        });
        if email {
            kept.push_str(&word.replace(core, "(an email address)"));
        } else {
            kept.push_str(word);
        }
    }
    kept.chars().take(most).collect()
}

/// Whether a part of an address's path looks like a token: long, of one
/// piece, with letters and digits mixed (not a page's name).
fn token_like(segment: &str) -> bool {
    let tokenish = segment
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '=');
    let digits = segment.chars().any(|c| c.is_ascii_digit());
    let letters = segment.chars().any(|c| c.is_ascii_alphabetic());
    tokenish
        && ((segment.len() >= 32)
            || (segment.len() >= 16 && digits && letters && !segment.contains('-')))
}

/// `url` as it is kept: without a user name or password, the values of
/// parameters that look like tokens, codes, keys or signatures (or are
/// long enough to be one), or a fragment that holds values; at most 300
/// characters.
pub fn clean_url(url: &str) -> String {
    let (rest, fragment) = match url.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (url, None),
    };
    let (base, query) = match rest.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (rest, None),
    };
    let mut out = match base.split_once("://") {
        Some((scheme, after)) => {
            let (authority, path) = match after.find('/') {
                Some(at) => after.split_at(at),
                None => (after, ""),
            };
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let path: Vec<&str> = path
                .split('/')
                .map(|segment| if token_like(segment) { "…" } else { segment })
                .collect();
            format!("{scheme}://{host}{}", path.join("/"))
        }
        None => base.to_string(),
    };
    if let Some(query) = query {
        let kept: Vec<String> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((name, value)) if !value.is_empty() => {
                    let lower = name.to_ascii_lowercase();
                    if value.len() > 64 || SECRET_PARAMETERS.iter().any(|s| lower.contains(s)) {
                        format!("{name}=…")
                    } else {
                        pair.to_string()
                    }
                }
                _ => pair.to_string(),
            })
            .collect();
        if !kept.is_empty() {
            out.push('?');
            out.push_str(&kept.join("&"));
        }
    }
    if let Some(fragment) = fragment {
        if !fragment.is_empty() && fragment.len() <= 40 && !fragment.contains(['=', '&']) {
            out.push('#');
            out.push_str(fragment);
        }
    }
    out.chars().take(300).collect()
}

/// Whether a page at `url` is the browser's own, not one to record.
fn ignorable(url: &str) -> bool {
    url.is_empty()
        || [
            "about:",
            "chrome:",
            "chrome-error:",
            "chrome-search:",
            "chrome-untrusted:",
            "devtools:",
            "data:",
            "blob:",
            "javascript:",
        ]
        .iter()
        .any(|p| url.starts_with(p))
}

/// Before a password typed in tab `tab` for `origin`: the value typed last
/// into a text field of that origin, among the steps just before (on the
/// password's page, or the page before it where a sign-in asks for the
/// name first), taken as the login's user name and not kept.
pub fn forget_user_name(steps: &mut [Step], tab: usize, origin: &str) {
    let before = steps
        .iter_mut()
        .rev()
        .take(8)
        .filter(|s| s.tab == tab)
        .map(|s| &mut s.act)
        .find(|a| matches!(a, Act::Type { origin: o, value: Some(_), .. } if o == origin));
    if let Some(Act::Type {
        value, login, role, ..
    }) = before
    {
        if matches!(role.as_str(), "textbox" | "searchbox" | "combobox") {
            *value = None;
            *login = true;
        }
    }
}

/// The step the recorder's `payload` says, checked: no secret's value,
/// texts cut to length.
pub fn act_of(payload: &Value) -> Option<Act> {
    let text = |key: &str| cut(payload[key].as_str().unwrap_or_default(), MAX_TEXT);
    let role = text("role");
    let name = text("name");
    match payload["act"].as_str()? {
        "click" => Some(Act::Click {
            role,
            name,
            checked: payload["checked"].as_bool(),
        }),
        "type" => {
            let secret = payload["secret"] == true;
            let login = payload["login"] == true;
            Some(Act::Type {
                role,
                name,
                value: if secret || login {
                    None
                } else {
                    payload["value"]
                        .as_str()
                        .map(|v| v.chars().take(MAX_VALUE).collect())
                },
                secret,
                password: secret && payload["password"] == true,
                login,
                origin: text("origin"),
            })
        }
        "choose" => Some(Act::Choose {
            role,
            name,
            option: text("option"),
        }),
        "files" => Some(Act::Files {
            role,
            name,
            files: payload["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(20)
                .map(|f| cut(f, MAX_TEXT))
                .collect(),
        }),
        "press" => Some(Act::Press {
            key: text("key"),
            role,
            name,
        }),
        _ => None,
    }
}

impl Teaching {
    /// Opens the browser for the person and starts recording.
    pub fn start(options: &Options) -> Result<Teaching, String> {
        if options.own_sandbox && running_as_root() {
            return Err(
                "Chromium does not start its sandbox as root: teach as an ordinary user".into(),
            );
        }
        // A folder made now, under a name no one could have guessed: one
        // already there (another user's, say) is never used.
        let profile = (0..8)
            .find_map(|_| {
                use std::hash::{BuildHasher, Hasher};
                let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
                hasher.write_u64(PROFILES.fetch_add(1, Ordering::Relaxed));
                hasher.write_u128(Instant::now().elapsed().as_nanos());
                let dir = std::env::temp_dir().join(format!(
                    "sion-teach-{}-{:016x}",
                    std::process::id(),
                    hasher.finish()
                ));
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&dir).ok().map(|()| dir)
            })
            .ok_or("no folder could be made for the browser's profile")?;
        let mut args = vec![
            "--remote-debugging-pipe".to_string(),
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-sync".into(),
            "--disable-extensions".into(),
            "--disable-default-apps".into(),
            "--disable-background-networking".into(),
            "--disable-component-update".into(),
            "--disable-features=Translate,MediaRouter,OptimizationHints".into(),
            "--password-store=basic".into(),
            "--use-mock-keychain".into(),
            format!("--window-size={},{}", options.size.0, options.size.1),
            "about:blank".into(),
        ];
        if options.headless {
            args.insert(0, "--headless=new".into());
            args.insert(1, "--mute-audio".into());
        }
        if !options.own_sandbox {
            args.insert(0, "--no-sandbox".into());
        }
        let program = options.program.to_string_lossy().into_owned();
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(PIPE_SHELL)
            .arg(&program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("the browser {program} could not start: {e}"))?;
        let stdin = child.stdin.take().ok_or("no pipe to the browser")?;
        let stdout = child.stdout.take().ok_or("no pipe from the browser")?;
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut teaching = Teaching {
            child,
            cdp: Cdp::for_person(stdin, stdout, Duration::from_secs(30)),
            profile,
            attached: BTreeMap::new(),
            tabs: Vec::new(),
            steps: Vec::new(),
            started: Instant::now(),
            started_at,
            limit: options
                .limit
                .min(Duration::from_secs(MAX_MINUTES * 60))
                .max(Duration::from_secs(1)),
            cut: false,
            gone: false,
        };
        teaching
            .cdp
            .send(None, "Browser.getVersion", json!({}))
            .map_err(|e| format!("the browser did not start: {e}"))?;
        // A window downloads where the person's browser would; without one,
        // into the profile, which goes with it.
        let downloads = if options.headless {
            json!({"behavior": "allow", "downloadPath": teaching.profile.join("downloads").to_string_lossy(), "eventsEnabled": true})
        } else {
            json!({"behavior": "default", "eventsEnabled": true})
        };
        let _ = teaching
            .cdp
            .send(None, "Browser.setDownloadBehavior", downloads);
        teaching
            .cdp
            .send(None, "Target.setDiscoverTargets", json!({"discover": true}))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while teaching.tabs.is_empty() {
            if Instant::now() >= deadline || !teaching.pump(Duration::from_millis(100)) {
                return Err("the browser opened no page".into());
            }
        }
        if let Some(start) = &options.start {
            teaching.send_to_tab(1, "Page.navigate", json!({"url": start}))?;
        }
        Ok(teaching)
    }

    /// The steps recorded so far.
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// Sends `method` to the page in tab `tab` (from 1): for a program that
    /// plays the person, as in tests and demos (`Input.dispatchMouseEvent`,
    /// `Input.insertText`, `Page.handleJavaScriptDialog`).
    pub fn send_to_tab(&self, tab: usize, method: &str, params: Value) -> Result<Value, String> {
        let session = self
            .attached
            .iter()
            .find(|(_, a)| a.tab == tab && !a.frame)
            .map(|(session, _)| session.clone())
            .ok_or_else(|| format!("there is no tab {tab}"))?;
        self.cdp.send(Some(&session), method, params)
    }

    /// Takes in what happened, waiting at most `wait` for something to;
    /// false once the browser is closed, or the recording reached its
    /// limit.
    pub fn pump(&mut self, wait: Duration) -> bool {
        if self.gone || self.cut {
            return false;
        }
        let left = self.limit.saturating_sub(self.started.elapsed());
        if left.is_zero() {
            self.cut = true;
            return false;
        }
        let mut events = Vec::new();
        if let Some(first) = self.cdp.wait_for(|_| true, wait.min(left)) {
            events.push(first);
            events.extend(self.cdp.take(|_| true));
        }
        for event in events {
            self.take_in(&event);
        }
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            self.gone = true;
            for event in self.cdp.take(|_| true) {
                self.take_in(&event);
            }
            return false;
        }
        !self.cut
    }

    /// Ends the recording: closes the browser, removes its profile, and
    /// returns what was recorded.
    pub fn finish(mut self) -> Recording {
        // What the browser has yet to say: the last page's title.
        let until = Instant::now() + Duration::from_millis(500);
        while !self.gone && Instant::now() < until {
            if !self.pump(Duration::from_millis(100)) {
                break;
            }
        }
        if !self.gone {
            let _ = self
                .cdp
                .send_waiting(None, "Browser.close", json!({}), Duration::from_secs(2));
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until && !matches!(self.child.try_wait(), Ok(Some(_))) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Recording {
            started: crate::schedule::rfc3339(self.started_at),
            seconds: self.started.elapsed().as_secs(),
            cut: self.cut,
            steps: std::mem::take(&mut self.steps),
        }
    }

    fn ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn push(&mut self, tab: usize, act: Act) {
        if self.steps.len() >= MAX_STEPS {
            self.cut = true;
            return;
        }
        // A password: the user name typed just before it, on its page or
        // the one before (a sign-in asking for one, then the other), is
        // the login's too, and is not kept.
        if let Act::Type {
            password: true,
            origin,
            ..
        } = &act
        {
            forget_user_name(&mut self.steps, tab, origin);
        }
        let at_ms = self.ms();
        // A field typed into again: what it ends up holding.
        if let Act::Type { role, name, .. } = &act {
            if let Some(last) = self.steps.last_mut() {
                if let Act::Type {
                    role: r, name: n, ..
                } = &last.act
                {
                    if last.tab == tab && r == role && n == name {
                        last.act = act;
                        last.at_ms = at_ms;
                        return;
                    }
                }
            }
        }
        self.steps.push(Step { at_ms, tab, act });
    }

    /// Puts the recorder into the page or frame attached as `session`, and
    /// lets it run if it waits for that.
    fn instrument(&self, session: &str, release: bool) {
        let commands = [
            (
                "Runtime.addBinding",
                json!({"name": BINDING, "executionContextName": WORLD}),
            ),
            (
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": RECORDER, "worldName": WORLD, "runImmediately": true}),
            ),
            ("Page.enable", json!({})),
            ("Runtime.enable", json!({})),
            (
                "Target.setAutoAttach",
                json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
            ),
        ];
        for (method, params) in commands {
            let _ = self.cdp.send(Some(session), method, params);
        }
        if release {
            let _ = self
                .cdp
                .send(Some(session), "Runtime.runIfWaitingForDebugger", json!({}));
        }
    }

    fn known(&self, target: &str) -> bool {
        self.tabs.iter().any(|t| t == target)
    }

    /// A tab: attached, its recorder put in, and, after the first, noted.
    fn new_tab(&mut self, session: String, info: &Value, release: bool) {
        let target = info["targetId"].as_str().unwrap_or_default().to_string();
        let tab = self.tabs.len() + 1;
        self.tabs.push(target);
        self.attached.insert(
            session.clone(),
            Attached {
                tab,
                frame: false,
                main_frame: None,
                url: String::new(),
            },
        );
        self.instrument(&session, release);
        if tab > 1 {
            let url = clean_url(info["url"].as_str().unwrap_or_default());
            self.push(
                tab,
                Act::Tab {
                    url: if ignorable(&url) { String::new() } else { url },
                    title: String::new(),
                },
            );
        }
    }

    fn take_in(&mut self, event: &Value) {
        let method = event["method"].as_str().unwrap_or_default();
        let session = event["sessionId"].as_str().map(str::to_string);
        let params = &event["params"];
        let here = session.as_ref().and_then(|s| self.attached.get(s)).cloned();
        match method {
            "Target.targetCreated" => {
                let info = &params["targetInfo"];
                let target = info["targetId"].as_str().unwrap_or_default();
                if info["type"] == "page" && !self.known(target) {
                    if let Ok(attached) = self.cdp.send(
                        None,
                        "Target.attachToTarget",
                        json!({"targetId": target, "flatten": true}),
                    ) {
                        if let Some(child) = attached["sessionId"].as_str() {
                            self.new_tab(child.to_string(), info, false);
                        }
                    }
                }
            }
            // Attached on its own, waiting: a frame of another site, a
            // worker, or a page a page opened.
            "Target.attachedToTarget" if session.is_some() => {
                let child = params["sessionId"].as_str().unwrap_or_default().to_string();
                let info = &params["targetInfo"];
                let target = info["targetId"].as_str().unwrap_or_default();
                let waiting = params["waitingForDebugger"] == true;
                match (info["type"].as_str(), &here) {
                    (Some("iframe"), Some(parent)) => {
                        self.attached.insert(
                            child.clone(),
                            Attached {
                                tab: parent.tab,
                                frame: true,
                                main_frame: None,
                                url: String::new(),
                            },
                        );
                        self.instrument(&child, waiting);
                    }
                    (Some("page"), _) if !self.known(target) => {
                        self.new_tab(child, info, waiting);
                    }
                    _ => {
                        if waiting {
                            let _ = self.cdp.send(
                                Some(&child),
                                "Runtime.runIfWaitingForDebugger",
                                json!({}),
                            );
                        }
                    }
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(child) = params["sessionId"].as_str() {
                    self.attached.remove(child);
                }
            }
            "Target.targetInfoChanged" => {
                let info = &params["targetInfo"];
                let target = info["targetId"].as_str().unwrap_or_default();
                if let Some(at) = self.tabs.iter().position(|t| t == target) {
                    let title = cut(info["title"].as_str().unwrap_or_default(), MAX_TEXT);
                    let url = clean_url(info["url"].as_str().unwrap_or_default());
                    self.retitle(at + 1, &url, &title);
                }
            }
            "Page.frameNavigated" => {
                let frame = &params["frame"];
                if let (Some(page), Some(session)) = (&here, &session) {
                    if !page.frame && frame.get("parentId").is_none() {
                        let url = frame["url"].as_str().unwrap_or_default().to_string();
                        if let Some(attached) = self.attached.get_mut(session) {
                            attached.main_frame = frame["id"].as_str().map(str::to_string);
                            attached.url = url.clone();
                        }
                        self.navigated(page.tab, &url, false);
                    }
                }
            }
            "Page.navigatedWithinDocument" => {
                if let (Some(page), Some(session)) = (&here, &session) {
                    let main = page.main_frame.as_deref() == params["frameId"].as_str();
                    let url = params["url"].as_str().unwrap_or_default().to_string();
                    let moved = url.split('#').next() != page.url.split('#').next();
                    if !page.frame && main && moved {
                        if let Some(attached) = self.attached.get_mut(session) {
                            attached.url = url.clone();
                        }
                        self.navigated(page.tab, &url, true);
                    }
                }
            }
            "Page.javascriptDialogOpening" => {
                if let Some(page) = &here {
                    let kind = params["type"].as_str().unwrap_or("alert").to_string();
                    if kind != "beforeunload" {
                        self.push(
                            page.tab,
                            Act::Dialog {
                                kind,
                                message: cut(
                                    params["message"].as_str().unwrap_or_default(),
                                    MAX_TEXT * 2,
                                ),
                                accepted: None,
                                answer: None,
                            },
                        );
                    }
                }
            }
            "Page.javascriptDialogClosed" => {
                if let Some(page) = &here {
                    let tab = page.tab;
                    if let Some(Act::Dialog { accepted, .. }) = self
                        .steps
                        .iter_mut()
                        .rev()
                        .find(|s| s.tab == tab && matches!(s.act, Act::Dialog { .. }))
                        .map(|s| &mut s.act)
                    {
                        // What was written in a prompt is not kept: it
                        // may be a code or a password.
                        if accepted.is_none() {
                            *accepted = Some(params["result"] == true);
                        }
                    }
                }
            }
            "Browser.downloadWillBegin" => {
                let frame = params["frameId"].as_str();
                let tab = self
                    .attached
                    .values()
                    .find(|a| frame.is_some() && a.main_frame.as_deref() == frame)
                    .map(|a| a.tab)
                    .or_else(|| self.steps.last().map(|s| s.tab))
                    .unwrap_or(1);
                let file = cut(
                    params["suggestedFilename"].as_str().unwrap_or_default(),
                    MAX_TEXT,
                );
                self.push(tab, Act::Download { file });
            }
            "Runtime.bindingCalled" if params["name"] == BINDING => {
                let payload = params["payload"]
                    .as_str()
                    .and_then(|p| serde_json::from_str::<Value>(p).ok())
                    .unwrap_or_default();
                let Some(page) = &here else { return };
                if payload["act"] == "title" {
                    if !page.frame {
                        let url = clean_url(payload["url"].as_str().unwrap_or_default());
                        let title = cut(payload["title"].as_str().unwrap_or_default(), MAX_TEXT);
                        self.retitle(page.tab, &url, &title);
                    }
                } else if let Some(act) = act_of(&payload) {
                    self.push(page.tab, act);
                }
            }
            _ => {}
        }
    }

    /// A page of tab `tab` at `url`: a new step, or the step before's
    /// address corrected when it was a redirect.
    fn navigated(&mut self, tab: usize, url: &str, within: bool) {
        let url = clean_url(url);
        if ignorable(&url) {
            return;
        }
        let now = self.ms();
        let last = self.steps.iter_mut().rev().find(|s| s.tab == tab);
        let led = match last {
            Some(step) => {
                let recent = now.saturating_sub(step.at_ms);
                match &mut step.act {
                    Act::Open { url: u, title } | Act::Arrive { url: u, title }
                        if recent < REDIRECT_MS =>
                    {
                        *u = url;
                        title.clear();
                        return;
                    }
                    Act::Tab { url: u, title } if recent < LED_MS || u.is_empty() => {
                        *u = url;
                        title.clear();
                        return;
                    }
                    act => act.is_action() && recent < LED_MS,
                }
            }
            None => false,
        };
        // A page that changed its address by itself, with no step of the
        // person's before it, is not a step.
        if within && !led {
            return;
        }
        self.push(
            tab,
            if led {
                Act::Arrive {
                    url,
                    title: String::new(),
                }
            } else {
                Act::Open {
                    url,
                    title: String::new(),
                }
            },
        );
    }

    /// The title of tab `tab`'s page at `url`, as it last was.
    fn retitle(&mut self, tab: usize, url: &str, title: &str) {
        // Until a page gives a title, the browser shows an address, which
        // may hold what the address bar was given: not a title.
        let address = title.contains("://")
            || (!title.contains(' ') && (title.contains('/') || title.contains('.')));
        if title.is_empty() || address {
            return;
        }
        let page = self
            .steps
            .iter_mut()
            .rev()
            .filter(|s| s.tab == tab)
            .find(|s| {
                matches!(
                    s.act,
                    Act::Open { .. } | Act::Arrive { .. } | Act::Tab { .. }
                )
            });
        if let Some(step) = page {
            if let Act::Open { url: u, title: t }
            | Act::Arrive { url: u, title: t }
            | Act::Tab { url: u, title: t } = &mut step.act
            {
                if u == url {
                    *t = title.to_string();
                }
            }
        }
    }
}

/// `text` quoted as the draft quotes names and values.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// An element as a step names it: `the button "Sign in"`.
fn element(role: &str, name: &str) -> String {
    let role = match role {
        "" | "generic" | "none" | "presentation" => "",
        other => other,
    };
    match (role.is_empty(), name.is_empty()) {
        (true, true) => "the element".into(),
        (true, false) => quoted(name),
        (false, true) => format!("the {role} (it has no name)"),
        (false, false) => format!("the {role} {}", quoted(name)),
    }
}

fn page(url: &str, title: &str) -> String {
    if title.is_empty() {
        url.to_string()
    } else {
        format!("{} ({url})", quoted(title))
    }
}

/// What a step did, in a line: as the recording is shown while it goes on.
pub fn said(step: &Step) -> String {
    match &step.act {
        Act::Open { url, title } => format!("opened {}", page(url, title)),
        Act::Arrive { url, title } => format!("arrived at {}", page(url, title)),
        Act::Tab { url, title } => format!("a new tab: {}", page(url, title)),
        Act::Click {
            role,
            name,
            checked,
        } => match checked {
            Some(true) => format!("checked {}", element(role, name)),
            Some(false) => format!("unchecked {}", element(role, name)),
            None => format!("clicked {}", element(role, name)),
        },
        Act::Type {
            role,
            name,
            value,
            password,
            login,
            ..
        } => match value {
            Some(value) => format!("typed {} into {}", quoted(value), element(role, name)),
            None if *password => format!(
                "filled a password into {} (not recorded)",
                element(role, name)
            ),
            None if *login => format!(
                "filled a user name into {} (not recorded)",
                element(role, name)
            ),
            None => format!(
                "filled a secret into {} (not recorded)",
                element(role, name)
            ),
        },
        Act::Choose { role, name, option } => {
            format!("chose {} in {}", quoted(option), element(role, name))
        }
        Act::Files { role, name, files } => {
            format!("picked {} for {}", files.join(", "), element(role, name))
        }
        Act::Press { key, .. } => format!("pressed {key}"),
        Act::Dialog {
            kind,
            message,
            accepted,
            ..
        } => format!(
            "the page asked {} ({kind}): {}",
            quoted(message),
            match accepted {
                Some(true) => "OK",
                Some(false) => "Cancel",
                None => "not answered",
            }
        ),
        Act::Download { file } => format!("downloaded {file}"),
    }
}

/// The hosts a recording's pages were on, in the order first seen.
pub fn hosts(recording: &Recording) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for step in &recording.steps {
        if let Act::Open { url, .. } | Act::Arrive { url, .. } | Act::Tab { url, .. } = &step.act {
            if let Some(host) = crate::permissions::host_of(url) {
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
        }
    }
    hosts
}

/// A draft `SKILL.md` of `recording`, named `name`, and described by
/// `description` or, without one, by where it went.
pub fn draft(
    name: &str,
    description: Option<&str>,
    recording: &Recording,
) -> Result<String, String> {
    if !crate::skills::valid_name(name) {
        return Err(format!(
            "{name:?} is not a valid skill name: 1 to 64 lowercase letters, digits and single hyphens"
        ));
    }
    let steps = &recording.steps;
    if steps.is_empty() {
        return Err("nothing was recorded".into());
    }
    let hosts = hosts(recording);
    let day = recording.started.get(..10).unwrap_or_default();
    let first_title = steps.iter().find_map(|s| match &s.act {
        Act::Open { title, .. } | Act::Arrive { title, .. } if !title.is_empty() => {
            Some(title.as_str())
        }
        _ => None,
    });
    let description = match description.map(str::trim).filter(|d| !d.is_empty()) {
        Some(given) => cut(given, crate::skills::MAX_DESCRIPTION),
        None => cut(
            &format!(
                "Repeat in the browser a task shown on {day}{}{}.",
                if hosts.is_empty() {
                    String::new()
                } else {
                    format!(" on {}", hosts.join(", "))
                },
                first_title
                    .map(|t| format!(", starting from {}", quoted(t)))
                    .unwrap_or_default()
            ),
            crate::skills::MAX_DESCRIPTION,
        ),
    };

    // The steps, in words.
    let mut lines: Vec<String> = Vec::new();
    let mut values: Vec<(usize, String, String)> = Vec::new();
    let mut logins: Vec<String> = Vec::new();
    let mut dialogs = false;
    let mut current_tab = steps[0].tab;
    let mut last_page: BTreeMap<usize, (String, String)> = BTreeMap::new();
    let mut i = 0;
    while i < steps.len() {
        let step = &steps[i];
        let next = steps.get(i + 1).filter(|n| n.tab == step.tab);
        let mut used = 1;
        if step.tab != current_tab && !matches!(step.act, Act::Tab { .. }) {
            if let Some((url, title)) = last_page.get(&step.tab) {
                lines.push(format!(
                    "Go back to {} (browser_back, or open its address).",
                    page(url, title)
                ));
            }
        }
        current_tab = step.tab;
        let mut line = match &step.act {
            Act::Open { url, title } => {
                last_page.insert(step.tab, (url.clone(), title.clone()));
                format!("Open {}.", page(url, title))
            }
            Act::Arrive { url, title } => {
                last_page.insert(step.tab, (url.clone(), title.clone()));
                format!("You are on {}.", page(url, title))
            }
            Act::Tab { url, title } => {
                last_page.insert(step.tab, (url.clone(), title.clone()));
                if url.is_empty() {
                    "A new tab opened; go on there.".to_string()
                } else {
                    format!(
                        "This opened {} in a new tab; your browser opens it in the same one.",
                        page(url, title)
                    )
                }
            }
            Act::Click {
                role,
                name,
                checked,
            } => match checked {
                Some(true) => format!("Check {}.", element(role, name)),
                Some(false) => format!("Uncheck {}.", element(role, name)),
                None => format!("Click {}.", element(role, name)),
            },
            Act::Type {
                role,
                name,
                value,
                password,
                login,
                origin,
                ..
            } => {
                let field = element(role, name);
                let mut said = match value {
                    Some(value) if value.is_empty() => format!("Clear {field}"),
                    Some(value) => {
                        values.push((lines.len() + 1, value.clone(), field.clone()));
                        format!("Type {} into {field}", quoted(value))
                    }
                    None if *login => {
                        if !logins.contains(origin) {
                            logins.push(origin.clone());
                        }
                        format!(
                            "Fill the user name of the login saved for {origin} into {field} \
                             (browser_fill_login, part username)"
                        )
                    }
                    None if *password => {
                        if !logins.contains(origin) {
                            logins.push(origin.clone());
                        }
                        format!(
                            "Fill the password of the login saved for {origin} into {field} \
                             (browser_fill_login, part password; it was not recorded)"
                        )
                    }
                    None => format!(
                        "Type into {field} the secret the task gives (a code, a card's \
                         number: it was not recorded, so stop and ask if the task gives none)"
                    ),
                };
                if let Some(Act::Press { key, .. }) = next.map(|n| &n.act) {
                    if key == "Enter" {
                        said.push_str(", and press Enter");
                        used = 2;
                    }
                }
                said.push('.');
                said
            }
            Act::Choose { role, name, option } => {
                format!("Choose {} in {}.", quoted(option), element(role, name))
            }
            Act::Files { role, name, files } => format!(
                "Choose the file{} {} for {} (browser_choose_files, from the workspace).",
                if files.len() == 1 { "" } else { "s" },
                files
                    .iter()
                    .map(|f| quoted(f))
                    .collect::<Vec<_>>()
                    .join(", "),
                element(role, name)
            ),
            Act::Press { key, role, name } => {
                if role.is_empty() || role == "generic" {
                    format!("Press {key}.")
                } else {
                    format!("Press {key} in {}.", element(role, name))
                }
            }
            Act::Dialog {
                kind,
                message,
                accepted,
                ..
            } => {
                dialogs = true;
                let answered: &str = match accepted {
                    Some(true) if kind == "prompt" => {
                        "it was answered (what was written was not recorded)"
                    }
                    Some(true) => "it was answered OK",
                    Some(false) => "it was answered Cancel",
                    None => "it was not answered",
                };
                format!("The page asks {} ({kind}): {answered}.", quoted(message))
            }
            Act::Download { file } => format!(
                "The file {} downloads (into the workspace's downloads/).",
                quoted(file)
            ),
        };
        // What the step led to: the page it opened.
        if let Some(Act::Arrive { url, title }) = steps
            .get(i + used)
            .filter(|n| n.tab == step.tab)
            .map(|n| &n.act)
        {
            if step.act.is_action() {
                last_page.insert(step.tab, (url.clone(), title.clone()));
                line.push_str(&format!(" It opens {}.", page(url, title)));
                used += 1;
            }
        }
        lines.push(line);
        i += used;
    }

    let title = {
        let words = name.replace('-', " ");
        let mut chars = words.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
            None => words,
        }
    };
    let shown_hosts: Vec<&String> = hosts.iter().take(10).collect();
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {name}\n"));
    out.push_str(&format!("description: {}\n", quoted(&description)));
    if !shown_hosts.is_empty() {
        out.push_str(&format!(
            "compatibility: {}\n",
            quoted(&format!(
                "a browser (--browser) that reaches {}",
                shown_hosts
                    .iter()
                    .map(|h| h.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        ));
    }
    out.push_str("metadata:\n");
    out.push_str(&format!("  sion-taught: {}\n", quoted(&recording.started)));
    out.push_str("---\n\n");
    out.push_str(&format!("# {title}\n\n"));
    out.push_str(&format!(
        "Shown once in a browser on {day} ({} step{}, {} min) and written down as it was \
         done. Do it with your browser tools: before each step, take a snapshot \
         (browser_snapshot) and find the element by its role and name, as the step gives \
         them. Pages change: if an element is not there, scroll and take another snapshot; if \
         it is still not there, or a page says something the steps do not expect, stop and say \
         at which step.\n\n",
        lines.len(),
        if lines.len() == 1 { "" } else { "s" },
        recording.seconds.div_ceil(60).max(1)
    ));
    let mut before = Vec::new();
    if !shown_hosts.is_empty() {
        before.push(format!(
            "Your browser must reach {} (the profile's --browser-host).",
            shown_hosts
                .iter()
                .map(|h| h.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for origin in &logins {
        before.push(format!(
            "A login saved for {origin} (sion agent login add <name> --origin {origin}), given \
             to the profile (--login): the password and the user name were not recorded."
        ));
    }
    if dialogs {
        before.push(
            "Your browser answers Cancel to the questions pages ask (confirm, prompt): a step \
             that needs another answer may not go as shown; if it does not, say so."
                .into(),
        );
    }
    if !before.is_empty() {
        out.push_str("## Before you start\n\n");
        for line in before {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    out.push_str("## Steps\n\n");
    for (n, line) in lines.iter().enumerate() {
        out.push_str(&format!("{}. {line}\n", n + 1));
    }
    if recording.cut {
        out.push_str(
            "\nThe recording stopped at its limit: the task may go on after the last step.\n",
        );
    }
    if !values.is_empty() {
        out.push_str(
            "\n## Values typed\n\nThese were typed while it was shown. Where the task gives \
             others, use those; where it gives none, ask before using these.\n\n",
        );
        for (step, value, field) in &values {
            out.push_str(&format!("- {} into {field} (step {step})\n", quoted(value)));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(tab: usize, act: Act) -> Step {
        Step { at_ms: 0, tab, act }
    }

    #[test]
    fn addresses_keep_no_secrets() {
        assert_eq!(
            clean_url("https://ann:pw@shop.example/orders?page=2&token=abc&q=lamp#top"),
            "https://shop.example/orders?page=2&token=…&q=lamp#top"
        );
        assert_eq!(
            clean_url("https://id.example/cb?code=xyz&state=s1#access_token=t"),
            "https://id.example/cb?code=…&state=…"
        );
        let long = format!("https://a.example/?v={}", "x".repeat(80));
        assert_eq!(clean_url(&long), "https://a.example/?v=…");
        assert!(
            ignorable("about:blank")
                && ignorable("chrome://newtab/")
                && !ignorable("https://a.example/")
        );
    }

    #[test]
    fn addresses_and_texts_lose_what_identifies_the_person() {
        assert_eq!(
            clean_url("https://id.example/signin?login_hint=ann%40x.org&email=ann%40x.org&lang=en"),
            "https://id.example/signin?login_hint=…&email=…&lang=en"
        );
        assert_eq!(
            clean_url(
                "https://shop.example/reset/8f14e45fceea167a5a36dedd4bea2543/a1B2c3D4e5F6g7H8"
            ),
            "https://shop.example/reset/…/…"
        );
        assert_eq!(
            clean_url("https://shop.example/how-to-export-invoices-in-2026"),
            "https://shop.example/how-to-export-invoices-in-2026"
        );
        assert_eq!(
            cut("Inbox – ann.lee@gmail.com – Gmail", 100),
            "Inbox – (an email address) – Gmail"
        );
    }

    #[test]
    fn a_user_name_typed_before_a_password_is_not_kept() {
        let typed = |name: &str, value: Option<&str>, password: bool| Step {
            at_ms: 0,
            tab: 1,
            act: Act::Type {
                role: "textbox".into(),
                name: name.into(),
                value: value.map(String::from),
                secret: password,
                password,
                login: false,
                origin: "https://id.example".into(),
            },
        };
        // Asked for the name, then on the next page for the password.
        let mut steps = vec![
            typed("Email or phone", Some("ann@x.org"), false),
            step(
                1,
                Act::Click {
                    role: "button".into(),
                    name: "Next".into(),
                    checked: None,
                },
            ),
            step(
                1,
                Act::Arrive {
                    url: "https://id.example/pw".into(),
                    title: String::new(),
                },
            ),
        ];
        forget_user_name(&mut steps, 1, "https://id.example");
        assert!(matches!(
            &steps[0].act,
            Act::Type {
                value: None,
                login: true,
                ..
            }
        ));
        // Another origin's field is left as it was.
        let mut steps = vec![typed("Search", Some("lamps"), false)];
        forget_user_name(&mut steps, 1, "https://other.example");
        assert!(matches!(&steps[0].act, Act::Type { value: Some(_), .. }));
    }

    #[test]
    fn a_secret_fields_value_is_never_kept() {
        let act = act_of(
            &json!({"act": "type", "role": "textbox", "name": "Password",
                                 "secret": true, "password": true, "value": "hunter2"}),
        )
        .unwrap();
        assert_eq!(
            act,
            Act::Type {
                role: "textbox".into(),
                name: "Password".into(),
                value: None,
                secret: true,
                password: true,
                login: false,
                origin: String::new(),
            }
        );
        let act = act_of(&json!({"act": "type", "role": "textbox", "name": "Email",
                                 "login": true, "value": "ann@example.org"}))
        .unwrap();
        assert!(!serde_json::to_string(&act).unwrap().contains("ann@"));
        assert!(act_of(&json!({"act": "steal"})).is_none());
    }

    #[test]
    fn a_recording_is_written_down_as_steps() {
        let recording = Recording {
            started: "2026-11-04T10:00:00Z".into(),
            seconds: 95,
            cut: false,
            steps: vec![
                step(
                    1,
                    Act::Open {
                        url: "https://shop.example/login".into(),
                        title: "Sign in".into(),
                    },
                ),
                step(
                    1,
                    Act::Type {
                        role: "textbox".into(),
                        name: "Email".into(),
                        value: None,
                        secret: false,
                        password: false,
                        login: true,
                        origin: "https://shop.example".into(),
                    },
                ),
                step(
                    1,
                    Act::Type {
                        role: "textbox".into(),
                        name: "Password".into(),
                        value: None,
                        secret: true,
                        password: true,
                        login: false,
                        origin: "https://shop.example".into(),
                    },
                ),
                step(
                    1,
                    Act::Press {
                        key: "Enter".into(),
                        role: "textbox".into(),
                        name: "Password".into(),
                    },
                ),
                step(
                    1,
                    Act::Arrive {
                        url: "https://shop.example/orders".into(),
                        title: "Orders".into(),
                    },
                ),
                step(
                    1,
                    Act::Type {
                        role: "searchbox".into(),
                        name: "Order number".into(),
                        value: Some("A-1042".into()),
                        secret: false,
                        password: false,
                        login: false,
                        origin: "https://shop.example".into(),
                    },
                ),
                step(
                    1,
                    Act::Press {
                        key: "Enter".into(),
                        role: "searchbox".into(),
                        name: "Order number".into(),
                    },
                ),
                step(
                    1,
                    Act::Click {
                        role: "checkbox".into(),
                        name: "Paid only".into(),
                        checked: Some(true),
                    },
                ),
                step(
                    1,
                    Act::Click {
                        role: "button".into(),
                        name: "Export".into(),
                        checked: None,
                    },
                ),
                step(
                    1,
                    Act::Dialog {
                        kind: "confirm".into(),
                        message: "Export 2 invoices?".into(),
                        accepted: Some(true),
                        answer: None,
                    },
                ),
                step(
                    1,
                    Act::Download {
                        file: "invoices.csv".into(),
                    },
                ),
                step(
                    1,
                    Act::Click {
                        role: "link".into(),
                        name: "Invoices".into(),
                        checked: None,
                    },
                ),
                step(
                    1,
                    Act::Arrive {
                        url: "https://shop.example/invoices".into(),
                        title: "Invoices".into(),
                    },
                ),
            ],
        };
        let text = draft(
            "export-invoices",
            Some("Export the paid invoices of an order."),
            &recording,
        )
        .unwrap();
        let front = crate::skills::front_matter(&text).unwrap();
        assert_eq!(front.name, "export-invoices");
        assert_eq!(front.description, "Export the paid invoices of an order.");
        assert_eq!(front.metadata["sion-taught"], "2026-11-04T10:00:00Z");
        for line in [
            "1. Open \"Sign in\" (https://shop.example/login).",
            "2. Fill the user name of the login saved for https://shop.example into the textbox \"Email\" (browser_fill_login, part username).",
            "3. Fill the password of the login saved for https://shop.example into the textbox \"Password\" (browser_fill_login, part password; it was not recorded), and press Enter. It opens \"Orders\" (https://shop.example/orders).",
            "4. Type \"A-1042\" into the searchbox \"Order number\", and press Enter.",
            "5. Check the checkbox \"Paid only\".",
            "6. Click the button \"Export\".",
            "7. The page asks \"Export 2 invoices?\" (confirm): it was answered OK.",
            "8. The file \"invoices.csv\" downloads (into the workspace's downloads/).",
            "9. Click the link \"Invoices\". It opens \"Invoices\" (https://shop.example/invoices).",
            "- \"A-1042\" into the searchbox \"Order number\" (step 4)",
            "- Your browser must reach shop.example (the profile's --browser-host).",
        ] {
            assert!(text.contains(line), "{line}\n---\n{text}");
        }
        assert!(text.contains("--origin https://shop.example"));
        assert!(text.contains("Your browser answers Cancel"));
        let plain = draft("export-invoices", None, &recording).unwrap();
        assert!(plain.contains(
            "description: \"Repeat in the browser a task shown on 2026-11-04 on shop.example, starting from \\\"Sign in\\\".\""
        ));
        assert!(draft("Export", None, &recording).is_err());
    }
}
