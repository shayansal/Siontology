//! A guest that joins a meeting by its link, against stand-in meeting
//! pages served here, with Chromium: it gives its name, turns its
//! microphone off, asks to join, waits to be let in and leaves; a page that
//! refuses it says why. Skipped where no Chromium is found.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sion_cli::meeting_bot::{Bot, Standing};

/// A meeting's page that asks for a name, lets the guest in after a moment
/// in the lobby, and tells the server what the guest did.
const LOBBY: &str = r#"<!doctype html><html><body>
<div id="before">
  <label>Your name <input id="name" placeholder="Your name"></label>
  <button aria-label="Turn off microphone" onclick="tell('mic-off'); this.remove()">mic</button>
  <button onclick="ask()">Ask to join</button>
</div>
<div id="lobby" hidden>Asking to be let in…</div>
<div id="in" hidden><p>In the meeting</p><button onclick="tell('left')">Leave call</button></div>
<script>
function tell(what) { fetch('/told?' + encodeURIComponent(what)); }
function ask() {
  tell('asked:' + document.getElementById('name').value);
  document.getElementById('before').hidden = true;
  document.getElementById('lobby').hidden = false;
  setTimeout(() => { document.getElementById('lobby').hidden = true; document.getElementById('in').hidden = false; }, 1500);
}
</script></body></html>"#;

/// A meeting's page that refuses guests.
const REFUSED: &str = r#"<!doctype html><html><body><h1>You can't join this call</h1>
<p>No one responded to your request to join.</p></body></html>"#;

/// Serves the two pages, and keeps what the pages tell it.
fn serve() -> (String, Arc<Mutex<Vec<String>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr());
    let told = Arc::new(Mutex::new(Vec::new()));
    let kept = told.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let (body, kind) = if url.starts_with("/told?") {
                let what = url
                    .trim_start_matches("/told?")
                    .replace("%3A", ":")
                    .replace("%20", " ");
                kept.lock().unwrap().push(what);
                (String::new(), "text/plain")
            } else if url.starts_with("/refused") {
                (REFUSED.to_string(), "text/html")
            } else {
                (LOBBY.to_string(), "text/html")
            };
            let response = tiny_http::Response::from_string(body).with_header(
                format!("Content-Type: {kind}")
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            );
            let _ = request.respond(response);
        }
    });
    (base, told)
}

#[test]
fn a_guest_gives_its_name_asks_to_join_waits_and_leaves() {
    let Some(program) = sion_agent::browser::find_program() else {
        eprintln!("skipped: no Chromium");
        return;
    };
    let (base, told) = serve();
    // Chromium's own sandbox does not start everywhere (as root, or where
    // unprivileged user namespaces are restricted, as on Ubuntu's CI
    // runners): the tests run it without, against pages served here.
    let own_sandbox = false;
    let bot = Bot::open(
        &program,
        &format!("{base}/meet/abc-defg-hij"),
        None,
        own_sandbox,
    )
    .unwrap();
    let mut said = Vec::new();
    bot.join("Notes (sion)", Duration::from_secs(30), &mut |done| {
        said.push(done.to_string())
    })
    .unwrap();
    assert!(said.contains(&"named".to_string()), "{said:?}");
    assert!(
        said.contains(&"off: Turn off microphone".to_string()),
        "{said:?}"
    );
    assert!(
        said.contains(&"clicked: Ask to join".to_string()),
        "{said:?}"
    );
    assert!(
        said.contains(&"waiting to be let in".to_string()),
        "{said:?}"
    );
    assert!(bot.in_meeting("Notes (sion)"));
    assert_eq!(bot.step("Notes (sion)").unwrap().0, Standing::Joined);
    bot.leave();
    std::thread::sleep(Duration::from_millis(300));
    let told = told.lock().unwrap().clone();
    assert!(told.contains(&"mic-off".to_string()), "{told:?}");
    assert!(told.contains(&"asked:Notes (sion)".to_string()), "{told:?}");
    assert!(told.contains(&"left".to_string()), "{told:?}");

    // A page that refuses the guest: it says why.
    let bot = Bot::open(&program, &format!("{base}/refused"), None, own_sandbox).unwrap();
    let refused = bot
        .join("Notes (sion)", Duration::from_secs(20), &mut |_| {})
        .unwrap_err();
    assert!(
        refused.contains("not let in: You can't join this call"),
        "{refused}"
    );
    bot.leave();
    assert!(Bot::open(&program, "file:///etc/passwd", None, own_sandbox).is_err());
    // A browser that stops as it starts says so, not that it was slow.
    let stopped = Bot::open(
        std::path::Path::new("/bin/false"),
        &format!("{base}/meet/x"),
        None,
        own_sandbox,
    )
    .unwrap_err();
    assert!(
        stopped.starts_with("the browser stopped as it started"),
        "{stopped}"
    );
}
