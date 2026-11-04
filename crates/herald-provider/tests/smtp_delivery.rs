//! End-to-end Maildir + SMTP test against a minimal in-process SMTP server.
//!
//! This exercises the real `lettre` transport, the RFC 5322 message builder,
//! the Sent copy, and the verification step, not a recording stub.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::mpsc;

use herald_core::ThreadId;
use herald_provider::CommunicationProvider;
use herald_provider::maildir::{MaildirConfig, MaildirProvider, SmtpConfig, SmtpSecurity};

fn write_maildir(root: &Path) {
    for folder in ["cur", "new", "tmp"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    let message = "From: Sam Lee <sam@example.com>\r\n\
                   To: me@example.com\r\n\
                   Subject: Lunch?\r\n\
                   Date: Tue, 3 Nov 2026 10:00:00 +0000\r\n\
                   Message-ID: <sam-1@example.com>\r\n\
                   \r\n\
                   Free on Thursday?\r\n";
    std::fs::write(root.join("new").join("sam-1"), message).unwrap();
}

/// A one-connection SMTP server that accepts one message and reports its DATA.
fn spawn_smtp() -> (u16, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        writer
            .write_all(b"220 localhost ESMTP herald-test\r\n")
            .ok();
        writer.flush().ok();

        let mut data = Vec::new();
        let mut in_data = false;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }

            if in_data {
                if line == ".\r\n" {
                    writer.write_all(b"250 2.0.0 queued\r\n").ok();
                    writer.flush().ok();
                    let _ = sender.send(std::mem::take(&mut data));
                    return;
                }
                // Undo SMTP dot-stuffing for the assertion.
                let line = line.strip_prefix('.').unwrap_or(&line);
                data.extend_from_slice(line.as_bytes());
                continue;
            }

            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                writer
                    .write_all(b"250-localhost\r\n250-SIZE 10485760\r\n250 8BITMIME\r\n")
                    .ok();
                writer.flush().ok();
            } else if upper.starts_with("DATA") {
                in_data = true;
                writer.write_all(b"354 end with <CRLF>.<CRLF>\r\n").ok();
                writer.flush().ok();
            } else if upper.starts_with("QUIT") {
                writer.write_all(b"221 2.0.0 bye\r\n").ok();
                writer.flush().ok();
                break;
            } else {
                writer.write_all(b"250 2.0.0 ok\r\n").ok();
                writer.flush().ok();
            }
        }

        let _ = sender.send(data);
    });

    (port, receiver)
}

#[tokio::test]
async fn maildir_sends_through_a_real_smtp_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Maildir");
    write_maildir(&root);

    let (port, received) = spawn_smtp();
    let config = MaildirConfig {
        root: root.clone(),
        sent_path: Some(root.join(".Sent")),
        from: Some("Me <me@example.com>".into()),
        smtp: Some(SmtpConfig {
            host: "127.0.0.1".into(),
            port,
            username: None,
            password: None,
            security: SmtpSecurity::None,
        }),
    };
    let provider = MaildirProvider::new(config);
    let thread = ThreadId::new("sam-1@example.com");

    let prepared = provider
        .prepare_reply(&thread, "Thursday works for me.", "action-smtp-1")
        .await
        .unwrap();
    let receipt = provider.send_reply(&prepared).await.unwrap();
    assert!(receipt.verified, "the Sent copy must verify");
    assert_eq!(receipt.body_hash, prepared.body_hash);

    let raw = received
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("SMTP server received DATA");
    let text = String::from_utf8_lossy(&raw);
    assert!(text.contains("From: Me <me@example.com>"));
    assert!(text.contains("To: sam@example.com"));
    assert!(text.contains("Subject: Re: Lunch?"));
    assert!(text.contains("In-Reply-To: <sam-1@example.com>"));
    assert!(text.contains("X-Herald-Token: action-smtp-1"));
    assert!(text.contains("Thursday works for me."));

    // The Sent copy exists and a second send is idempotent.
    let sent = root.join(".Sent");
    assert!(sent.join("cur").is_dir());
    let second = provider.send_reply(&prepared).await.unwrap();
    assert_eq!(second.provider_message_id, receipt.provider_message_id);
}
