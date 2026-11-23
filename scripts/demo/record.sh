#!/bin/sh
# Re-records docs/assets/demo.cast and docs/assets/demo.gif in a throwaway sandbox of
# made-up projects (see sandbox.sh). Commands in demo.exp use paths relative to the
# sandbox home, never a bare ~, because some shells expand ~ from the passwd database
# and ignore $HOME.
# Needs: asciinema, agg, expect, fzf, zoxide, jq, git, and a release build of zop.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
work=$(cd "$(mktemp -d)" && pwd -P)
trap 'rm -rf "$work"' EXIT
sandbox=$work/sandbox

"$here/sandbox.sh" "$sandbox" >/dev/null

# demo.exp starts a clean bash that sources this.
cat > "$work/rc.sh" <<ENV
. $sandbox/env.sh
export PS1='\\$ ' FZF_DEFAULT_OPTS='--reverse --no-height --border=rounded --prompt="project> "'
ENV

cd "$root"
asciinema rec --overwrite --headless --window-size 92x26 --idle-time-limit 2 \
  --command "expect $here/demo.exp $work/rc.sh" "$root/docs/assets/demo.cast"

# asciinema writes the command line and environment into the header. Drop them, along with
# the timestamp, so the cast carries no local paths or names.
python3 - "$root/docs/assets/demo.cast" <<'PY'
import json, re, sys
path = sys.argv[1]
lines = open(path).read().splitlines()
header = json.loads(lines[0])
for key in ("command", "env", "timestamp"):
    header.pop(key, None)
body = "\n".join(lines[1:])
open(path, "w").write(json.dumps(header, separators=(",", ":")) + "\n" + body + "\n")
text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", body)
leaks = [w for w in ("/Users/", "/home/", "/var/folders", "/private/", "tmp.") if w in text]
if leaks:
    sys.exit(f"the recording contains local paths: {leaks}")
PY

agg --theme dracula --font-size 16 --quiet "$root/docs/assets/demo.cast" "$root/docs/assets/demo.gif"
