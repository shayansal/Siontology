//! The browser and the desktop for real: Chromium in bubblewrap, driven over
//! the DevTools protocol, against two sites served on this machine; and a
//! desktop of Xvfb, xdotool and ImageMagick in bubblewrap. Each test is
//! skipped where what it needs is not installed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use sion_agent::browser::{self, Credential, Credentials, Shared};
use sion_agent::permissions::{HostRule, ScriptNetwork};
use sion_agent::sandbox::{Backend, Sandbox};
use sion_agent::tools::{Outcome, Tool};

/// The operator's logins: one, for the shop.
#[derive(Debug)]
struct Logins(BTreeMap<String, Credential>);

impl Credentials for Logins {
    fn get(&self, name: &str) -> Option<Credential> {
        self.0.get(name).cloned()
    }

    fn list(&self) -> Vec<(String, String)> {
        self.0
            .iter()
            .map(|(name, c)| (name.clone(), c.origin.clone()))
            .collect()
    }
}

const PASSWORD: &str = "pa55-w0rd-7c1e";

/// A site on this machine: `pages` by path, and `/session`, which says who
/// signed in.
fn site(pages: Vec<(&'static str, String)>) -> u16 {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let pages: BTreeMap<&str, String> = pages.into_iter().collect();
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let (path, query) = url.split_once('?').unwrap_or((&url, ""));
            let body = if path == "/session" {
                let mut form = String::new();
                let _ = request.as_reader().read_to_string(&mut form);
                let fields = sion_agent::oauth::query(if form.is_empty() { query } else { &form });
                if fields.get("pass").map(String::as_str) == Some(PASSWORD) {
                    format!(
                        "<html><head><title>Account</title></head><body><h1>Signed in as {}</h1></body></html>",
                        fields.get("user").cloned().unwrap_or_default()
                    )
                } else {
                    "<html><body><h1>Wrong password</h1></body></html>".to_string()
                }
            } else {
                match pages.get(path) {
                    Some(page) => page.clone(),
                    None => {
                        let _ = request.respond(
                            tiny_http::Response::from_string("not found").with_status_code(404),
                        );
                        continue;
                    }
                }
            };
            let _ = request.respond(
                tiny_http::Response::from_string(body).with_header(
                    "Content-Type: text/html; charset=utf-8"
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                ),
            );
        }
    });
    port
}

const LOGIN: &str = r#"<html><head><title>Sign in</title></head><body>
<form action="/session" method="get">
  <label>User <input name="user" type="text"></label>
  <label>Password <input name="pass" type="password"></label>
  <button type="submit">Sign in</button>
</form></body></html>"#;

fn bubblewrap() -> Option<Sandbox> {
    let sandbox = Sandbox::new(Backend::Bubblewrap);
    sandbox.probe().ok().map(|()| sandbox)
}

fn call(tools: &[Box<dyn Tool>], name: &str, input: Value) -> Outcome {
    let tool = tools
        .iter()
        .find(|t| t.spec().name == name)
        .unwrap_or_else(|| panic!("no tool {name}"));
    tool.call(&input, "k")
}

/// The reference of the first line of a snapshot that has all of `words`.
fn reference(snapshot: &str, words: &[&str]) -> String {
    let line = snapshot
        .lines()
        .find(|l| words.iter().all(|w| l.contains(w)))
        .unwrap_or_else(|| panic!("no line with {words:?} in:\n{snapshot}"));
    let at = line.find("[ref=").expect("a reference") + 5;
    line[at..].split(']').next().unwrap().to_string()
}

