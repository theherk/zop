#!/bin/sh
# Builds a throwaway sandbox of made-up projects for demos and screencasts, so nothing
# real or sensitive is on screen. Nothing outside the sandbox is read or written.
#
#   scripts/demo/sandbox.sh DIR       create DIR (must not exist) and print how to enter it
#
# Then, in a fresh shell, or in whatever you are recording (a Zellij session, Rex, a
# terminal):
#
#   . DIR/env.sh                      point HOME, zop, zoxide and tmp at the sandbox
#   zop refresh --root projects
#
# The sandbox has its own HOME, zop index, zoxide database and zop tmp area. A real
# `zop` and `zoxide` on PATH are used, but both are told to use the sandbox's data.
# Needs: git, zoxide, fzf, jq, and a release build of zop (cargo build --release).
set -eu

[ $# -eq 1 ] || { echo "usage: $0 DIR" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
[ ! -e "$1" ] || { echo "$1 already exists" >&2; exit 1; }
mkdir -p "$1"
dir=$(cd "$1" && pwd -P)

[ -x "$root/target/release/zop" ] || { echo "run: cargo build --release" >&2; exit 1; }
mkdir -p "$dir/home/projects" "$dir/bin" "$dir/state" "$dir/zo" "$dir/tmp" "$dir/config"
cp "$root/target/release/zop" "$dir/bin/"

export GIT_CONFIG_GLOBAL=/dev/null GIT_AUTHOR_NAME=d GIT_AUTHOR_EMAIL=d@d \
  GIT_COMMITTER_NAME=d GIT_COMMITTER_EMAIL=d@d
cd "$dir/home/projects"
for r in \
  github.com/acme/api github.com/acme/web github.com/acme/docs github.com/acme/mobile \
  github.com/acme/design-system github.com/jdoe/dotfiles github.com/jdoe/notes \
  github.com/jdoe/docs github.com/jdoe/zop-demo github.com/jdoe/blog \
  gitlab.com/acme/infra gitlab.com/acme/ops; do
  mkdir -p "$r"
  git -C "$r" init -q -b main
  git -C "$r" commit -q --allow-empty -m init
done
git -C github.com/acme/api worktree add -q ../api-fix-login -b fix-login
git -C github.com/acme/web worktree add -q ../web-redesign -b redesign

tool_dir() { dirname "$(command -v "$1")"; }
cat > "$dir/env.sh" <<ENV
# Source this. It points everything at the sandbox, never at your real data.
export HOME=$dir/home
export XDG_STATE_HOME=$dir/state XDG_CONFIG_HOME=$dir/config XDG_DATA_HOME=$dir/data
export _ZO_DATA_DIR=$dir/zo
export ZOP_TMP_ROOT=$dir/tmp ZOP_INDEX=$dir/state/zop/projects ZOP_ZOXIDE=$(command -v zoxide)
export ZELLIJ_CONFIG_DIR=$dir/config/zellij ZELLIJ_SOCKET_DIR=$dir/zellij
export PATH=$dir/bin:$(tool_dir zoxide):$(tool_dir fzf):$(tool_dir jq):$(tool_dir git):/usr/bin:/bin:/usr/sbin:/sbin
cd "\$HOME"
ENV
mkdir -p "$dir/data" "$dir/zellij"

echo "sandbox ready: $dir"
echo "  12 repos and 2 worktrees under $dir/home/projects"
echo "  enter it with:  . $dir/env.sh"
echo "  tear it down with:  rm -rf $dir"
