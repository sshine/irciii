//! The "first goal" smoke test: connect to irc.libera.chat over TLS, join a
//! channel, say hello, and quit cleanly.
//!
//! Requires networking; run with `just test-net` (`cargo test -- --ignored`).

use std::time::Duration;

use irc::proto::{Command, Response};
use irciii_daemon::connection::{ConnectParams, Connection};
use irciii_daemon::translate::translate;
use irciii_proto::history::HistoryKind;

const CHANNEL: &str = "##irciii-test";
const HELLO: &str = "hello from the irciii integration test";

#[tokio::test]
#[ignore = "network: connects to irc.libera.chat"]
async fn connect_join_hello_quit() {
    let nick = format!("irciii-{:04x}", rand::random::<u16>());
    let params = ConnectParams {
        server: "irc.libera.chat".to_string(),
        port: 6697,
        tls: true,
        nick: nick.clone(),
    };

    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let mut conn = Connection::connect(params).await.expect("connect");

        // Wait for registration (RPL_WELCOME), then join.
        loop {
            let message = conn
                .next_message()
                .await
                .expect("stream error")
                .expect("stream ended before welcome");
            if matches!(message.command, Command::Response(Response::RPL_WELCOME, _)) {
                break;
            }
        }
        conn.client().send_join(CHANNEL).expect("send JOIN");

        // Wait until we see ourselves join, then say hello.
        loop {
            let message = conn
                .next_message()
                .await
                .expect("stream error")
                .expect("stream ended before join");
            let is_us = message.source_nickname() == Some(nick.as_str());
            match translate(&message) {
                Some(HistoryKind::Join { channel, .. }) if is_us && channel == CHANNEL => break,
                _ => {}
            }
        }
        conn.client()
            .send_privmsg(CHANNEL, HELLO)
            .expect("send PRIVMSG");
        conn.client().send_quit("bye").expect("send QUIT");

        // Drain until the server closes the stream; a clean end means the
        // quit was accepted.
        while let Some(_message) = conn.next_message().await.unwrap_or(None) {}
    })
    .await;

    result.expect("test timed out after 60s");
}
