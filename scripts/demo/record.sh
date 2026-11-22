#!/bin/sh
# Re-records docs/assets/demo.cast in a throwaway sandbox of made-up projects.
# Commands in demo.exp use paths relative to the sandbox home, never a bare ~, because
# some shells expand ~ from the passwd database and ignore $HOME.
# Needs: asciinema, expect, fzf, zoxide, jq, git, and a release build of zop.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
sandbox=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$sandbox"' EXIT

mkdir -p "$sandbox/home/projects" "$sandbox/bin" "$sandbox/state" "$sandbox/zo"
cp "$root/target/release/zop" "$sandbox/bin/"

export GIT_CONFIG_GLOBAL=/dev/null GIT_AUTHOR_NAME=d GIT_AUTHOR_EMAIL=d@d \
  GIT_COMMITTER_NAME=d GIT_COMMITTER_EMAIL=d@d
cd "$sandbox/home/projects"
for r in github.com/acme/api github.com/acme/web github.com/acme/docs \
  github.com/jdoe/dotfiles github.com/jdoe/notes github.com/jdoe/docs \
  gitlab.com/acme/infra github.com/jdoe/zop-demo; do
  mkdir -p "$r"
  git -C "$r" init -q -b main
  git -C "$r" commit -q --allow-empty -m init
done
git -C github.com/acme/api worktree add -q ../api-fix-login -b fix-login

cat > "$sandbox/rc.sh" <<ENV
export HOME=$sandbox/home XDG_STATE_HOME=$sandbox/state XDG_CONFIG_HOME=$sandbox/config
export _ZO_DATA_DIR=$sandbox/zo ZOP_TMP_ROOT=$sandbox/tmp ZOP_ZOXIDE=$(command -v zoxide)
export PATH=$sandbox/bin:$(dirname "$(command -v zoxide)"):$(dirname "$(command -v fzf)"):$(dirname "$(command -v jq)"):/usr/bin:/bin
export PS1='\\$ ' FZF_DEFAULT_OPTS='--reverse --no-height --border=rounded --prompt="project> "'
cd "$sandbox/home"
ENV
mkdir -p "$sandbox/tmp"

cd "$root"
asciinema rec --overwrite --headless --window-size 92x26 --idle-time-limit 2 \
  --command "expect $here/demo.exp $sandbox/rc.sh" "$root/docs/assets/demo.cast"

# asciinema writes the command line and environment into the header. Drop them, along with
# the timestamp, so the cast carries no local paths or names.
python3 - "$root/docs/assets/demo.cast" <<'PY'
import json, sys
path = sys.argv[1]
lines = open(path).read().splitlines()
header = json.loads(lines[0])
for key in ("command", "env", "timestamp"):
    header.pop(key, None)
open(path, "w").write(json.dumps(header, separators=(",", ":")) + "\n" + "\n".join(lines[1:]) + "\n")
PY
