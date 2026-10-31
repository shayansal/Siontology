//! Files on the messengers that take them, against stand-ins for their
//! APIs: what a person sends (a picture, a document) is found in what each
//! messenger gives and fetched as it is, and a picture and a document the
//! agent shares are sent as each messenger takes them. The bridge's part,
//! the agent shown the picture and told where each file was put, is in
//! `bridging.rs`; email's and iMessage's are their modules' own tests.

use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};
use sion_cli::files::File;
use sion_cli::messengers::{Incoming, Note, Options};

mod common;
use common::{form, http_stand_in, part, sent_to};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nchart";
const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0photo";
const PDF: &[u8] = b"%PDF-1.7 report";

fn picture() -> File {
    File {
        media_type: "image/png".into(),
        name: "chart.png".into(),
        bytes: PNG.to_vec(),
    }
}

fn document() -> File {
    File {
        media_type: "application/pdf".into(),
        name: "report.pdf".into(),
        bytes: PDF.to_vec(),
    }
}

fn options(chat: &str) -> Options {
    Options {
        allow: vec![chat.into()],
        once: true,
        ..Default::default()
    }
}

fn note(id: &str, media_type: &str, name: Option<&str>) -> Note {
    Note {
        id: id.into(),
        media_type: Some(media_type.into()),
        name: name.map(str::to_string),
    }
}

