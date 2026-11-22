# Why zop

Most setups for jumping between projects grow the same way. A script finds repositories, and `zoxide` or a shell history ranks them. A picker such as `fzf` shows the result. Then each tool you use (shell, terminal multiplexer, editor, launcher) gets its own copy of that pipeline, and the copies drift apart.

## What goes wrong

- **Blanket `zoxide add`.** Registering every repo with zoxide each time the list is rebuilt gives all of them the same score. A project you use daily looks the same as one you never opened, and ties come out in arbitrary order.
- **Copied pipelines.** `zoxide query -l | filter | sed | fzf` appears in every tool. Each copy has its own filter, and a fix has to be made everywhere.
- **Inconsistent names.** One tool names a session by basename, another by path. Two projects called `docs` collide, and a lookup by name silently picks the first.
- **Hidden depth limits.** A shallow `find` or `fd` depth cap that keeps submodules out by accident also hides deep repos.
- **Invisible visits.** Tools that open a project without a `cd` never tell zoxide, so those projects never rise in the ranking.
- **Sockets inside repos.** An editor socket at `<repo>/_neovim` litters the repo and leaves stale files. It also overflows the unix socket path limit for deep repos, and every reader needs to know the workaround.

## What zop does instead

| Problem | zop |
|---|---|
| Every repo has the same score | Reads zoxide's order and never writes scores in bulk. Unvisited projects follow, sorted by path. |
| Copied pipelines | One `zop list`, plain or `--json`. |
| Inconsistent names | A unique, tool-safe `id` per project. |
| Hidden depth limits | Marker-based discovery that stops at the first `.git` or `.jj`, with an explicit `--max-depth`. |
| Invisible visits | `zop visit PATH` records one project you actually opened. |
| Sockets in repos | `zop tmp` gives a short, owner-checked path in your private temporary directory, and `--live` tells you whether anything is listening. |
