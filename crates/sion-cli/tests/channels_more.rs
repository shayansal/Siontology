//! The messengers added last, against stand-ins for their services: Zalo
//! (polled, Telegram's shape), ntfy (topics polled and published to),
//! WeCom (encrypted, signed callbacks, answered through the app's API),
//! QQ (webhooks signed with Ed25519, answered as replies), WeChat (signed
//! in with a code, polled, each reply in the context of what it answers),
//! phone calls through Twilio Voice (each step of a call a signed
//! webhook), X's direct messages (polled, the token renewed), Synology
//! Chat (a chatbot's webhooks, answered through its incoming URL) and
//! SimpleX Chat (its client's WebSocket API, replies as live messages)
//! and Nostr (a relay's subscription, private and older direct messages,
//! each answered the way it came). The bridge itself is tested with
//! every messenger elsewhere; here each one's wire.

use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};
use sion_cli::messengers::{Incoming, Options};

mod common;
use common::{http_stand_in, sent_to};

fn options(chat: &str) -> Options {
    Options {
        allow: vec![chat.into()],
        once: true,
        ..Default::default()
    }
}

fn free() -> String {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("127.0.0.1:{}", probe.local_addr().unwrap().port())
}

fn poster() -> sion_http::Client {
    sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        ..sion_http::Options::default()
    })
    .unwrap()
}

/// What a webhook-fed messenger passes on, waiting for `n` messages.
fn received(messenger: &dyn sion_cli::messengers::Messenger, n: usize) -> Vec<Incoming> {
    let mut got = Vec::new();
    for _ in 0..20 {
        got.extend(messenger.receive("", 1).unwrap().0);
        if got.len() >= n {
            break;
        }
    }
    got
}

