//! A single IRC server connection.

use futures::StreamExt;
use irc::client::prelude::{Client, Config};
use irc::proto::Message;
use thiserror::Error;

/// Errors from the IRC connection layer.
#[derive(Debug, Error)]
pub enum ConnectionError {
    /// The underlying irc crate failed.
    #[error("irc: {0}")]
    Irc(#[from] irc::error::Error),
    /// The connection is gone.
    #[error("not connected")]
    NotConnected,
}

/// Parameters for connecting to a server.
#[derive(Debug, Clone)]
pub struct ConnectParams {
    /// Server host name.
    pub server: String,
    /// Server port.
    pub port: u16,
    /// Whether to use TLS.
    pub tls: bool,
    /// Nickname to use.
    pub nick: String,
}

/// A live connection to one IRC server.
pub struct Connection {
    client: Client,
    stream: irc::client::ClientStream,
    params: ConnectParams,
}

impl Connection {
    /// Connect and register with the server.
    pub async fn connect(params: ConnectParams) -> Result<Self, ConnectionError> {
        let config = Config {
            nickname: Some(params.nick.clone()),
            // Survive nick collisions instead of dying during registration.
            alt_nicks: vec![format!("{}_", params.nick), format!("{}__", params.nick)],
            server: Some(params.server.clone()),
            port: Some(params.port),
            use_tls: Some(params.tls),
            ..Config::default()
        };
        let mut client = Client::from_config(config).await?;
        client.identify()?;
        let stream = client.stream()?;
        Ok(Self {
            client,
            stream,
            params,
        })
    }

    /// The server this connection points at.
    pub fn server(&self) -> &str {
        &self.params.server
    }

    /// The nick we registered with (the live nick is tracked by the session).
    pub fn nick(&self) -> &str {
        &self.params.nick
    }

    /// Access the sending half.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Await the next message from the server; `Ok(None)` means the stream ended.
    pub async fn next_message(&mut self) -> Result<Option<Message>, ConnectionError> {
        Ok(self.stream.next().await.transpose()?)
    }
}