#[test]
fn a_browser_reads_clicks_types_and_fills_a_login_only_where_it_belongs() {
    let (Some(program), Some(sandbox)) = (browser::find_program(), bubblewrap()) else {
        eprintln!("skipped: no Chromium, or bubblewrap does not work here");
        return;
    };
    // The other site: a form of its own, which the shop's page frames.
    let other = site(vec![("/login", LOGIN.to_string())]);
    let shop = site(vec![
        (
            "/",
            "<html><head><title>Shop</title></head><body><h1>Welcome</h1>\
             <a href=\"/login\">Sign in</a></body></html>"
                .to_string(),
        ),
        ("/login", LOGIN.to_string()),
        (
            "/popup",
            "<html><head><title>Popup</title></head><body>\
             <a href=\"/login\" target=\"_blank\">Sign in elsewhere</a>\
             <button onclick=\"alert('Saved!')\">Save</button></body></html>"
                .to_string(),
        ),
        (
            "/framed",
            format!(
                "<html><head><title>Framed</title></head><body><h1>Offers</h1>\
                 <iframe src=\"http://127.0.0.1:{other}/login\" width=600 height=300></iframe></body></html>"
            ),
        ),
    ]);
    let origin = format!("http://127.0.0.1:{shop}");
    let logins = Logins(BTreeMap::from([(
        "shop".to_string(),
        Credential {
            origin: origin.clone(),
            username: "ana".into(),
            password: PASSWORD.into(),
        },
    )]));
    let allowed = ScriptNetwork::Hosts(vec![
        HostRule::parse(&format!("127.0.0.1:{shop}")).unwrap(),
        HostRule::parse(&format!("127.0.0.1:{other}")).unwrap(),
    ]);
    let config = browser::Config {
        program,
        sandbox: Some(sandbox),
        bridge: Some(vec![env!("CARGO_BIN_EXE_sion-sandbox-bridge").to_string()]),
        network: allowed,
        egress: sion_agent::egress::Options {
            allow_private: true,
            upstream: None,
        },
        size: (1024, 768),
        workspace: None,
        confidential: Vec::new(),
    };
    let shared = Shared::new(config.clone(), Some(Arc::new(logins)));
    let tools = shared.tools();
    let fill = tools
        .iter()
        .find(|t| t.spec().name == "browser_fill_login")
        .unwrap();
    assert!(
        fill.spec()
            .description
            .contains(&format!("shop ({origin})")),
        "{}",
        fill.spec().description
    );

    // A page, as its accessibility tree.
    let page = call(&tools, "browser_open", json!({"url": format!("{origin}/")}));
    assert!(!page.is_error, "{}", page.content);
    assert!(page.untrusted, "a page is content from outside");
    assert!(page.content.contains("title \"Shop\""), "{}", page.content);
    assert!(
        page.content.contains("heading \"Welcome\""),
        "{}",
        page.content
    );

    // A click follows the link.
    let link = reference(&page.content, &["link", "Sign in"]);
    let page = call(&tools, "browser_click", json!({"ref": link}));
    assert!(page.content.contains("/login"), "{}", page.content);

    // The login is filled without the model seeing it; the password never
    // comes back, not even in the address the form goes to.
    let user = reference(&page.content, &["textbox", "User"]);
    let pass = reference(&page.content, &["textbox", "Password"]);
    let filled = call(
        &tools,
        "browser_fill_login",
        json!({"ref": user, "login": "shop", "part": "username"}),
    );
    assert!(!filled.is_error, "{}", filled.content);
    let filled = call(
        &tools,
        "browser_fill_login",
        json!({"ref": pass, "login": "shop", "part": "password"}),
    );
    assert!(!filled.is_error, "{}", filled.content);
    let button = reference(&filled.content, &["button", "Sign in"]);
    let page = call(&tools, "browser_click", json!({"ref": button}));
    assert!(
        page.content.contains("Signed in as ana"),
        "{}",
        page.content
    );
    assert!(!page.content.contains(PASSWORD), "{}", page.content);
    assert!(
        page.content.contains("[secret PASSWORD]"),
        "{}",
        page.content
    );

    // Typing, then a key.
    let page = call(&tools, "browser_back", json!({}));
    let user = reference(&page.content, &["textbox", "User"]);
    let typed = call(
        &tools,
        "browser_type",
        json!({"ref": user, "text": "bo", "submit": false}),
    );
    assert!(typed.content.contains("value=\"bo\""), "{}", typed.content);

    // In a page of the shop, a field framed from another site is not given
    // the shop's login.
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/framed")}),
    );
    let framed = reference(&page.content, &["textbox", "Password"]);
    let refused = call(
        &tools,
        "browser_fill_login",
        json!({"ref": framed, "login": "shop", "part": "password"}),
    );
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused
            .content
            .contains(&format!("the field is on http://127.0.0.1:{other}")),
        "{}",
        refused.content
    );

    // A picture of the page.
    let shot = call(&tools, "browser_screenshot", json!({}));
    assert_eq!(shot.pictures.len(), 1);
    assert_eq!(shot.pictures[0].media_type, "image/png");
    assert!(shot.pictures[0].base64.starts_with("iVBORw0KGgo"));

    // The browser keeps one page: a link aimed at another window opens in
    // this one.
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/popup")}),
    );
    let elsewhere_link = reference(&page.content, &["link", "Sign in elsewhere"]);
    let page = call(&tools, "browser_click", json!({"ref": elsewhere_link}));
    assert!(
        page.content.contains(&format!("page {origin}/login")),
        "{}",
        page.content
    );

    // A dialog is dismissed, and the page says what it asked.
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/popup")}),
    );
    let save = reference(&page.content, &["button", "Save"]);
    let page = call(&tools, "browser_click", json!({"ref": save.clone()}));
    assert!(
        page.content
            .contains("the page showed a dialog (alert) saying \"Saved!\": it was dismissed"),
        "{}",
        page.content
    );

    // A browser started again goes back to the page, and keeps the
    // references that still name the same elements.
    let state = shared.state().expect("the browser runs");
    assert_eq!(state.url, format!("{origin}/popup"));
    let again = Shared::new(config.clone(), None).restoring(Some(state));
    let tools_again = again.tools();
    let page = call(&tools_again, "browser_click", json!({"ref": save}));
    assert!(!page.is_error, "{}", page.content);
    assert!(page.content.contains("Saved!"), "{}", page.content);
    drop(tools_again);
    drop(again);

    // A site that is not allowed is not reached.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let elsewhere = closed.local_addr().unwrap().port();
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("http://127.0.0.1:{elsewhere}/")}),
    );
    assert!(
        page.content.contains("is not among the hosts allowed"),
        "{}",
        page.content
    );
}

