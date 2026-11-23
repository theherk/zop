<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/zop-dark.svg">
    <img src="docs/assets/zop-light.svg" width="96" alt="zop">
  </picture>
</p>

# zop

Every project, ranked by use.

`zop` keeps one list of all your projects: the repos under your roots plus any extra paths. Projects [zoxide](https://github.com/ajeetdsouza/zoxide) has seen come first, in zoxide's order. Everything else follows, sorted by path. It also gives each project a stable, short-lived, private place for sockets, locks and logs.

```
zop list                         # every project, ranked
zop refresh --root ~/projects    # rescan roots, rewrite the index, prune stale tmp state
zop tmp ROOT nvim.sock --live    # per-project tmp path; succeeds if the socket is live
zop visit PATH                   # tell zoxide this project was opened
```

![zop picking a project with fzf](docs/assets/demo.gif)

The recording is made in a sandbox of made-up projects. `scripts/demo/record.sh` regenerates `docs/assets/demo.cast`.

## Install

Until the crate and the tap are published, install from git or download a release binary.

```
cargo install --git https://github.com/theherk/zop --tag 0.2.0
```

Release binaries for Linux and macOS (amd64 and arm64) are on the [releases page](https://github.com/theherk/zop/releases). Download the one for your platform, check it against its `.sha256` file, and make it executable:

```
curl -LO https://github.com/theherk/zop/releases/download/0.2.0/zop-darwin-arm64
chmod +x zop-darwin-arm64 && mv zop-darwin-arm64 ~/.local/bin/zop
```

Once published, these work too:

```
cargo install zop
brew install theherk/tap/zop
```

With [mise](https://mise.jdx.dev), which picks the right release binary for your platform and pins the version, no crate or tap needed:

```
mise use -g github:theherk/zop
```

## In use

`zop` is not a switcher. It is the list behind one. Each tool below draws its own picker and calls `zop visit` after a choice. The projects, their order and their ids come from `zop`.

| Zellij, in Ghostty | WezTerm | Rex |
|:---:|:---:|:---:|
| ![Zellij session switcher fed by zop](docs/assets/demo-zellij.gif) | ![WezTerm workspace switcher fed by zop](docs/assets/demo-wezterm.gif) | ![Rex project switcher fed by zop](docs/assets/demo-rex.gif) |

How each is wired up is on the [In use](docs/guide/in-use.md) page, with links to the scripts and configuration. Raycast, VS Codium and Emacs Projectile are not documented yet, but anything that can run a command and read lines can use `zop` now.

## Docs

The documentation is an [mdBook](https://rust-lang.github.io/mdBook/) under [`docs/`](docs/). Start at [`docs/introduction.md`](docs/introduction.md), or read it on GitHub Pages.

## Development

Tools and tasks come from [mise](https://mise.jdx.dev). Rust is expected from your own toolchain.

```
mise install            # mdBook (pinned) and node
mise run check          # lint, tests, and the docs checks
mise run docs:serve     # the book, with live reload
mise tasks              # everything else
```

## Licence

[MIT](LICENSE).
