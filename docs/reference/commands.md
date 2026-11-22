# Commands

```
zop [GLOBAL] [list] [OPTIONS]
zop [GLOBAL] refresh [OPTIONS]
zop [GLOBAL] tmp PATH [NAME] [--create | --live]
zop [GLOBAL] tmp --all NAME [--live]
zop [GLOBAL] visit PATH|-
zop [GLOBAL] completions SHELL
```

`zop` with no command is `zop list`. The rewrite rules are in [Configuration](configuration.md).

## Conventions

- **Streams.** Data goes to stdout. Diagnostics go to stderr.
- **Exit codes.** `0` success. `1` a negative answer (`tmp --live` found no live socket). `2` usage error or failure. A `list` that matches nothing, including `list --live NAME`, is `0` with empty output.
- **Booleans are pairs.** Each toggle has a `--x` and a `--no-x`, and the last one on the line wins. This is what lets the command line override the [config file](configuration.md).
- **Expansion.** `zop` expands `~`, `$VAR` and `${VAR}` itself in `--root`, `--add`, `--index`, `--config`, `--zoxide-path` and config values. It does so because a shell does not expand `~` after `=`. ripgrep does no expansion, so this is a deliberate difference. A literal `$` is `\$`. An undefined variable is an error.
- **Repeatable flags** (`--root`, `--add`) accumulate. They can't be negated from the command line. Use `--no-config` to drop the config's values.

## Global flags

| Flag | Meaning |
|---|---|
| `--config FILE` | Config file. Also `ZOP_CONFIG_PATH`. An explicit file that doesn't exist is an error. The default file may be absent. |
| `--no-config` | Ignore the config file entirely. |
| `--index FILE` | Index file for `list` and `refresh`. Also `ZOP_INDEX`. Default `${XDG_STATE_HOME:-~/.local/state}/zop/projects`. |
| `--zoxide-path FILE` | The `zoxide` binary. Also `ZOP_ZOXIDE`. Default: `zoxide` on `PATH`, then `/opt/homebrew/bin/zoxide`, `/usr/local/bin/zoxide` and the `mise` shim. |

GUI apps often start with launchd's minimal `PATH` and without your shell's `XDG_*` variables. Set `ZOP_ZOXIDE` and `ZOP_INDEX` explicitly for them, or `zop` may quietly use a different index and rank without zoxide.

## `zop list`

Prints every project, ranked. See [Ranking and Visits](../concepts/ranking.md).

| Flag | Meaning |
|---|---|
| `--tilde` / `--no-tilde` | Collapse `$HOME` to `~` in plain output. No effect on `--json`. |
| `--json` / `--no-json` | Print the [JSON envelope](json.md) instead of paths. |
| `--zoxide` / `--no-zoxide` | Use zoxide for the ranked head. Default on. A missing or failing zoxide degrades to the sorted list and is reported in the JSON `zoxide` field. |
| `--live NAME` | Keep only projects whose `zop tmp` socket `NAME` accepts a connection. Read-only. |
| `--refresh-if-older DUR` / `--no-refresh-if-older` | Refresh first if the index is older than `DUR` (`30m`, `12h`, `1d`). |

`list` runs an implicit refresh when the index is missing, when its format version is older than this build, or when `--refresh-if-older` says it is stale. Its inputs come from the config's `[refresh]` section, or else from the inputs stored in the index. With neither, `list` exits 2 and writes nothing. An implicit refresh skips pruning. See [Index and State](index-file.md).

## `zop refresh`

Rescans and rewrites the index, then prunes stale tmp state. See [Projects](../concepts/projects.md) and [Index and State](index-file.md).

| Flag | Meaning |
|---|---|
| `--root DIR` | Walk `DIR` for projects. Repeatable. A missing or unreadable root is a fatal error and the old index is kept. |
| `--add PATH` | Add `PATH`. A directory is classified by its markers and is an `extra` only if it has none. A regular file is read as one path per line. Repeatable. |
| `--worktrees` / `--no-worktrees` | Include linked worktrees and jj workspaces. Default on. |
| `--max-depth N` | Walk depth limit. Default 12. |
| `--prune` / `--no-prune` | Prune stale tmp state afterwards. Default on. |

With no `--root` or `--add`, `refresh` uses the config's `[refresh]` section, or else the inputs stored in the index, so a bare `zop refresh` repeats the last one. With none of those it is a usage error.

An `--add` file ignores blank lines and `#` comments, expands `~` and `$VAR`, resolves relative paths against the file's own directory, and warns on stderr about paths that don't exist. A line with an undefined `$VAR` also warns, naming the variable, and is skipped. It doesn't abort the refresh, so one machine without `$ONEDRIVE` still gets the rest of the list. A file can't itself contain `--add` lines. `~/commons/.projects-extra` works unchanged. A missing `--add` directory also warns and is skipped. Only a missing `--root` is fatal.

## `zop tmp`

Prints a per-project tmp path. Path arguments are taken as raw bytes, so a path that isn't valid UTF-8 works the same as any other. See [Per-project tmp](../concepts/tmp.md).

```
zop tmp PATH [NAME] [--create | --live]
zop tmp --all NAME [--live]
```

| Flag | Meaning |
|---|---|
| `--create` | Create the hash directory (mode 0700) and its `path` marker, atomically. |
| `--live` | With a `NAME`, exit 0 only if a socket there accepts a connection. Read-only. |
| `--all NAME` | Print every existing `BASE/*/NAME`, found by scanning the hash directories, not by reading the markers. With `--live`, only sockets that accept a connection. |

`PATH` must be the project root. `--create` and `--live` are mutually exclusive. `--all` takes no `PATH`. Exit codes for `--live` are 0 live, 1 not live, 2 error.

## `zop visit`

```
zop visit PATH
```

Runs `zoxide add -- PATH`. With `-` as `PATH`, it reads one path per line from stdin, skipping blank lines, and adds each. If any line can't be read or expanded, `visit` exits 2 naming `stdin` and the line number, and records nothing. Otherwise it exits 0. If zoxide can't be found it warns on stderr, so a lost visit is noticeable, and still exits 0. A failing `zoxide add` also warns and exits 0. `PATH` need not be in the index and `visit` never reads it. See [Ranking and Visits](../concepts/ranking.md#zop-visit).

## `zop completions`

```
zop completions SHELL
```

Prints a completion script for `bash`, `elvish`, `fish`, `powershell` or `zsh`. Nushell needs the separate `clap_complete_nushell` crate.

## Not supported

`zop` does not descend into submodules or nested clones, has no flag for the id separator, and has no nushell completions.