/// A site with a file to download and forms that take files: `/report`
/// answers with an attachment, `/received` with what a form sent.
fn files_site() -> u16 {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let html = |body: String| {
                tiny_http::Response::from_string(body).with_header(
                    "Content-Type: text/html; charset=utf-8"
                        .parse::<tiny_http::Header>()
                        .unwrap(),
                )
            };
            let response = match url.as_str() {
                "/" => html(
                    "<html><head><title>Reports</title></head><body><h1>Reports</h1>\
                     <a href=\"/report\">Get the report</a></body></html>"
                        .into(),
                ),
                "/report" => tiny_http::Response::from_string("month,total\noctober,42\n")
                    .with_header(
                        "Content-Type: text/csv"
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    )
                    .with_header(
                        "Content-Disposition: attachment; filename=\"report.csv\""
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    ),
                "/upload" => html(
                    "<html><head><title>Upload</title></head><body>\
                     <form method=\"post\" action=\"/received\" enctype=\"multipart/form-data\">\
                     <label>Document <input type=\"file\" name=\"doc\"></label>\
                     <button type=\"submit\">Send</button></form>\
                     <button onclick=\"document.getElementById('hidden').click()\">Attach</button>\
                     <input type=\"file\" id=\"hidden\" style=\"display:none\" \
                      onchange=\"document.title = 'chose ' + this.files[0].name\">\
                     </body></html>"
                        .into(),
                ),
                "/received" => {
                    let mut body = Vec::new();
                    let _ = request.as_reader().read_to_end(&mut body);
                    let text = String::from_utf8_lossy(&body);
                    let name = text
                        .split("filename=\"")
                        .nth(1)
                        .and_then(|rest| rest.split('"').next())
                        .unwrap_or("nothing")
                        .to_string();
                    let content = text
                        .split("\r\n\r\n")
                        .nth(1)
                        .and_then(|rest| rest.split("\r\n--").next())
                        .unwrap_or_default()
                        .to_string();
                    html(format!(
                        "<html><head><title>Received</title></head><body><h1>Received {name}: {content}</h1></body></html>"
                    ))
                }
                _ => html("not found".into()).with_status_code(404),
            };
            let _ = request.respond(response);
        }
    });
    port
}

