//! Teaching a task for real: Chromium recording what a program playing the
//! person does on a shop served on this machine (signing in, searching,
//! choosing, checking, exporting through a question the page asks, and
//! following a link), and the recording written down as a draft skill.
//! Neither the password nor the login's user name is recorded. Skipped
//! where no Chromium is installed.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sion_agent::browser;
use sion_agent::teach::{draft, Act, Options, Teaching};

const PASSWORD: &str = "pa55-w0rd-7c1e";
const EMAIL: &str = "ann@shop.example";

const LOGIN: &str = r#"<html><head><title>Sign in</title></head><body>
<form method="post" action="/orders">
<label>Email <input id="user" name="user" type="email"></label>
<label>Password <input id="pass" name="pass" type="password"></label>
<button type="submit">Sign in</button>
</form></body></html>"#;

const ORDERS: &str = r#"<html><head><title>Orders</title></head><body>
<h1>Orders</h1>
<input id="q" type="search" placeholder="Order number">
<select id="period" aria-label="Period"><option>This month</option><option>Last month</option></select>
<label><input id="paid" type="checkbox"> Paid only</label>
<button id="export">Export</button>
<p>Nothing to see here.</p>
<a id="invoices" href="/invoices">Invoices</a>
<script>
document.getElementById('export').addEventListener('click', () => setTimeout(() => {
  if (confirm('Export 2 invoices?')) location.href = '/export.csv';
}, 50));
</script></body></html>"#;

/// The shop, on this machine.
fn shop() -> u16 {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let html = |body: &str| {
                tiny_http::Response::from_string(body).with_header(
                    "Content-Type: text/html; charset=utf-8"
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                )
            };
            let path = request
                .url()
                .split('?')
                .next()
                .unwrap_or_default()
                .to_string();
            let _ = match path.as_str() {
                "/login" => request.respond(html(LOGIN)),
                "/orders" => request.respond(html(ORDERS)),
                "/invoices" => request.respond(html(
                    "<html><head><title>Invoices</title></head><body><h1>Invoices</h1></body></html>",
                )),
                "/export.csv" => request.respond(
                    tiny_http::Response::from_string("order,total\nA-1042,85\n")
                        .with_header("Content-Type: text/csv".parse::<tiny_http::Header>().unwrap())
                        .with_header(
                            "Content-Disposition: attachment; filename=\"invoices.csv\""
                                .parse::<tiny_http::Header>()
                                .unwrap(),
                        ),
                ),
                _ => request.respond(tiny_http::Response::from_string("not found").with_status_code(404)),
            };
        }
    });
    port
}

/// Takes in what happens until `done` holds, at most 20 s.
fn until(teaching: &mut Teaching, what: &str, done: impl Fn(&[sion_agent::teach::Step]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(teaching.steps()) {
        assert!(
            Instant::now() < deadline,
            "no {what}: {:#?}",
            teaching.steps()
        );
        assert!(
            teaching.pump(Duration::from_millis(100)),
            "the browser went away"
        );
    }
}

fn evaluate(teaching: &Teaching, expression: &str) -> Value {
    teaching
        .send_to_tab(
            1,
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true}),
        )
        .unwrap()["result"]["value"]
        .clone()
}

/// Waits until the page at `path` has loaded.
fn loaded(teaching: &mut Teaching, path: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let ready = format!("location.pathname === {path:?} && document.readyState === 'complete'");
    while evaluate(teaching, &ready) != json!(true) {
        assert!(Instant::now() < deadline, "{path} did not load");
        teaching.pump(Duration::from_millis(50));
    }
}

/// Clicks the element `selector` names, as a person's pointer does.
fn click(teaching: &Teaching, selector: &str) {
    let at = evaluate(
        teaching,
        &format!(
            "(() => {{ const r = document.querySelector({selector:?}).getBoundingClientRect(); \
             return [r.x + r.width / 2, r.y + r.height / 2]; }})()"
        ),
    );
    let (x, y) = (at[0].as_f64().unwrap(), at[1].as_f64().unwrap());
    for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
        teaching
            .send_to_tab(
                1,
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1}),
            )
            .unwrap();
    }
}

fn type_text(teaching: &Teaching, text: &str) {
    teaching
        .send_to_tab(1, "Input.insertText", json!({"text": text}))
        .unwrap();
}

fn key(teaching: &Teaching, key: &str, code: u32, text: Option<&str>) {
    let mut down =
        json!({"type": "keyDown", "key": key, "code": key, "windowsVirtualKeyCode": code});
    if let Some(text) = text {
        down["text"] = json!(text);
    }
    teaching
        .send_to_tab(1, "Input.dispatchKeyEvent", down)
        .unwrap();
    teaching
        .send_to_tab(
            1,
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": key, "code": key, "windowsVirtualKeyCode": code}),
        )
        .unwrap();
}

fn path_of(url: &str) -> String {
    url.splitn(4, '/')
        .nth(3)
        .map(|p| format!("/{p}"))
        .unwrap_or_default()
}