#[test]
fn the_last_messengers_answer_on_their_own_wires() {
    // The only test in this binary: the environment is its own.
    let home = std::env::temp_dir().join(format!("sion-channels-more-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("SION_HOME", &home);
    // Zalo: one update a poll, the picture an address; typing, and the
    // reply sent.
    let base_at = Arc::new(OnceLock::<String>::new());
    let zalo_base = base_at.clone();
    let (zalo, zalo_seen) = http_stand_in(move |sent| {
        let base = zalo_base.get().unwrap();
        let result = match sent.url.rsplit('/').next().unwrap_or_default() {
            "getUpdates" => json!({"event_name": "message.image.received", "message": {
                "message_id": "m1", "date": 1, "caption": "What is this?",
                "photo": format!("{base}/media/p.jpg"),
                "from": {"id": "u1", "display_name": "Ann", "is_bot": false},
                "chat": {"id": "u1", "chat_type": "PRIVATE"}}}),
            "p.jpg" => return (200, b"\xFF\xD8\xFF\xE0zalo".to_vec()),
            _ => json!({"message_id": "m2", "date": 2}),
        };
        (
            200,
            json!({"ok": true, "result": result})
                .to_string()
                .into_bytes(),
        )
    });
    base_at.set(zalo.clone()).unwrap();
    std::env::set_var("ZALO_BOT_TOKEN", "123:zalo-secret");
    std::env::set_var("ZALO_API_BASE", &zalo);
    let bot = sion_cli::messengers::for_bridge("zalo", &options("u1")).unwrap();
    let (got, _) = bot.receive("", 0).unwrap();
    assert_eq!(got[0].text, "What is this?");
    assert_eq!(
        bot.fetch_file(&got[0].files[0]).unwrap().bytes,
        b"\xFF\xD8\xFF\xE0zalo"
    );
    let mut typing = bot.typist(&got[0], false).unwrap();
    typing.show().unwrap();
    bot.reply(&got[0], "A receipt.").unwrap();
    let typed = sent_to(&zalo_seen, "POST", "/bot123:zalo-secret/sendChatAction").remove(0);
    assert_eq!(typed.json(), json!({"chat_id": "u1", "action": "typing"}));
    let sent = sent_to(&zalo_seen, "POST", "/bot123:zalo-secret/sendMessage").remove(0);
    assert_eq!(sent.json(), json!({"chat_id": "u1", "text": "A receipt."}));

    // ntfy: the first poll takes the bridge's place; what others publish
    // after it is taken, the bridge's own answers left out; the reply
    // published to the topic, tagged.
    let (ntfy, ntfy_seen) = http_stand_in(|sent| {
        if sent.method == "GET" {
            let lines = [
                json!({"id": "n1", "time": 5, "event": "message", "topic": "ops", "message": "How much is left?"}),
                json!({"id": "n2", "time": 6, "event": "message", "topic": "ops", "message": "85 USD.", "tags": ["robot", "sion"]}),
            ];
            return (
                200,
                lines
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into_bytes(),
            );
        }
        (200, br#"{"id": "n3", "event": "message"}"#.to_vec())
    });
    std::env::set_var("NTFY_URL", &ntfy);
    std::env::set_var("NTFY_TOKEN", "tk_ntfy");
    let topic = sion_cli::messengers::for_bridge("ntfy", &options("ops")).unwrap();
    let (first, cursor) = topic.receive("", 0).unwrap();
    assert!(
        first.is_empty(),
        "the first poll only takes the bridge's place"
    );
    let (got, cursor) = topic.receive(&cursor, 0).unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].text, "How much is left?");
    assert!(cursor.contains("n2"), "{cursor}");
    topic.reply(&got[0], "85 USD left.").unwrap();
    let polled = sent_to(&ntfy_seen, "GET", "/ops/json?poll=1&since=").remove(0);
    assert_eq!(polled.header("authorization"), Some("Bearer tk_ntfy"));
    let published = sent_to(&ntfy_seen, "POST", "/ops").remove(0);
    assert_eq!(published.body, b"85 USD left.");
    assert_eq!(published.header("tags"), Some("robot,sion"));

    // WeCom: a signed, encrypted callback, read; the reply through the
    // app's API, with an access token.
    let (wecom, wecom_seen) = http_stand_in(|sent| {
        let path = sent.url.split('?').next().unwrap_or_default();
        let answer = match path {
            "/cgi-bin/gettoken" => {
                json!({"errcode": 0, "access_token": "wt-1", "expires_in": 7200})
            }
            _ => json!({"errcode": 0, "errmsg": "ok"}),
        };
        (200, answer.to_string().into_bytes())
    });
    let key = "jWmYm7qr5nMoAUwZRjGtBxmz3KA1tkAj3ykkR6q2B2C";
    std::env::set_var("WECOM_CORP_ID", "ww5823bf96d3bd56c7");
    std::env::set_var("WECOM_AGENT_ID", "1000002");
    std::env::set_var("WECOM_SECRET", "wecom-secret");
    std::env::set_var("WECOM_TOKEN", "QDG6eK");
    std::env::set_var("WECOM_AES_KEY", key);
    std::env::set_var("WECOM_API_BASE", &wecom);
    let listen = free();
    let app = sion_cli::messengers::for_bridge(
        "wecom",
        &Options {
            listen: listen.clone(),
            ..options("ann")
        },
    )
    .unwrap();
    let crypt = sion_cli::wecom::MsgCrypt::new("QDG6eK", key, "ww5823bf96d3bd56c7").unwrap();
    let xml = "<xml><ToUserName><![CDATA[ww5823bf96d3bd56c7]]></ToUserName><FromUserName><![CDATA[ann]]></FromUserName><CreateTime>1</CreateTime><MsgType><![CDATA[text]]></MsgType><Content><![CDATA[How much is left?]]></Content><MsgId>77</MsgId><AgentID>1000002</AgentID></xml>";
    let encrypted = crypt.encrypt(xml, &[3; 16]).unwrap();
    let signature = crypt.signature("1", "nonce", &encrypted);
    let posted = poster()
        .send(
            "POST",
            &format!("http://{listen}/?msg_signature={signature}&timestamp=1&nonce=nonce"),
            &[],
            Some(format!("<xml><Encrypt><![CDATA[{encrypted}]]></Encrypt></xml>").as_bytes()),
        )
        .unwrap();
    assert_eq!(posted.status, 200);
    let got = received(app.as_ref(), 1);
    assert_eq!(
        (got[0].chat.as_str(), got[0].text.as_str()),
        ("ann", "How much is left?")
    );
    app.reply(&got[0], "85 USD left.").unwrap();
    let sent = sent_to(
        &wecom_seen,
        "POST",
        "/cgi-bin/message/send?access_token=wt-1",
    )
    .remove(0);
    assert_eq!(
        sent.json(),
        json!({"touser": "ann", "msgtype": "text", "text": {"content": "85 USD left."}, "agentid": 1000002})
    );
    let unsigned = poster()
        .send(
            "POST",
            &format!(
                "http://{listen}/?msg_signature={}&timestamp=1&nonce=nonce",
                "0".repeat(40)
            ),
            &[],
            Some(format!("<xml><Encrypt><![CDATA[{encrypted}]]></Encrypt></xml>").as_bytes()),
        )
        .unwrap();
    assert_eq!(unsigned.status, 401);

    // QQ: an event signed with the bot's key; the reply a passive one, its
    // parts numbered.
    let (qq, qq_seen) = http_stand_in(|sent| {
        let answer = match sent.url.as_str() {
            "/app/getAppAccessToken" => json!({"access_token": "qq-1", "expires_in": "7200"}),
            _ => json!({"id": "out-1"}),
        };
        (200, answer.to_string().into_bytes())
    });
    std::env::set_var("QQ_APP_ID", "102000001");
    std::env::set_var("QQ_APP_SECRET", "naOC0ocQE3shWLAfffVLB1rhYPG7");
    std::env::set_var("QQ_API_BASE", &qq);
    std::env::set_var("QQ_TOKEN_URL", format!("{qq}/app/getAppAccessToken"));
    let listen = free();
    let bot = sion_cli::messengers::for_bridge(
        "qq",
        &Options {
            listen: listen.clone(),
            ..options("user:U1")
        },
    )
    .unwrap();
    let event = r#"{"id":"ev1","op":0,"t":"C2C_MESSAGE_CREATE","d":{"id":"m1","author":{"user_openid":"U1"},"content":"How much is left?"}}"#;
    let seed = sion_cli::qq::qq_seed("naOC0ocQE3shWLAfffVLB1rhYPG7").unwrap();
    let mut signed = b"1725442400".to_vec();
    signed.extend_from_slice(event.as_bytes());
    let signature = hex::encode(sion_http::cipher::ed25519_sign(&seed, &signed).unwrap());
    let posted = poster()
        .send(
            "POST",
            &format!("http://{listen}/"),
            &[
                ("X-Signature-Ed25519", &signature),
                ("X-Signature-Timestamp", "1725442400"),
            ],
            Some(event.as_bytes()),
        )
        .unwrap();
    assert_eq!(posted.status, 200);
    let got = received(bot.as_ref(), 1);
    assert_eq!(
        (got[0].chat.as_str(), got[0].text.as_str()),
        ("user:U1", "How much is left?")
    );
    bot.reply(&got[0], "85 USD").unwrap();
    bot.reply(&got[0], "left.").unwrap();
    let sent: Vec<Value> = sent_to(&qq_seen, "POST", "/v2/users/U1/messages")
        .iter()
        .map(common::Sent::json)
        .collect();
    assert_eq!(
        sent,
        [
            json!({"content": "85 USD", "msg_type": 0, "msg_id": "m1", "msg_seq": 1}),
            json!({"content": "left.", "msg_type": 0, "msg_id": "m1", "msg_seq": 2}),
        ]
    );
    let asked = sent_to(&qq_seen, "POST", "/v2/users/U1/messages").remove(0);
    assert_eq!(asked.header("authorization"), Some("QQBot qq-1"));

    // A phone call: greeted, heard, kept waiting, and the reply said.
    std::env::set_var("TWILIO_AUTH_TOKEN", "tw-voice");
    std::env::set_var("TWILIO_VOICE_URL", "https://bot.example.org/phone");
    let listen = free();
    let phone = sion_cli::messengers::for_bridge(
        "phone",
        &Options {
            listen: listen.clone(),
            ..options("+15550001111")
        },
    )
    .unwrap();
    let step = |path: &str, form: &[(&str, &str)]| {
        let body = form
            .iter()
            .map(|(n, v)| format!("{n}={}", v.replace('+', "%2B").replace(' ', "+")))
            .collect::<Vec<_>>()
            .join("&");
        let signature = sion_cli::webhooks::twilio_signature(
            "tw-voice",
            &format!("https://bot.example.org{path}"),
            &sion_cli::webhooks::decode_form(body.as_bytes()),
        );
        let answer = poster()
            .send(
                "POST",
                &format!("http://{listen}{path}"),
                &[
                    ("X-Twilio-Signature", &signature),
                    ("Content-Type", "application/x-www-form-urlencoded"),
                ],
                Some(body.as_bytes()),
            )
            .unwrap();
        String::from_utf8(answer.read_all(1 << 16).unwrap()).unwrap()
    };
    let caller = [("CallSid", "CA1"), ("From", "+15550001111")];
    assert!(step("/phone", &caller).contains("<Gather input=\"speech\""));
    let heard = step(
        "/phone/heard",
        &[
            ("CallSid", "CA1"),
            ("From", "+15550001111"),
            ("SpeechResult", "How much budget is left?"),
        ],
    );
    assert!(heard.contains("/phone/wait"), "{heard}");
    let got = received(phone.as_ref(), 1);
    assert_eq!(got[0].text, "How much budget is left?");
    phone.reply(&got[0], "85 USD left.").unwrap();
    let said = step("/phone/wait", &caller);
    assert!(said.contains(">85 USD left.</Say>"), "{said}");
    let stranger = step("/phone", &[("CallSid", "CA2"), ("From", "+15559999999")]);
    assert!(stranger.contains("<Hangup/>"), "{stranger}");

    // WeChat: signed in with a code scanned, its sign-in kept sealed; then
    // polled, and the reply carries the context of what it answers.
    let (weixin, weixin_seen) = http_stand_in(|sent| {
        let path = sent.url.split('?').next().unwrap_or_default();
        let answer = match path {
            "/ilink/bot/get_bot_qrcode" => {
                json!({"qrcode": "q1", "qrcode_img_content": "https://weixin.example/q1"})
            }
            "/ilink/bot/get_qrcode_status" => {
                json!({"status": "confirmed", "bot_token": "wx-token",
                "ilink_bot_id": "bot1@im.bot", "ilink_user_id": "wxid_owner"})
            }
            "/ilink/bot/getupdates" => json!({"ret": 0, "get_updates_buf": "c1", "msgs": [
                {"message_id": 1, "from_user_id": "wxid_ann", "message_type": 1,
                 "context_token": "ctx-1",
                 "item_list": [{"type": 1, "text_item": {"text": "How much is left?"}}]}]}),
            _ => json!({"ret": 0}),
        };
        (200, answer.to_string().into_bytes())
    });
    std::env::set_var("WEIXIN_API_BASE", &weixin);
    let mut shown = String::new();
    let said = sion_cli::weixin::login_with(&mut |url: &str| shown = url.to_string()).unwrap();
    assert_eq!(shown, "https://weixin.example/q1");
    assert!(said.contains("bot1@im.bot"), "{said}");
    let wechat = sion_cli::messengers::for_bridge("weixin", &options("wxid_ann")).unwrap();
    let (got, cursor) = wechat.receive("", 0).unwrap();
    assert_eq!(
        (cursor.as_str(), got[0].text.as_str()),
        ("c1", "How much is left?")
    );
    wechat.reply(&got[0], "85 USD left.").unwrap();
    let sent = sent_to(&weixin_seen, "POST", "/ilink/bot/sendmessage").remove(0);
    assert_eq!(sent.header("authorization"), Some("Bearer wx-token"));
    assert_eq!(sent.header("authorizationtype"), Some("ilink_bot_token"));
    let body = sent.json();
    assert_eq!(body["msg"]["context_token"], "ctx-1");
    assert_eq!(body["msg"]["to_user_id"], "wxid_ann");
    assert_eq!(
        body["msg"]["item_list"][0]["text_item"]["text"],
        "85 USD left."
    );
    assert_eq!(body["base_info"]["channel_version"], "1.0.2");
    assert!(sion_cli::weixin::logout().unwrap());

    // X: direct messages polled, the first poll taking the bridge's place;
    // an access token X refuses renewed from the refresh token.
    let (x, x_seen) = http_stand_in(|sent| {
        let path = sent.url.split('?').next().unwrap_or_default();
        let fresh = sent.header("authorization") == Some("Bearer x-2");
        let answer = match path {
            "/2/oauth2/token" => {
                json!({"access_token": "x-2", "refresh_token": "xr-2", "token_type": "bearer"})
            }
            _ if !fresh => return (401, br#"{"title": "Unauthorized"}"#.to_vec()),
            "/2/users/me" => json!({"data": {"id": "7", "username": "lead"}}),
            "/2/dm_events" => json!({"data": [
                {"id": "1001", "event_type": "MessageCreate", "text": "Earlier.", "sender_id": "42"},
                {"id": "1002", "event_type": "MessageCreate", "text": "How much is left?", "sender_id": "42"}]}),
            _ => json!({"data": {"dm_conversation_id": "7-42", "dm_event_id": "1003"}}),
        };
        (200, answer.to_string().into_bytes())
    });
    std::env::set_var("X_API_BASE", &x);
    std::env::set_var("X_ACCESS_TOKEN", "x-1");
    std::env::set_var("X_REFRESH_TOKEN", "xr-1");
    std::env::set_var("X_CLIENT_ID", "x-client");
    std::env::set_var("X_POLL_SECS", "0");
    let account = sion_cli::messengers::for_bridge("x", &options("42")).unwrap();
    let (first, cursor) = account.receive("1001", 0).unwrap();
    assert_eq!(cursor, "1002");
    assert_eq!(first.len(), 1, "only what came after the place: {first:?}");
    account.reply(&first[0], "85 USD left.").unwrap();
    let renewed = common::form(&sent_to(&x_seen, "POST", "/2/oauth2/token").remove(0).body);
    assert!(renewed.contains(&("refresh_token".to_string(), "xr-1".to_string())));
    let sent = sent_to(&x_seen, "POST", "/2/dm_conversations/with/42/messages").remove(0);
    assert_eq!(sent.json(), json!({"text": "85 USD left."}));

    // Synology Chat: a chatbot's request, carrying its token; the reply
    // through the incoming URL, to the user who wrote.
    let (nas, nas_seen) = http_stand_in(|_| (200, br#"{"success": true}"#.to_vec()));
    std::env::set_var(
        "SYNOLOGY_CHAT_INCOMING_URL",
        format!("{nas}/webapi/entry.cgi?api=SYNO.Chat.External&method=chatbot&version=2&token=%22syno-token%22"),
    );
    std::env::set_var("SYNOLOGY_CHAT_TOKEN", "syno-token");
    let listen = free();
    let bot = sion_cli::messengers::for_bridge(
        "synology-chat",
        &Options {
            listen: listen.clone(),
            ..options("5")
        },
    )
    .unwrap();
    let form = "token=syno-token&user_id=5&username=ann&post_id=31&thread_id=0&timestamp=1&text=How+much+is+left%3F";
    let posted = poster()
        .send(
            "POST",
            &format!("http://{listen}/"),
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(form.as_bytes()),
        )
        .unwrap();
    assert_eq!(posted.status, 200);
    let got = received(bot.as_ref(), 1);
    assert_eq!(
        (got[0].chat.as_str(), got[0].text.as_str()),
        ("5", "How much is left?")
    );
    bot.reply(&got[0], "85 USD left.").unwrap();
    let sent = sent_to(
        &nas_seen,
        "POST",
        "/webapi/entry.cgi?api=SYNO.Chat.External&method=chatbot",
    )
    .remove(0);
    let payload = common::form(&sent.body);
    assert_eq!(payload[0].0, "payload");
    assert_eq!(
        serde_json::from_str::<Value>(&payload[0].1).unwrap(),
        json!({"text": "85 USD left.", "user_ids": [5]})
    );
    let forged = poster()
        .send(
            "POST",
            &format!("http://{listen}/"),
            &[],
            Some(form.replace("syno-token", "guess").as_bytes()),
        )
        .unwrap();
    assert_eq!(forged.status, 401);

    // SimpleX Chat: the client's WebSocket API, who the bot is asked
    // first; a contact's message as an event, the reply a command, and a
    // reply shown as it is written sent live and updated.
    let client_api = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std::env::set_var(
        "SIMPLEX_URL",
        format!("ws://{}", client_api.local_addr().unwrap()),
    );
    let commands = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let kept = commands.clone();
    std::thread::spawn(move || {
        let (tcp, _) = client_api.accept().unwrap();
        let (mut socket, _) = sion_http::ws::WebSocket::accept(tcp, 1 << 20).unwrap();
        let mut told = false;
        loop {
            let text = match socket.receive(std::time::Duration::from_millis(100)) {
                Ok(Some(sion_http::ws::Message::Text(text))) => text,
                Ok(_) => continue,
                Err(_) => return,
            };
            let command: Value = serde_json::from_str(&text).unwrap();
            let (corr, cmd) = (
                command["corrId"].clone(),
                command["cmd"].as_str().unwrap().to_string(),
            );
            let resp = if cmd == "/user" {
                json!({"type": "activeUser", "user": {"userId": 1, "localDisplayName": "sion"}})
            } else if cmd.starts_with("/_send") {
                json!({"type": "newChatItems", "chatItems": [{"chatInfo": {"type": "direct", "contact": {"contactId": 3}},
                    "chatItem": {"chatDir": {"type": "directSnd"}, "meta": {"itemId": 20},
                                 "content": {"type": "sndMsgContent", "msgContent": {"type": "text", "text": ""}}}}]})
            } else {
                json!({"type": "chatItemUpdated", "chatItem": {}})
            };
            kept.lock().unwrap().push(cmd);
            socket
                .send_text(&json!({"corrId": corr, "resp": resp}).to_string())
                .unwrap();
            if !told {
                told = true;
                let event = json!({"resp": {"type": "newChatItems", "chatItems": [
                    {"chatInfo": {"type": "direct", "contact": {"contactId": 3, "localDisplayName": "ann"}},
                     "chatItem": {"chatDir": {"type": "directRcv"}, "meta": {"itemId": 19},
                                  "content": {"type": "rcvMsgContent", "msgContent": {"type": "text", "text": "How much is left?"}}}}]}});
                socket.send_text(&event.to_string()).unwrap();
            }
        }
    });
    let simplex = sion_cli::messengers::for_bridge("simplex", &options("@3")).unwrap();
    let got = received(simplex.as_ref(), 1);
    assert_eq!(
        (
            got[0].chat.as_str(),
            got[0].id.as_str(),
            got[0].text.as_str()
        ),
        ("@3", "@3/19", "How much is left?")
    );
    assert_eq!(simplex.me().unwrap(), ["sion"]);
    simplex.reply(&got[0], "85 USD left.").unwrap();
    let mut drafting = simplex.drafting(&got[0]).unwrap();
    let mut draft = (drafting.open)();
    draft.show("85 USD", None).unwrap();
    draft.finish("85 USD left.").unwrap();
    let text = |t: &str| json!({"msgContent": {"type": "text", "text": t}, "mentions": {}});
    assert_eq!(
        *commands.lock().unwrap(),
        [
            "/user".to_string(),
            format!("/_send @3 json {}", json!([text("85 USD left.")])),
            format!("/_send @3 live=on json {}", json!([text("85 USD")])),
            format!("/_update item @3 20 json {}", text("85 USD left.")),
        ]
    );

    // Nostr: a relay that asks who reads (NIP-42); a private message
    // (NIP-17) and an older one (NIP-04), written while the bridge was
    // away, taken from its place and each answered the way it came; the
    // bridge names its relays for private messages, as the account named
    // none.
    use sion_cli::nostr::{
        gift_wrap, nip04_decrypt, nip04_encrypt, npub, open_gift_wrap, Event, Keys,
    };
    let bot = Keys::from_secret(&[7; 32]).unwrap();
    let (ann, bo) = (
        Keys::from_secret(&[8; 32]).unwrap(),
        Keys::from_secret(&[9; 32]).unwrap(),
    );
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let held: Arc<std::sync::Mutex<Vec<Event>>> = Arc::new(std::sync::Mutex::new(vec![
        gift_wrap(&ann, &bot.public, "How much is left?", at).unwrap(),
        bo.sign(
            4,
            vec![vec!["p".into(), bot.public_hex()]],
            nip04_encrypt(&bo, &bot.public, "And in EUR?", &[5; 16]).unwrap(),
            at,
        )
        .unwrap(),
    ]));
    let published: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
    let relay = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let relay_url = format!("ws://{}", relay.local_addr().unwrap());
    let (store, told) = (held.clone(), published.clone());
    std::thread::spawn(move || {
        for tcp in relay.incoming() {
            let (store, told) = (store.clone(), told.clone());
            std::thread::spawn(move || {
                let (mut socket, _) =
                    sion_http::ws::WebSocket::accept(tcp.unwrap(), 1 << 20).unwrap();
                socket
                    .send_text(&json!(["AUTH", "challenge-1"]).to_string())
                    .unwrap();
                loop {
                    let text = match socket.receive(std::time::Duration::from_millis(100)) {
                        Ok(Some(sion_http::ws::Message::Text(text))) => text,
                        Ok(_) => continue,
                        Err(_) => return,
                    };
                    let message: Value = serde_json::from_str(&text).unwrap();
                    match message[0].as_str().unwrap() {
                        "REQ" => {
                            let sub = message[1].clone();
                            let filters = message.as_array().unwrap()[2..].to_vec();
                            for event in store.lock().unwrap().iter() {
                                let matches = filters.iter().any(|f| {
                                    let has = |key: &str, value: Value| {
                                        f[key].as_array().map_or(true, |list| list.contains(&value))
                                    };
                                    has("kinds", json!(event.kind))
                                        && has("authors", json!(event.pubkey))
                                        && f["#p"].as_array().map_or(true, |list| {
                                            event
                                                .tags
                                                .iter()
                                                .any(|t| t[0] == "p" && list.contains(&json!(t[1])))
                                        })
                                        && f["since"]
                                            .as_u64()
                                            .map_or(true, |since| event.created_at >= since)
                                });
                                if matches {
                                    socket
                                        .send_text(&json!(["EVENT", sub, event]).to_string())
                                        .unwrap();
                                }
                            }
                            socket.send_text(&json!(["EOSE", sub]).to_string()).unwrap();
                        }
                        "EVENT" | "AUTH" => {
                            let event = message[1].clone();
                            told.lock().unwrap().push(message.clone());
                            socket
                                .send_text(&json!(["OK", event["id"], true, ""]).to_string())
                                .unwrap();
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    std::env::set_var("NOSTR_PRIVATE_KEY", hex::encode([7u8; 32]));
    std::env::set_var("NOSTR_RELAYS", &relay_url);
    std::env::remove_var("NOSTR_DM");
    let nostr = sion_cli::messengers::for_bridge("nostr", &options(&npub(&ann.public))).unwrap();
    let mut got = Vec::new();
    let mut place = (at - 10).to_string();
    for _ in 0..20 {
        let (messages, next) = nostr.receive(&place, 1).unwrap();
        got.extend(messages);
        place = next;
        if got.len() >= 2 {
            break;
        }
    }
    got.sort_by(|a, b| a.text.cmp(&b.text));
    let said: Vec<(&str, &str)> = got
        .iter()
        .map(|m| (m.chat.as_str(), m.text.as_str()))
        .collect();
    let (ann_npub, bo_npub) = (npub(&ann.public), npub(&bo.public));
    assert_eq!(
        said,
        [
            (bo_npub.as_str(), "And in EUR?"),
            (ann_npub.as_str(), "How much is left?")
        ]
    );
    assert_eq!(
        place,
        at.to_string(),
        "the place is the last message's time"
    );
    for message in &got {
        nostr.reply(message, "85 USD left.").unwrap();
    }
    let mut told = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    for _ in 0..50 {
        told = published.lock().unwrap().clone();
        events = told
            .iter()
            .filter(|m| m[0] == "EVENT")
            .map(|m| serde_json::from_value(m[1].clone()).unwrap())
            .collect();
        if events.iter().any(|e| e.kind == 10050) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let auth: Event =
        serde_json::from_value(told.iter().find(|m| m[0] == "AUTH").unwrap()[1].clone()).unwrap();
    assert_eq!(
        (auth.kind, auth.pubkey.as_str()),
        (22242, bot.public_hex().as_str())
    );
    assert!(auth
        .tags
        .contains(&vec!["challenge".to_string(), "challenge-1".to_string()]));
    assert!(sion_cli::nostr::verify(&auth));
    let listed = events
        .iter()
        .find(|e| e.kind == 10050)
        .expect("the bridge names its relays");
    assert_eq!(listed.tags, [vec!["relay".to_string(), relay_url.clone()]]);
    let wrap = events
        .iter()
        .find(|e| e.kind == 1059)
        .expect("a private reply to Ann");
    let (sender, rumor) = open_gift_wrap(&ann, wrap).unwrap();
    assert_eq!(
        (sender, rumor["content"].as_str()),
        (bot.public, Some("85 USD left."))
    );
    let older = events
        .iter()
        .find(|e| e.kind == 4)
        .expect("an older reply to Bo");
    assert_eq!(
        nip04_decrypt(&bo, &bot.public, &older.content).unwrap(),
        "85 USD left."
    );
    assert_eq!(older.tags, [vec!["p".to_string(), bo.public_hex()]]);
    std::fs::remove_dir_all(&home).unwrap();
}