#[test]
fn a_browser_keeps_tabs_downloads_into_the_workspace_and_chooses_files() {
    let (Some(program), Some(sandbox)) = (browser::find_program(), bubblewrap()) else {
        eprintln!("skipped: no Chromium, or bubblewrap does not work here");
        return;
    };
    let port = files_site();
    let origin = format!("http://127.0.0.1:{port}");
    let workspace = std::env::temp_dir().join(format!("sion-browser-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("notes.txt"), "hello world").unwrap();
    let config = browser::Config {
        program,
        sandbox: Some(sandbox),
        bridge: Some(vec![env!("CARGO_BIN_EXE_sion-sandbox-bridge").to_string()]),
        network: ScriptNetwork::Hosts(vec![HostRule::parse(&format!("127.0.0.1:{port}")).unwrap()]),
        egress: sion_agent::egress::Options {
            allow_private: true,
            upstream: None,
        },
        size: (1024, 768),
        workspace: Some(workspace.clone()),
        confidential: vec!["secret.txt".into()],
    };
    let shared = Shared::new(config.clone(), None);
    let tools = shared.tools();

    // A link to a file downloads it into the workspace.
    let page = call(&tools, "browser_open", json!({"url": format!("{origin}/")}));
    assert!(!page.is_error, "{}", page.content);
    let link = reference(&page.content, &["link", "Get the report"]);
    let page = call(&tools, "browser_click", json!({"ref": link}));
    assert!(
        page.content
            .contains("downloaded downloads/report.csv (23 bytes, from"),
        "{}",
        page.content
    );
    assert!(page.untrusted);
    assert_eq!(
        std::fs::read_to_string(workspace.join("downloads/report.csv")).unwrap(),
        "month,total\noctober,42\n"
    );
    // Opening the file's address downloads it too, under a name of its own.
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/report")}),
    );
    assert!(!page.is_error, "{}", page.content);
    assert!(
        page.content.contains("downloads/report (2).csv"),
        "{}",
        page.content
    );

    // A second tab, current until another is chosen.
    let page = call(
        &tools,
        "browser_tab_open",
        json!({"url": format!("{origin}/upload")}),
    );
    assert!(page.content.starts_with("opened tab 2"), "{}", page.content);
    let tabs = call(&tools, "browser_tabs", json!({}));
    assert!(tabs.content.contains("1. \"Reports\""), "{}", tabs.content);
    assert!(
        tabs.content.contains("2. \"Upload\"") && tabs.content.ends_with("(current)"),
        "{}",
        tabs.content
    );

    // A file of the workspace, chosen for the form's field and sent.
    let field = reference(&page.content, &["Document", "[ref="]);
    let chosen = call(
        &tools,
        "browser_choose_files",
        json!({"ref": field, "paths": ["notes.txt"]}),
    );
    assert!(!chosen.is_error, "{}", chosen.content);
    let send = reference(&chosen.content, &["button", "Send"]);
    let page = call(&tools, "browser_click", json!({"ref": send}));
    assert!(
        page.content.contains("Received notes.txt: hello world"),
        "{}",
        page.content
    );
    // A path out of the workspace is refused.
    let page = call(&tools, "browser_back", json!({}));
    let field = reference(&page.content, &["Document", "[ref="]);
    let refused = call(
        &tools,
        "browser_choose_files",
        json!({"ref": field, "paths": ["../etc/passwd"]}),
    );
    assert!(refused.is_error, "{}", refused.content);
    // So is a confidential file, however its path is written.
    std::fs::write(workspace.join("secret.txt"), "the key").unwrap();
    let refused = call(
        &tools,
        "browser_choose_files",
        json!({"ref": field, "paths": [".//secret.txt"]}),
    );
    assert!(
        refused.is_error && refused.content.contains("is confidential"),
        "{}",
        refused.content
    );
    // A button that opens a file chooser: the chooser is answered.
    let attach = reference(&page.content, &["button", "Attach"]);
    let page = call(
        &tools,
        "browser_choose_files",
        json!({"ref": attach, "paths": ["notes.txt"]}),
    );
    assert!(!page.is_error, "{}", page.content);
    assert!(page.content.contains("chose notes.txt"), "{}", page.content);

    // Back to the first tab, the second closed; the last one stays.
    let page = call(&tools, "browser_tab_switch", json!({"tab": 1}));
    assert!(
        page.content.contains("heading \"Reports\""),
        "{}",
        page.content
    );
    let closed = call(&tools, "browser_tab_close", json!({"tab": 2}));
    assert!(
        closed.content.starts_with("closed tab 2; tab 1 is current"),
        "{}",
        closed.content
    );
    assert!(call(&tools, "browser_tab_close", json!({"tab": 1})).is_error);
    drop(tools);
    drop(shared);

    // Without a workspace: no download is taken, and no file is chosen.
    let bare = Shared::new(
        browser::Config {
            workspace: None,
            ..config
        },
        None,
    );
    let tools = bare.tools();
    assert!(!tools
        .iter()
        .any(|t| t.spec().name == "browser_choose_files"));
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/report")}),
    );
    assert!(
        page.content.contains("takes no downloads"),
        "{}",
        page.content
    );
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn a_desktop_is_seen_and_used_with_the_pointer() {
    let Some(sandbox) = bubblewrap() else {
        eprintln!("skipped: bubblewrap does not work here");
        return;
    };
    for program in ["Xvfb", "xdotool", "import"] {
        if !std::path::Path::new("/usr/bin").join(program).exists() {
            eprintln!("skipped: no {program}");
            return;
        }
    }
    let workspace: PathBuf =
        std::env::temp_dir().join(format!("sion-desktop-ws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).unwrap();
    let computer = sion_agent::desktop::Computer::new(sion_agent::desktop::Config {
        sandbox,
        size: (800, 600),
        programs: Vec::new(),
        bridge: Some(vec![env!("CARGO_BIN_EXE_sion-sandbox-bridge").to_string()]),
        network: ScriptNetwork::None,
        egress: sion_agent::egress::Options::default(),
        workspace: workspace.clone(),
    });
    let shot = computer.call(&json!({"action": "screenshot"}), "k");
    assert!(!shot.is_error, "{}", shot.content);
    assert!(shot.untrusted, "the screen is content from outside");
    assert_eq!(shot.pictures.len(), 1);
    assert!(shot.pictures[0].base64.starts_with("iVBORw0KGgo"));

    let moved = computer.call(
        &json!({"action": "mouse_move", "coordinate": [321, 123]}),
        "k",
    );
    assert!(!moved.is_error, "{}", moved.content);
    let at = computer.call(&json!({"action": "cursor_position"}), "k");
    assert_eq!(at.content, "the pointer is at (321, 123)");

    let off = computer.call(
        &json!({"action": "left_click", "coordinate": [900, 10]}),
        "k",
    );
    assert!(
        off.is_error && off.content.contains("off the screen"),
        "{}",
        off.content
    );
    let none = computer.call(&json!({"action": "launch", "program": "xterm"}), "k");
    assert!(
        none.is_error && none.content.contains("not a program you may start"),
        "{}",
        none.content
    );
    drop(computer);
    let _ = std::fs::remove_dir_all(&workspace);
}