#[test]
fn files_go_both_ways_on_every_messenger_that_takes_them() {
    // Telegram: a photo, its largest size, and a document; a picture sent
    // as a photo, anything else as a document.
    let (telegram, telegram_seen) = http_stand_in(|sent| {
        let method = sent.url.rsplit('/').next().unwrap_or_default();
        let result = match (method, sent.url.as_str()) {
            (_, "/file/bottg-token/photos/p.jpg") => return (200, JPEG.to_vec()),
            (_, "/file/bottg-token/documents/d.pdf") => return (200, PDF.to_vec()),
            ("getUpdates", _) => json!([
                {"update_id": 1, "message": {"message_id": 10, "chat": {"id": 42, "type": "private"},
                    "from": {"id": 5}, "caption": "What is this?", "photo": [
                        {"file_id": "small", "width": 90, "height": 90},
                        {"file_id": "large", "width": 1280, "height": 960}]}},
                {"update_id": 2, "message": {"message_id": 11, "chat": {"id": 42, "type": "private"},
                    "from": {"id": 5}, "document": {"file_id": "doc", "file_name": "report.pdf",
                        "mime_type": "application/pdf"}}}
            ]),
            ("getFile", _) => match sent.json()["file_id"].as_str() {
                Some("large") => json!({"file_path": "photos/p.jpg"}),
                Some("doc") => json!({"file_path": "documents/d.pdf"}),
                _ => {
                    return (
                        400,
                        br#"{"ok": false, "description": "no such file"}"#.to_vec(),
                    )
                }
            },
            _ => json!({"message_id": 12}),
        };
        (
            200,
            json!({"ok": true, "result": result})
                .to_string()
                .into_bytes(),
        )
    });
    std::env::set_var("TELEGRAM_BOT_TOKEN", "tg-token");
    std::env::set_var("TELEGRAM_API_BASE", &telegram);
    let bot = sion_cli::messengers::for_bridge("telegram", &options("42")).unwrap();
    let (got, _) = bot.receive("", 0).unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].text, "What is this?");
    assert_eq!(got[0].files, [note("large", "image/jpeg", None)]);
    assert_eq!(
        got[1].files,
        [note("doc", "application/pdf", Some("report.pdf"))]
    );
    let photo = bot.fetch_file(&got[0].files[0]).unwrap();
    assert_eq!(photo.bytes, JPEG);
    assert!(photo.is_picture());
    assert_eq!(photo.name, "large.jpg");
    assert_eq!(bot.fetch_file(&got[1].files[0]).unwrap(), document());
    assert!(bot.send_file(&got[0], &picture(), "").unwrap());
    assert!(bot.send_file(&got[0], &document(), "Your report.").unwrap());
    let photo = sent_to(&telegram_seen, "POST", "/bottg-token/sendPhoto").remove(0);
    assert_eq!(part(&photo.body, "chat_id"), Some(&b"42"[..]));
    assert_eq!(part(&photo.body, "photo"), Some(PNG));
    assert_eq!(part(&photo.body, "caption"), None);
    let sent = sent_to(&telegram_seen, "POST", "/bottg-token/sendDocument").remove(0);
    assert_eq!(part(&sent.body, "document"), Some(PDF));
    assert_eq!(part(&sent.body, "caption"), Some(&b"Your report."[..]));
    let disposition = String::from_utf8_lossy(&sent.body).to_string();
    assert!(
        disposition.contains("filename=\"report.pdf\""),
        "{disposition}"
    );

    // Slack, polled: the files a message shares, its audio apart; a file
    // shared back in the message's thread.
    let base_at = Arc::new(OnceLock::<String>::new());
    let slack_base = base_at.clone();
    let (slack, slack_seen) = http_stand_in(move |sent| {
        let base = slack_base.get().unwrap();
        let answer = match sent.url.as_str() {
            "/conversations.history" if sent.json()["oldest"].is_null() => {
                json!({"ok": true, "messages": [{"ts": "100.0", "user": "U9", "text": "earlier"}]})
            }
            "/conversations.history" => json!({"ok": true, "messages": [{"ts": "101.0",
                "user": "U9", "subtype": "file_share", "text": "Here", "thread_ts": "90.0",
                "files": [
                    {"id": "F1", "name": "receipt.png", "mimetype": "image/png",
                        "url_private_download": format!("{base}/files/receipt.png")},
                    {"id": "F2", "subtype": "slack_audio", "mimetype": "audio/webm",
                        "url_private_download": format!("{base}/files/clip.webm")}]}]}),
            "/files/receipt.png" if sent.header("authorization") == Some("Bearer xoxb-files") => {
                return (200, PNG.to_vec())
            }
            "/files.getUploadURLExternal" => json!({"ok": true, "file_id": "F9",
                "upload_url": format!("{base}/upload/F9")}),
            "/upload/F9" => return (200, b"OK".to_vec()),
            "/files.completeUploadExternal" => json!({"ok": true}),
            _ => return (404, b"{}".to_vec()),
        };
        (200, answer.to_string().into_bytes())
    });
    base_at.set(slack.clone()).unwrap();
    std::env::remove_var("SLACK_APP_TOKEN");
    std::env::set_var("SLACK_BOT_TOKEN", "xoxb-files");
    std::env::set_var("SLACK_API_BASE", &slack);
    let channel = sion_cli::messengers::for_bridge("slack", &options("C3")).unwrap();
    let (_, cursor) = channel.receive("", 0).unwrap();
    let (got, _) = channel.receive(&cursor, 0).unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        got[0].files,
        [note(
            &format!("{slack}/files/receipt.png"),
            "image/png",
            Some("receipt.png")
        )]
    );
    assert!(got[0].voice.is_some(), "the audio is a voice note");
    assert_eq!(got[0].thread, "90.0");
    let shown = channel.fetch_file(&got[0].files[0]).unwrap();
    assert_eq!(
        (shown.name.as_str(), shown.bytes.as_slice()),
        ("receipt.png", PNG)
    );
    assert!(channel.send_file(&got[0], &document(), "").unwrap());
    let asked = sent_to(&slack_seen, "POST", "/files.getUploadURLExternal").remove(0);
    assert_eq!(
        form(&asked.body),
        [
            ("filename".to_string(), "report.pdf".to_string()),
            ("length".to_string(), PDF.len().to_string())
        ]
    );
    assert_eq!(
        sent_to(&slack_seen, "POST", "/upload/F9").remove(0).body,
        PDF
    );
    let shared = form(
        &sent_to(&slack_seen, "POST", "/files.completeUploadExternal")
            .remove(0)
            .body,
    );
    assert!(shared.contains(&("channel_id".to_string(), "C3".to_string())));
    assert!(
        shared.contains(&("thread_ts".to_string(), "90.0".to_string())),
        "{shared:?}"
    );
    assert!(
        !shared.iter().any(|(name, _)| name == "initial_comment"),
        "a file alone has no text: {shared:?}"
    );

    // Matrix: an m.image with a caption, an m.file without one; a picture
    // sent back as an m.image.
    let (matrix, matrix_seen) = http_stand_in(|sent| {
        let answer = match (sent.method.as_str(), sent.url.as_str()) {
            ("GET", "/_matrix/client/v3/account/whoami") => json!({"user_id": "@lead:example.org"}),
            ("GET", "/_matrix/client/v3/sync?timeout=0") => json!({"next_batch": "m1"}),
            ("GET", "/_matrix/client/v3/sync?timeout=0&since=m1") => json!({"next_batch": "m2",
                "rooms": {"join": {"!ops:example.org": {"summary": {"m.joined_member_count": 3},
                    "timeline": {"events": [
                        {"type": "m.room.message", "event_id": "$p", "sender": "@ana:example.org",
                            "content": {"msgtype": "m.image", "body": "What is this?",
                                "filename": "receipt.png", "url": "mxc://example.org/pic1",
                                "info": {"mimetype": "image/png"}}},
                        {"type": "m.room.message", "event_id": "$d", "sender": "@ana:example.org",
                            "content": {"msgtype": "m.file", "body": "report.pdf",
                                "url": "mxc://example.org/doc1",
                                "info": {"mimetype": "application/pdf"}}}]}}}}}),
            ("GET", "/_matrix/client/v1/media/download/example.org/pic1") => {
                return (200, PNG.to_vec())
            }
            ("GET", "/_matrix/client/v1/media/download/example.org/doc1") => {
                return (200, PDF.to_vec())
            }
            ("POST", u) if u.starts_with("/_matrix/media/v3/upload?filename=chart.png") => {
                json!({"content_uri": "mxc://example.org/up1"})
            }
            ("PUT", u) if u.starts_with("/_matrix/client/v3/rooms/%21ops%3Aexample.org/send/") => {
                json!({"event_id": "$1"})
            }
            _ => return (404, b"{}".to_vec()),
        };
        (200, answer.to_string().into_bytes())
    });
    std::env::set_var("MATRIX_HOMESERVER", &matrix);
    std::env::set_var("MATRIX_ACCESS_TOKEN", "matrix-files");
    let room = sion_cli::messengers::for_bridge("matrix", &options("!ops:example.org")).unwrap();
    let (_, cursor) = room.receive("", 0).unwrap();
    let (got, _) = room.receive(&cursor, 0).unwrap();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].text, "What is this?");
    assert_eq!(
        got[0].files,
        [note(
            "mxc://example.org/pic1",
            "image/png",
            Some("receipt.png")
        )]
    );
    assert_eq!(got[1].text, "", "a file's name is no caption");
    assert_eq!(
        got[1].files,
        [note(
            "mxc://example.org/doc1",
            "application/pdf",
            Some("report.pdf")
        )]
    );
    assert_eq!(room.fetch_file(&got[1].files[0]).unwrap(), document());
    assert!(room.send_file(&got[0], &picture(), "").unwrap());
    let upload = sent_to(&matrix_seen, "POST", "/_matrix/media/v3/upload").remove(0);
    assert_eq!(upload.body, PNG);
    assert_eq!(upload.header("content-type"), Some("image/png"));
    let message = sent_to(
        &matrix_seen,
        "PUT",
        "/_matrix/client/v3/rooms/%21ops%3Aexample.org/send/",
    )
    .remove(0)
    .json();
    assert_eq!(
        message,
        json!({"msgtype": "m.image", "body": "chart.png", "filename": "chart.png",
            "url": "mxc://example.org/up1", "info": {"mimetype": "image/png", "size": PNG.len()}})
    );

    // Signal: an attachment that is no voice note; a file sent attached.
    let (signal, signal_seen) =
        http_stand_in(|sent| match (sent.method.as_str(), sent.url.as_str()) {
            ("GET", "/v1/receive/%2B15559990000") => (
                200,
                json!([{"envelope": {"sourceNumber": "+15550001111", "timestamp": 7,
                    "dataMessage": {"timestamp": 7, "message": "What is this?", "attachments": [
                        {"id": "pic1.png", "contentType": "image/png", "filename": "receipt.png",
                            "size": PNG.len()}]}}}])
                .to_string()
                .into_bytes(),
            ),
            ("GET", "/v1/attachments/pic1.png") => (200, PNG.to_vec()),
            ("POST", "/v2/send") => (201, br#"{"timestamp": "1"}"#.to_vec()),
            _ => (404, b"{}".to_vec()),
        });
    std::env::set_var("SIGNAL_API_BASE", &signal);
    std::env::set_var("SIGNAL_NUMBER", "+15559990000");
    let number = sion_cli::messengers::for_bridge("signal", &options("+15550001111")).unwrap();
    let (got, _) = number.receive("", 0).unwrap();
    assert_eq!(
        got[0].files,
        [note("pic1.png", "image/png", Some("receipt.png"))]
    );
    assert!(got[0].voice.is_none());
    assert_eq!(number.fetch_file(&got[0].files[0]).unwrap().bytes, PNG);
    assert!(number.send_file(&got[0], &document(), "").unwrap());
    let sent = sent_to(&signal_seen, "POST", "/v2/send").remove(0).json();
    assert_eq!(sent["recipients"], json!(["+15550001111"]));
    assert_eq!(sent["message"], "", "a file alone has no text");
    assert_eq!(
        sent["base64_attachments"],
        json!([format!(
            "data:application/pdf;filename=report.pdf;base64,{}",
            sion_http::base64(PDF)
        )])
    );

    // Discord: an attachment of a message from the gateway; a file posted
    // attached.
    let (discord, discord_seen) =
        http_stand_in(|sent| match (sent.method.as_str(), sent.url.as_str()) {
            ("GET", "/attachments/receipt.png") => (200, PNG.to_vec()),
            ("POST", "/channels/900/messages") => (200, br#"{"id": "1"}"#.to_vec()),
            _ => (404, b"{}".to_vec()),
        });
    let created = sion_cli::live::discord_message(
        &json!({"id": "m1", "channel_id": "900", "guild_id": "1", "author": {"id": "A1"},
            "content": "What is this?", "attachments": [
                {"url": format!("{discord}/attachments/receipt.png"), "content_type": "image/png",
                    "filename": "receipt.png"},
                {"url": format!("{discord}/attachments/note.ogg"), "content_type": "audio/ogg"}]}),
        Some("B7"),
    )
    .unwrap();
    assert_eq!(
        created.files,
        [note(
            &format!("{discord}/attachments/receipt.png"),
            "image/png",
            Some("receipt.png")
        )]
    );
    assert!(created.voice.is_some());
    std::env::set_var("DISCORD_BOT_TOKEN", "discord-files");
    std::env::set_var("DISCORD_API_BASE", &discord);
    let server = sion_cli::messengers::for_bridge("discord", &options("900")).unwrap();
    assert_eq!(server.fetch_file(&created.files[0]).unwrap().bytes, PNG);
    assert!(server.send_file(&created, &picture(), "").unwrap());
    let posted = sent_to(&discord_seen, "POST", "/channels/900/messages").remove(0);
    assert_eq!(part(&posted.body, "files[0]"), Some(PNG));
    let payload: Value =
        serde_json::from_slice(part(&posted.body, "payload_json").unwrap()).unwrap();
    assert_eq!(payload, json!({}), "a file alone has no text");

    // Mattermost: a post with files; a file posted in the thread.
    let (mattermost, mattermost_seen) =
        http_stand_in(|sent| match (sent.method.as_str(), sent.url.as_str()) {
            ("GET", "/api/v4/files/f1") => (200, PDF.to_vec()),
            ("POST", "/api/v4/files") => (
                201,
                json!({"file_infos": [{"id": "up1"}]})
                    .to_string()
                    .into_bytes(),
            ),
            ("POST", "/api/v4/posts") => (201, br#"{"id": "p9"}"#.to_vec()),
            _ => (404, b"{}".to_vec()),
        });
    std::env::set_var("MATTERMOST_URL", &mattermost);
    std::env::set_var("MATTERMOST_TOKEN", "mm-files");
    let post = json!({"id": "p1", "user_id": "u9", "channel_id": "c1", "root_id": "r1",
        "message": "", "type": "", "file_ids": ["f1"]});
    let message = sion_cli::mattermost::mattermost_message(
        &json!({"post": post.to_string(), "channel_type": "O"}),
        Some("b7"),
        None,
        vec![note("f1", "application/pdf", Some("report.pdf"))],
    )
    .expect("a post of files alone is a message");
    let team = sion_cli::messengers::for_bridge("mattermost", &options("c1")).unwrap();
    assert_eq!(team.fetch_file(&message.files[0]).unwrap(), document());
    assert!(team.send_file(&message, &document(), "").unwrap());
    let uploaded = sent_to(&mattermost_seen, "POST", "/api/v4/files").remove(0);
    assert_eq!(part(&uploaded.body, "channel_id"), Some(&b"c1"[..]));
    assert_eq!(part(&uploaded.body, "files"), Some(PDF));
    assert_eq!(
        sent_to(&mattermost_seen, "POST", "/api/v4/posts")
            .remove(0)
            .json(),
        json!({"channel_id": "c1", "message": "", "file_ids": ["up1"], "root_id": "r1"})
    );

    // WhatsApp: a picture with its caption and a document, through a signed
    // webhook; a picture sent as one, a document as one.
    let meta_at = Arc::new(OnceLock::<String>::new());
    let meta_base = meta_at.clone();
    let (meta, meta_seen) = http_stand_in(move |sent| {
        let base = meta_base.get().unwrap();
        let answer = match (sent.method.as_str(), sent.url.as_str()) {
            ("GET", "/media-1") => {
                json!({"url": format!("{base}/blob/1"), "mime_type": "image/jpeg"})
            }
            ("GET", "/blob/1") => return (200, JPEG.to_vec()),
            ("POST", "/106/media") => json!({"id": "up-1"}),
            _ => json!({"messages": [{"id": "wamid.out"}]}),
        };
        (200, answer.to_string().into_bytes())
    });
    meta_at.set(meta.clone()).unwrap();
    std::env::set_var("WHATSAPP_TOKEN", "wa-token");
    std::env::set_var("WHATSAPP_PHONE_NUMBER_ID", "106");
    std::env::set_var("WHATSAPP_APP_SECRET", "wa-secret");
    std::env::set_var("WHATSAPP_VERIFY_TOKEN", "wa-verify");
    std::env::set_var("WHATSAPP_API_BASE", &meta);
    let listen = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("127.0.0.1:{}", probe.local_addr().unwrap().port())
    };
    let whatsapp = sion_cli::messengers::for_bridge(
        "whatsapp",
        &Options {
            listen: listen.clone(),
            ..options("15550001111")
        },
    )
    .unwrap();
    let body = json!({"entry": [{"changes": [{"value": {"metadata": {"phone_number_id": "106"},
        "messages": [
            {"id": "wamid.p", "from": "15550001111", "type": "image",
                "image": {"id": "media-1", "mime_type": "image/jpeg", "caption": "What is this?"}},
            {"id": "wamid.d", "from": "15550001111", "type": "document",
                "document": {"id": "media-2", "mime_type": "application/pdf",
                    "filename": "report.pdf"}}]}}]}]})
    .to_string();
    let signature = format!(
        "sha256={}",
        hex::encode(sion_http::hmac_sha256(b"wa-secret", body.as_bytes()))
    );
    let poster = sion_http::Client::new(sion_http::Options {
        allow_private: true,
        proxy_from_env: false,
        ..sion_http::Options::default()
    })
    .unwrap();
    let posted = poster
        .send(
            "POST",
            &format!("http://{listen}/"),
            &[("X-Hub-Signature-256", &signature)],
            Some(body.as_bytes()),
        )
        .unwrap();
    assert_eq!(posted.status, 200);
    let mut got: Vec<Incoming> = Vec::new();
    for _ in 0..20 {
        got.extend(whatsapp.receive("", 1).unwrap().0);
        if got.len() >= 2 {
            break;
        }
    }
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].text, "What is this?");
    assert_eq!(got[0].files, [note("media-1", "image/jpeg", None)]);
    assert_eq!(
        got[1].files,
        [note("media-2", "application/pdf", Some("report.pdf"))]
    );
    let photo = whatsapp.fetch_file(&got[0].files[0]).unwrap();
    assert_eq!(
        (photo.media_type.as_str(), photo.bytes.as_slice()),
        ("image/jpeg", JPEG)
    );
    assert!(whatsapp.send_file(&got[0], &picture(), "").unwrap());
    assert!(whatsapp.send_file(&got[0], &document(), "").unwrap());
    let uploads = sent_to(&meta_seen, "POST", "/106/media");
    assert_eq!(part(&uploads[0].body, "file"), Some(PNG));
    assert_eq!(
        part(&uploads[1].body, "type"),
        Some(&b"application/pdf"[..])
    );
    let sent: Vec<Value> = sent_to(&meta_seen, "POST", "/106/messages")
        .iter()
        .map(common::Sent::json)
        .collect();
    assert_eq!(
        sent,
        [
            json!({"messaging_product": "whatsapp", "to": "15550001111", "type": "image",
                "image": {"id": "up-1"}}),
            json!({"messaging_product": "whatsapp", "to": "15550001111", "type": "document",
                "document": {"id": "up-1", "filename": "report.pdf"}}),
        ]
    );
}
