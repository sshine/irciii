# irciii — oneshot prompt

In ./irciii build a terminal IRC client in Rust using the 'irc' crate with the feel of
irssi, but with much less functionality. I'll explain the desired initial feature set
briefly, but first some practical matters:

## Practical matters

- Use ./walltime-rs as a template (dendritic nix, cargo workspace, justfile, etc.).
- Use ./promptui-rs as the UI library (by referencing https://github.com/sshine/promptui.git,
  or whatever the correct git crate reference format is, in Cargo.toml).
- Use hk-nix for nix hooks instead of lefthook-nix: https://github.com/nix-tools/hk-nix
  -- there's an example in the readme, but it's not dendritic, so make it that; i.e. put
  something like `nix/hooks.nix`. I'm thinking:
  - **pre-commit**: treefmt-nix (rust, nix, markdown)
  - **pre-push**: deadnix, clippy, cargo-readme (use the git version of cargo-readme:
    https://github.com/webern/cargo-readme.git because it supports Cargo workspaces)
  - **pre-push**: a lock-check. From https://git.shine.town/infra/helm-vendor/src/branch/main/nix/devshell.nix:
    ```nix
    pre-push.commands.lock-check = {
      run = "cargo metadata --locked --format-version 1 > /dev/null";
    };
    ```
    I think this is fixing a problem that sometimes occurs with the lockfile being out of sync.
  - **commit-msg**: a hook that checks that the commit message uses conventional commits;
    it should be configurable to allow one to specify that the scope is mandatory (i.e.
    "feat(foo): ...") and that the set of verbs is restricted to a short list (but defaults
    to the full conventional commits set, which I think is well known, but might be ambiguous,
    not sure).

## Feature list

Commands:

- `/connect` (with tls support)
- `/join`
- `/part`
- `/nick`
- `/msg`
- `/notice`
- `/mode`
- `/query`
- `/names`
- `/quit`
- `/topic`
- `/quote`
- `/disconnect`
- `/detach`

It does not need to support multiple windows: one window for all servers and all channels
is fine.

It should have a status bar that shows an HH:MM:SS timestamp, nick(+modes) and current
channel(+modes).

It should have a client-server model that uses TCP and the postcard crate, and starting the
main CLI should start both client and server in separate processes; `/exit` should terminate
both client and server, and `/detach` should only terminate the client; upon detaching, the
client should print a session ID (uuid) that can be used to `--attach <session-id>` with.

`^X` should cycle between the active window.

Here is a "screenshot" (copy-paste of irssi with my current theme):

```
21:16:28 <zfnmxt> But, yeah, I have dual citizenship.
21:16:36 <zfnmxt> I consider myself more American than German.
21:17:03 <sshine> so did Oppenheimer!
21:17:05 <zfnmxt> I don't think I'm going to use AI generated code.
21:17:08 <sshine> I am not going anywhere with that
21:17:16 <zfnmxt> We'll see how long I can get away with it.
21:17:55 <sshine> you could try... what's it called, Cursor? like basic line-by-line autocomplete. I hear it's annoying.
21:18:34 <sshine> I'd like to know if AI in compiler making is worth it. I think there's a lot of routine work with compilers, like a 
                  lot of big ASTs that need tree-walkers and stuff.
21:18:43 <zfnmxt> Nah. I'm okay with using AI as a sort of rubber ducky and such (maybe), but I think I should do all the edits myself 
                  for now.
21:19:10  * sshine imagines having a conversation about horses and diesel engines
21:19:34 <zfnmxt> Yeah, I've tried being like "fill in this boring tree walk for me and leave the non trivial cases as undefined". It 
                  works for that, but somehow I still find it disagreeable and feel this tacit degradation of familiarity with the 
                  structures/AST I'm transforming.
21:20:10 <sshine> the question is, can you degrade your familiarity without degrading the quality of what you make?
21:20:13 <n_ko> Sometimes AI is also just not that fast. A prompt can take longer than writing out one case and then copy pasting that a 
                few t imes.
21:20:13 <zfnmxt> Haha, also that. Maybe it's a bad strategy. I don't know. I'm not sure the analogy completely holds here.
21:20:26 <n_ko> But I guess you use a dumber model then, for speed
21:20:41 <sshine> I mean, at some point you could be trimming every small branch of your garden with a nail clipper.
21:20:48 <zfnmxt> I don't find it enjoyable to be unfamiliar. It's not just about the quality of the result. It's also about what I 
                  learn and how much I enjoy the process.
21:21:26 <zfnmxt> But that's why I pitched it as an optimization problem. Not sure what's the best long-term strategy.
21:22:25 <sshine> n_ko, that's true, about AI sometimes taking longer. but it's an early adaptation thing. for comparison, when I 
                  learned Rust, I'd spend as much time fighting the borrow checker as I spent fighting the type checker as I spent 
                  coding. gradually, fighting the borrow checker disappeared entirely. like, my first week, borrow checker fights were 
                  30-60 minutes. and suddenly, they don't exist.
21:22:42 <zfnmxt> Anyway I gotta go out my big boy pants on and negotiate my offer. See you guys later 🫡
21:22:53 <n_ko> gl!
21:22:54 <zfnmxt> s/out/put
21:22:58 <concieggs> Rettet: <zfnmxt> Anyway I gotta go put my big boy pants on and negotiate my offer. See you guys later 🫡
21:22:59 <sshine> glhf
21:24:35 <sshine> n_ko, are you also into quantum computing?
21:27:11 <n_ko> Rust was the first language I used with a fancy type system. I remember how the Rust borrow checker made me feel abashed
21:27:26 <n_ko> but I was so impressed by its ability to point out exactly where I was wrong
21:27:41 <n_ko> Don't know anything about quantum, no
10:57:38 sshine(+Ziw) 1:#diku(+n)
```

Note how the timestamp on each line aligns with the timestamp in the menubar.

Besides this, it's totally OK to include more features if they are naturally given by the
irc crate, or are easy to implement given the irc crate.

## Reactive chat history

The chat history should be scrollable with pgup/pgdn, and each item in it should be rendered
dynamically. I.e. if I change the timestamp format (not a feature request yet), each line
should be re-rendered, but only the visible part. So the chat history is not just an already
rendered dump, it's reactive: it holds structured items and renders them on demand. I don't
know if this warrants a reactive terminal framework, I think it's doable without.

## Hook system

I want a hook system, which should only be available from within the source code initially;
every IRC event should be extensible with a hook that catches the event and does something
(either renders the UI, or sends a message back).

## Hot-reloading

I want for the IRC client to be hot-reloadable:
https://kampffrosch94.github.io/posts/hotreloading_rust/

## First goal

OK, that's a big wish list. Ultimately, I want to connect to irc.libera.chat 6697 and join a
channel and say hello, and quit, that's sort of my first goal, so make a test that does that
(it's okay that it uses networking, I can mock it later).

Save this entire prompt to ./irciii/oneshot.md because I need to change model before I proceed.
