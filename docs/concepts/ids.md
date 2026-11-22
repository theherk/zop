# Ids

Sessions and workspaces need a name that is short, unique and legal in every tool. A basename is not enough. Names like `docs`, `api` and `utils` can each exist twice in a project list, and worktrees live in directories named like `main` or `fix-login`.

Every project therefore has an `id`.

> **An id is a label, not a key.** `path` is the key. Use `id` to name a session and show a picker. Never use it to look something up long-term.

## The rule

| Project | Id |
|---|---|
| repo or extra | the shortest suffix of path segments that is unique across the whole list, joined with `~` |
| worktree with a parent in the index | `<parent id>@<directory name>` |
| worktree without a parent in the index | the repo rule, applied to its own path |

Examples:

| Path | Id |
|---|---|
| `~/projects/github.com/example/zop` | `zop` |
| `~/commons` | `me~commons` |
| `~/projects/github.com/example/commons` | `example~commons` |
| `~/projects/github.com/example/zop-fix-login`, a worktree of `zop` | `zop@zop-fix-login` |

The `@` marks a worktree, and `~` joins segments. Both are legal in Zellij session names, and neither collides with `:`, which session pickers commonly split on.

## Why `~`

Zellij rejects `/` in session names (`Session name cannot contain '/'`). `/` would therefore fail for exactly the colliding projects. `~` is rare in repo names and is safe in shells, multiplexers and workspace managers.

## Characters and length

- **Sanitised.** Any character outside `[A-Za-z0-9._@~+-]` in a segment becomes `-`, before uniqueness is checked. A directory name can contain `:`, spaces or control characters, and pickers commonly split on `:`.
- **Bounded.** An id longer than 64 bytes is truncated and gets `~` plus six hex characters. Those come from the SHA-256 of the project's real path. After sanitising everything is ASCII, so truncation can't split a character.
- **Within Zellij's limit.** Zellij 0.46.0 accepts session names of at least 108 bytes, so a 64-byte bound is well inside it.

## Guarantees

- **Computed over the full index**, after entries whose path no longer exists are dropped. Ids never depend on filtering. `zop list --live nvim.sock` and plain `zop list` give the same id for the same project.
- **Unique.** After building ids, `zop` checks for collisions. A collision, including one introduced by a directory literally named `a@b`, is resolved by extending the suffix, and as a last resort by appending `~` and six hex characters of the path hash.
- **Deterministic.** The same index always gives the same ids.
- **Two worktrees of one parent with the same directory name** (`~/wt/a/fix` and `~/wt/b/fix`) resolve with the same suffix rule, applied within that parent.
- **A worktree whose parent isn't in the index** gets an id from the repo rule on its own path. It never refers to a parent that isn't listed.

## Stability

An id changes when the set of colliding projects changes. Cloning a second `docs` renames the existing one, and removing a colliding repo shortens the survivor's id. A running session under the old name then looks like a non-project session, and picking that project would create a second session.

Collisions are rare, and the cost is one orphaned session. Ids are deliberately not stored in the index, so the same set of projects gives the same ids on every machine.
