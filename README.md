A minimal irssi-like terminal IRC client.

Running `irciii` starts a daemon (which owns the IRC connections and chat
history) and a client (the TUI) as separate processes. `/detach` leaves the
daemon running and prints a session id; `irciii --attach <session-id>`
reattaches with full scrollback. `/exit` (or `/quit`) terminates both.

```
irciii [<server> <port> [<channel>]] [--no-tls] [--nick <nick>]
```

With a server, the client connects on startup (TLS unless `--no-tls`) and
joins the channel once registration completes; without one it starts
unconnected, awaiting `/connect`.

![irciii-logo.png](./irciii-logo.png)