#[test]
fn a_task_shown_in_the_browser_is_written_down_as_a_skill() {
    let Some(program) = browser::find_program() else {
        eprintln!("skipped: no Chromium");
        return;
    };
    let port = shop();
    let mut teaching = Teaching::start(&Options {
        program,
        start: Some(format!("http://127.0.0.1:{port}/login")),
        limit: Duration::from_secs(120),
        // SION_TEACH_HEADED=1 opens a window, as for a person (on a display).
        headless: std::env::var_os("SION_TEACH_HEADED").is_none(),
        // As root, as in a container, Chromium's own sandbox does not start.
        own_sandbox: false,
        size: (1000, 800),
    })
    .unwrap();
    loaded(&mut teaching, "/login");

    // Signing in: the user name and the password typed, then Enter.
    click(&teaching, "#user");
    type_text(&teaching, EMAIL);
    click(&teaching, "#pass");
    type_text(&teaching, PASSWORD);
    key(&teaching, "Enter", 13, Some("\r"));
    until(&mut teaching, "arrival at the orders", |steps| {
        steps
            .iter()
            .any(|s| matches!(&s.act, Act::Arrive { url, .. } if url.ends_with("/orders")))
    });
    loaded(&mut teaching, "/orders");

    // Searching, choosing, checking, and plain text clicked: not a step.
    click(&teaching, "#q");
    type_text(&teaching, "A-1042");
    key(&teaching, "Enter", 13, Some("\r"));
    evaluate(&teaching, "document.getElementById('period').focus()");
    key(&teaching, "ArrowDown", 40, None);
    click(&teaching, "#paid");
    click(&teaching, "p");
    until(&mut teaching, "the box checked", |steps| {
        steps.iter().any(|s| {
            matches!(
                &s.act,
                Act::Click {
                    checked: Some(true),
                    ..
                }
            )
        })
    });

    // Exporting: the page asks, the person answers OK, the file downloads.
    click(&teaching, "#export");
    until(&mut teaching, "the question", |steps| {
        steps.iter().any(|s| matches!(&s.act, Act::Dialog { .. }))
    });
    teaching
        .send_to_tab(1, "Page.handleJavaScriptDialog", json!({"accept": true}))
        .unwrap();
    until(&mut teaching, "the download", |steps| {
        steps.iter().any(|s| matches!(&s.act, Act::Download { .. }))
    });

    // A link followed.
    loaded(&mut teaching, "/orders");
    click(&teaching, "#invoices");
    until(&mut teaching, "arrival at the invoices", |steps| {
        steps
            .iter()
            .any(|s| matches!(&s.act, Act::Arrive { url, .. } if url.ends_with("/invoices")))
    });
    let recording = teaching.finish();

    let seen: Vec<String> = recording
        .steps
        .iter()
        .map(|s| match &s.act {
            Act::Open { url, .. } => format!("open {}", path_of(url)),
            Act::Arrive { url, .. } => format!("arrive {}", path_of(url)),
            Act::Type {
                role,
                name,
                value,
                secret,
                login,
                ..
            } => {
                format!("type {role} {name} {value:?} secret={secret} login={login}")
            }
            Act::Press { key, name, .. } => format!("press {key} in {name}"),
            Act::Choose { name, option, .. } => format!("choose {option} in {name}"),
            Act::Click {
                role,
                name,
                checked,
            } => format!("click {role} {name} {checked:?}"),
            Act::Dialog {
                kind,
                message,
                accepted,
                ..
            } => {
                format!("dialog {kind} {message} {accepted:?}")
            }
            Act::Download { file } => format!("download {file}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        seen,
        [
            "open /login",
            "type textbox Email None secret=false login=true",
            "type textbox Password None secret=true login=false",
            "press Enter in Password",
            "arrive /orders",
            "type searchbox Order number Some(\"A-1042\") secret=false login=false",
            "press Enter in Order number",
            "choose Last month in Period",
            "click checkbox Paid only Some(true)",
            "click button Export None",
            "dialog confirm Export 2 invoices? Some(true)",
            "download invoices.csv",
            "click link Invoices None",
            "arrive /invoices",
        ],
        "{:#?}",
        recording.steps
    );
    let kept = serde_json::to_string(&recording).unwrap();
    assert!(!kept.contains(PASSWORD) && !kept.contains(EMAIL), "{kept}");
    assert!(!recording.cut);

    let text = draft(
        "export-invoices",
        Some("Export an order's invoices from the shop."),
        &recording,
    )
    .unwrap();
    let front = sion_agent::skills::front_matter(&text).unwrap();
    assert_eq!(front.name, "export-invoices");
    let origin = format!("http://127.0.0.1:{port}");
    assert!(
        text.contains(&format!(
            "3. Fill the password of the login saved for {origin} into the textbox \"Password\" (browser_fill_login, part password; it was not recorded), and press Enter. It opens \"Orders\" ({origin}/orders)."
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("1. Open \"Sign in\" ({origin}/login).")),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "Click the link \"Invoices\". It opens \"Invoices\" ({origin}/invoices)."
        )),
        "{text}"
    );
    assert!(
        text.contains("- Your browser must reach 127.0.0.1"),
        "{text}"
    );
    assert!(!text.contains(PASSWORD) && !text.contains(EMAIL));
}
