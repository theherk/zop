# JSON Output

`zop list --json` prints one JSON object. It is wrapped in an envelope so the schema can grow.

```json
{
  "version": 1,
  "zoxide": "ok",
  "projects": [
    {
      "path": "/home/me/projects/github.com/example/zop",
      "realpath": "/home/me/projects/github.com/example/zop",
      "name": "zop",
      "id": "zop",
      "kind": "repo",
      "parent": null,
      "visited": true,
      "rank": 0,
      "tmp": "/var/folders/ab/cdef/T/zop/9f2c4b7a1d3e5f60"
    },
    {
      "path": "/home/me/projects/github.com/example/zop-fix-login",
      "realpath": "/home/me/projects/github.com/example/zop-fix-login",
      "name": "zop-fix-login",
      "id": "zop@zop-fix-login",
      "kind": "worktree",
      "parent": "/home/me/projects/github.com/example/zop",
      "visited": false,
      "rank": 1,
      "tmp": "/var/folders/ab/cdef/T/zop/0a7e91c2b4d86f35"
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `version` | Schema version. Currently `1`. |
| `zoxide` | What happened with zoxide: `ok`, `disabled` (`--no-zoxide`), `missing` (not found) or `failed` (error or timeout). Anything but `ok` means every `visited` is `false`. |
| `path` | Absolute path as discovered, keeping the spelling of the root or `--add` argument. This is the key. |
| `realpath` | The resolved path, used for dedupe and as the tmp hash input. |
| `name` | The last component of `path`. |
| `id` | The collision-safe label. See [Ids](../concepts/ids.md). |
| `kind` | `repo`, `worktree` or `extra`. |
| `parent` | For worktrees, the main checkout's path **if it is in the index**. `null` otherwise. |
| `visited` | `true` if zoxide ranked it. |
| `rank` | Zero-based position in this output. Under `--live` it counts the filtered list. |
| `tmp` | The project's [tmp directory](../concepts/tmp.md). Not created. Append a name to build a socket path. |

## Rules for consumers

- Ignore fields you don't know. Ignore `kind` values you don't know. New kinds such as `submodule` may appear later.
- `--tilde` does not affect JSON. Paths are always absolute.
- `--live NAME` filters the list before output, so every project listed has a live socket by that name. Ids are still computed over the full index.
- Check `zoxide`. A consumer that gets `missing` is probably running with a minimal `PATH`. See [Commands](commands.md#global-flags).
- Key anything long-lived by `path`, never by `id`.
