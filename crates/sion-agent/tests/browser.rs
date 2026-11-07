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
        profile: None,
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
        profile: None,
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
        code: true,
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

/// A program that opens a window named "Ledger" at (40, 50), 300×200,
/// with nothing but the X library, and keeps it open.
const WINDOW: &str = r#"import ctypes, time
x = ctypes.cdll.LoadLibrary("libX11.so.6")
x.XOpenDisplay.restype = ctypes.c_void_p
x.XOpenDisplay.argtypes = [ctypes.c_char_p]
d = x.XOpenDisplay(None)
x.XDefaultRootWindow.restype = ctypes.c_ulong
x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x.XCreateSimpleWindow.restype = ctypes.c_ulong
x.XCreateSimpleWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
    ctypes.c_uint, ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_ulong]
w = x.XCreateSimpleWindow(d, x.XDefaultRootWindow(d), 40, 50, 300, 200, 1, 0, 0xffffff)
x.XStoreName.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_char_p]
x.XStoreName(d, w, b"Ledger")
x.XMapWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x.XMapWindow(d, w)
x.XFlush.argtypes = [ctypes.c_void_p]
x.XFlush(d)
time.sleep(120)
"#;

#[test]
fn a_desktop_is_used_in_code_as_well_as_with_the_pointer() {
    let Some(sandbox) = bubblewrap() else {
        eprintln!("skipped: bubblewrap does not work here");
        return;
    };
    for program in ["Xvfb", "xdotool", "import", "python3", "bash"] {
        if !std::path::Path::new("/usr/bin").join(program).exists() {
            eprintln!("skipped: no {program}");
            return;
        }
    }
    let workspace: PathBuf =
        std::env::temp_dir().join(format!("sion-desktop-code-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).unwrap();
    let config = sion_agent::desktop::Config {
        sandbox,
        size: (800, 600),
        programs: vec![vec![
            "/usr/bin/python3".to_string(),
            "-c".to_string(),
            WINDOW.to_string(),
        ]],
        bridge: Some(vec![env!("CARGO_BIN_EXE_sion-sandbox-bridge").to_string()]),
        network: ScriptNetwork::None,
        egress: sion_agent::egress::Options::default(),
        workspace: workspace.clone(),
        code: true,
    };
    let computer = sion_agent::desktop::Computer::new(config.clone());
    assert!(computer.spec().description.contains("run_code"));
    let code = |language: &str, code: &str, timeout: u64| {
        computer.call(
            &json!({"action": "run_code", "language": language, "code": code, "timeout": timeout}),
            "k",
        )
    };

    // A program of its own: on the desktop's display, in the workspace.
    let ran = code(
        "python",
        "import os\nprint('display', os.environ['DISPLAY'])\nopen('made.txt', 'w').write('in code')",
        30,
    );
    assert!(!ran.is_error, "{}", ran.content);
    assert!(
        ran.content.starts_with("it exited with 0"),
        "{}",
        ran.content
    );
    assert!(
        ran.content.contains("printed:\ndisplay :"),
        "{}",
        ran.content
    );
    assert!(
        ran.untrusted,
        "what a program prints is content from outside"
    );
    assert_eq!(ran.pictures.len(), 1, "it answers with a screenshot too");
    assert_eq!(
        std::fs::read_to_string(workspace.join("made.txt")).unwrap(),
        "in code"
    );
    let ran = code("bash", "xdotool getdisplaygeometry", 30);
    assert!(ran.content.contains("printed:\n800 600"), "{}", ran.content);
    let failed = code(
        "python",
        "import sys\nprint('no', file=sys.stderr)\nsys.exit(3)",
        30,
    );
    assert!(
        failed.content.starts_with("it exited with 3") && failed.content.contains("errors:\nno"),
        "{}",
        failed.content
    );
    let slow = code("bash", "sleep 30", 1);
    assert!(slow.content.contains("ran too long"), "{}", slow.content);
    let unknown = code("ruby", "puts 1", 30);
    assert!(
        unknown.is_error && unknown.content.contains("python or bash"),
        "{}",
        unknown.content
    );

    // Its windows, by their names and places, and one brought forward.
    let none = computer.call(&json!({"action": "windows"}), "k");
    assert_eq!(none.content, "no window is open");
    let started = computer.call(&json!({"action": "launch", "program": "python3"}), "k");
    assert!(!started.is_error, "{}", started.content);
    let mut listed = String::new();
    for _ in 0..50 {
        listed = computer.call(&json!({"action": "windows"}), "k").content;
        if listed.contains("Ledger") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let line = listed
        .lines()
        .find(|l| l.contains("\"Ledger\""))
        .unwrap_or_else(|| panic!("no window named Ledger in:\n{listed}"));
    assert!(line.ends_with("at (40, 50), 300×200"), "{line}");
    let id: u64 = line
        .trim_start_matches("window ")
        .split(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let focused = computer.call(&json!({"action": "focus", "window": id}), "k");
    assert_eq!(focused.content, format!("focused window {id}, \"Ledger\""));
    let has = code("bash", "xdotool getwindowfocus", 30);
    assert!(
        has.content.contains(&format!("printed:\n{id}")),
        "{}",
        has.content
    );
    let missing = computer.call(&json!({"action": "focus", "window": 7}), "k");
    assert!(missing.is_error, "{}", missing.content);
    assert_eq!(
        computer.effect_of(&json!({"action": "windows"})),
        sion_agent::tools::Effect::Read
    );
    assert_eq!(
        computer.effect_of(&json!({"action": "run_code"})),
        sion_agent::tools::Effect::External
    );
    drop(computer);

    // A desktop used only with the pointer and the keyboard runs no code.
    let gui = sion_agent::desktop::Computer::new(sion_agent::desktop::Config {
        code: false,
        ..config
    });
    assert!(!gui.spec().description.contains("run_code"));
    let refused = gui.call(
        &json!({"action": "run_code", "language": "bash", "code": "true"}),
        "k",
    );
    assert!(
        refused.is_error && refused.content.contains("not an action"),
        "{}",
        refused.content
    );
    drop(gui);
    let _ = std::fs::remove_dir_all(&workspace);
}

/// A page with what the accessibility tree says little of: a board drawn on
/// a canvas, and a field for a code, each at a place of its own.
const POINTER: &str = r#"<html><head><title>Pointer</title></head><body style="margin:0">
<canvas id="board" width="200" height="100" style="position:absolute;left:100px;top:100px;background:#ddd"></canvas>
<input id="code" aria-label="Code" style="position:absolute;left:100px;top:300px;width:200px;height:30px">
<p id="out" style="position:absolute;left:100px;top:400px">nothing yet</p>
<p id="typed" style="position:absolute;left:100px;top:440px">typed nothing</p>
<script>
document.getElementById('board').addEventListener('click', e => {
  document.getElementById('out').textContent = 'clicked the board at ' + e.clientX + ',' + e.clientY;
});
document.getElementById('code').addEventListener('input', e => {
  document.getElementById('typed').textContent = 'typed ' + e.target.value;
});
</script></body></html>"#;

fn over_http(port: u16, request: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);
    String::from_utf8_lossy(&answer).into_owned()
}

/// The person a page is handed to: they open the address they are given,
/// see the page, click into the field, type the code sent to them and
/// reply. What they were asked, and each answer of the page they used.
#[derive(Debug, Default)]
struct Person {
    asked: std::sync::Mutex<Vec<String>>,
    answers: std::sync::Mutex<Vec<String>>,
}

impl sion_agent::mcp_input::Asker for Person {
    fn ask(&self, question: &str) -> Option<String> {
        self.asked.lock().unwrap().push(question.to_string());
        let url = question
            .split_whitespace()
            .find(|w| w.starts_with("http://127.0.0.1:"))?;
        let (port, token) = url
            .trim_start_matches("http://127.0.0.1:")
            .split_once('/')?;
        let port: u16 = port.parse().ok()?;
        let host = format!("127.0.0.1:{port}");
        let mut answers = Vec::new();
        answers.push(over_http(
            port,
            &format!("GET /{token} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        ));
        answers.push(over_http(
            port,
            &format!("GET /{token}/view HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        ));
        for (path, body) in [("click", "150 315"), ("text", "777111")] {
            answers.push(over_http(
                port,
                &format!(
                    "POST /{token}/{path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                ),
            ));
        }
        *self.answers.lock().unwrap() = answers;
        Some("done, the code is in".into())
    }
}

#[test]
fn a_signed_in_profile_is_kept_and_a_page_is_pointed_at_and_handed_to_a_person() {
    let (Some(program), Some(sandbox)) = (browser::find_program(), bubblewrap()) else {
        eprintln!("skipped: no Chromium, or bubblewrap does not work here");
        return;
    };
    let port = site(vec![
        (
            "/sign-in",
            "<html><head><title>Sign in</title></head><body><h1>Signed in</h1>\
             <script>document.cookie = 'session=ana; max-age=3600; path=/'</script></body></html>"
                .to_string(),
        ),
        (
            "/account",
            "<html><head><title>Account</title></head><body><h1 id=\"who\">nobody</h1>\
             <script>document.getElementById('who').textContent = 'cookie: ' + (document.cookie || 'none')</script>\
             </body></html>"
                .to_string(),
        ),
        ("/pointer", POINTER.to_string()),
        (
            "/check",
            "<html><head><title>Check</title></head><body><h1>Are you a person?</h1>\
             <iframe src=\"https://www.google.com/recaptcha/api2/anchor?k=x&size=normal\" width=\"304\" height=\"78\"></iframe>\
             </body></html>"
                .to_string(),
        ),
        (
            "/quiet-check",
            "<html><head><title>Quiet</title></head><body><h1>Shop</h1>\
             <iframe src=\"https://www.google.com/recaptcha/api2/anchor?k=x&size=invisible\" width=\"256\" height=\"60\"></iframe>\
             <div style=\"visibility:hidden\"><iframe src=\"https://www.google.com/recaptcha/api2/bframe?k=x\" width=\"400\" height=\"580\"></iframe></div>\
             </body></html>"
                .to_string(),
        ),
    ]);
    let origin = format!("http://127.0.0.1:{port}");
    let profile = std::env::temp_dir().join(format!("sion-browser-kept-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
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
        workspace: None,
        confidential: Vec::new(),
        profile: Some(profile.clone()),
    };

    // No profile until a person signs in to one; then one browser at a time
    // uses it, and it stays when the browser closes.
    let missing = browser::Browser::start(&config).err().unwrap();
    assert!(missing.contains("sign in to one first"), "{missing}");
    std::fs::create_dir_all(&profile).unwrap();
    {
        let mut first = browser::Browser::start(&config).unwrap();
        assert!(browser::in_use(&profile));
        let second = browser::Browser::start(&config).err().unwrap();
        assert!(second.contains("in use by another session"), "{second}");
        first.open(&format!("{origin}/sign-in")).unwrap();
        assert!(first.snapshot().unwrap().contains("Signed in"));
    }
    assert!(!browser::in_use(&profile), "let go");
    assert!(profile.is_dir(), "kept");

    // The next session's browser is signed in: the cookie stayed.
    let person = Arc::new(Person::default());
    let asker: Arc<dyn sion_agent::mcp_input::Asker> = person.clone();
    let shared = Shared::new(config.clone(), None).asking(Some(asker));
    let tools = shared.tools();
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/account")}),
    );
    assert!(
        page.content.contains("cookie: session=ana"),
        "{}",
        page.content
    );

    // What has no reference is clicked where a screenshot shows it, and
    // typed into once it has the focus.
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/pointer")}),
    );
    assert!(!page.is_error, "{}", page.content);
    // A page drawn on a canvas: said once that its tree says little.
    let hint = "The tree says little of this page";
    assert!(page.content.contains(hint), "{}", page.content);
    let again = call(&tools, "browser_snapshot", json!({}));
    assert!(!again.content.contains(hint), "{}", again.content);
    let shot = call(&tools, "browser_screenshot", json!({"marks": true}));
    assert!(!shot.is_error, "{}", shot.content);
    assert!(
        shot.content.contains("marked on it") && !shot.content.contains(" 0 elements"),
        "{}",
        shot.content
    );
    assert!(shot.pictures[0].base64.starts_with("iVBORw0KGgo"), "a PNG");
    let page = call(&tools, "browser_click_at", json!({"x": 150, "y": 120}));
    assert!(
        page.content.contains("clicked the board at 150,120"),
        "{}",
        page.content
    );
    call(&tools, "browser_click_at", json!({"x": 150, "y": 315}));
    let page = call(&tools, "browser_type_focused", json!({"text": "4242"}));
    assert!(page.content.contains("typed 4242"), "{}", page.content);
    let off = call(&tools, "browser_click_at", json!({"x": 5000, "y": 10}));
    assert!(
        off.is_error && off.content.contains("is not on the page"),
        "{}",
        off.content
    );

    // A page that shows a captcha says so, with the advice to hand it over;
    // an invisible one, and a challenge out of sight, are not counted.
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/check")}),
    );
    assert!(
        page.content.contains(
            "The page shows a captcha (reCAPTCHA): hand it to the person (browser_ask_person)."
        ),
        "{}",
        page.content
    );
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/quiet-check")}),
    );
    assert!(!page.content.contains("captcha"), "{}", page.content);
    let page = call(
        &tools,
        "browser_open",
        json!({"url": format!("{origin}/pointer")}),
    );
    // Back on the board, its field empty again.
    assert!(page.content.contains("typed nothing"), "{}", page.content);

    // Handed to the person, who sees the page, clicks into the field and
    // types the code sent to them: their reply, and the page as it is then.
    let page = call(
        &tools,
        "browser_ask_person",
        json!({"why": "the site sent a code to your phone"}),
    );
    assert!(!page.is_error, "{}", page.content);
    assert!(
        page.content
            .contains("the person replied: done, the code is in"),
        "{}",
        page.content
    );
    assert!(page.content.contains("typed 777111"), "{}", page.content);
    let asked = person.asked.lock().unwrap().clone();
    assert!(
        asked[0].contains("the site sent a code to your phone"),
        "{asked:?}"
    );
    let answers = person.answers.lock().unwrap().clone();
    assert!(answers[0].starts_with("HTTP/1.1 200"), "{}", answers[0]);
    assert!(
        answers[1].starts_with("HTTP/1.1 200") && answers[1].contains("image/jpeg"),
        "{}",
        answers[1]
    );
    assert!(
        answers[2..].iter().all(|a| a.starts_with("HTTP/1.1 204")),
        "{answers:?}"
    );
    // Without a person there, the page is handed to no one.
    assert!(!Shared::new(config.clone(), None)
        .tools()
        .iter()
        .any(|t| t.spec().name == "browser_ask_person"));
    drop(tools);
    drop(shared);
    assert!(!browser::in_use(&profile), "let go");
    let _ = std::fs::remove_dir_all(&profile);
}
