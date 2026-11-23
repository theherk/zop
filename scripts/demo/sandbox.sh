#!/bin/sh
# Builds a throwaway sandbox of made-up projects for demos and screencasts, so nothing
# real or sensitive is on screen. Nothing outside the sandbox is read or written.
#
#   scripts/demo/sandbox.sh [--fish[=DIR]] [--copy FILE]... SANDBOX
#
#   --fish[=DIR]  Copy your fish setup into the sandbox, so the prompt, abbreviations and
#                 functions look as they do for you. DIR defaults to
#                 ${XDG_CONFIG_HOME:-~/.config}/fish. Symlinks are followed, and
#                 fish_variables is left out, so the sandbox cannot change your real
#                 config. starship.toml is copied too. Shell history (atuin) is not.
#   --zellij      Copy your Zellij config (config.kdl, layouts, themes) into the sandbox,
#                 and the plugin files its layouts load from ~/bin (*.wasm that the
#                 config names). The plugin permissions Zellij saved for you are copied
#                 too, so the plugins do not stop to ask again. Zellij reads its config
#                 from ZELLIJ_CONFIG_DIR, which the sandbox sets.
#   --wezterm     Make SANDBOX/wezterm, which opens a WezTerm window inside the sandbox.
#                 It runs a separate WezTerm process, so it does not touch the windows
#                 you have open. Your WezTerm config is copied into the sandbox, because
#                 its modules (require "theme") are found under HOME. The plugin is
#                 copied from your WezTerm plugin cache, so nothing is downloaded, and a
#                 zop shim is put where the config expects the mise one. The sandbox
#                 holds a frozen copy: rebuild it to pick up edits. The window needs a
#                 display, so it cannot be tested from a script.
#   --rex         Make SANDBOX/rex, which starts a separate Rex server inside the sandbox and
#                 opens a Rex window on it. The server has its own socket and state, so
#                 it never touches your sessions, and the window connects to it only
#                 (REX_HOME and REX_SERVER). Your Rex config (init.lua and its modules)
#                 is copied in. It refuses to start while your real Rex app or server is
#                 running, since that would risk them; quit Rex first, and open it again
#                 afterwards. SANDBOX/rex stop ends the sandbox's Rex and nothing else.
#                 The window needs a display, so it cannot be tested from a script.
#   --config NAME Copy ${XDG_CONFIG_HOME:-~/.config}/NAME into the sandbox's config
#                 directory (repeatable), for tools that read it, such as ripgrep, bat,
#                 eza or atuin. Symlinks are followed.
#   --copy FILE   Copy FILE into the sandbox's bin directory (repeatable). Use it for
#                 scripts you want to run on camera, such as zp.
#
# Enter it with SANDBOX/enter. It opens a shell inside the sandbox, so there is nothing
# to source and it works from any shell. Leave with `exit`. Tear down with rm -rf.
#
# The sandbox has its own HOME, zop index, zoxide database, zop tmp area and Zellij
# socket directory. A real `zop`, `zoxide`, `fzf` and `jq` are used, but all are told to
# use the sandbox's data.
#
# Run this from a normal shell, not from inside a sandbox: there `~` and $HOME are the
# sandbox, so paths to your real files would point at the wrong place.
#
# RIPGREP_CONFIG_PATH points at a file that may not exist in the sandbox; ripgrep warns
# about that, so the sandbox env leaves it unset unless --config ripgrep copied it.
#
# ZOP_SANDBOX_DENY is an extra regular expression for words you never want on screen,
# for example 'myemployer|secret-project'. Copied config is scanned for it.
#
# Needs: git, zoxide, fzf, jq, and a release build of zop (cargo build --release).
set -eu

usage() {
  sed -n '2,/^set -eu/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2
  exit 2
}

fish_src=""
want_fish=no
want_zellij=no
want_wezterm=no
want_rex=no
copies=""
configs=""
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    --zellij) want_zellij=yes ;;
    --wezterm) want_wezterm=yes ;;
    --rex) want_rex=yes ;;
    --fish) want_fish=yes ;;
    --fish=*) want_fish=yes; fish_src=${1#--fish=} ;;
    --copy)
      [ $# -ge 2 ] || usage
      copies="$copies
$2"
      shift
      ;;
    --config)
      [ $# -ge 2 ] || usage
      configs="$configs
$2"
      shift
      ;;
    --config=*) configs="$configs
${1#--config=}" ;;
    --copy=*) copies="$copies
${1#--copy=}" ;;
    -h|--help) usage ;;
    -*) echo "unknown option: $1" >&2; usage ;;
    *)
      [ -z "$dir" ] || usage
      dir=$1
      ;;
  esac
  shift
done
[ -n "$dir" ] || usage

# Inside a sandbox, ~ and $HOME are the sandbox's, so a path like ~/commons/... points at
# the wrong place. Sandboxes made before ZOP_SANDBOX existed are recognised by their
# layout: $HOME is SANDBOX/home, next to SANDBOX/env.sh.
inside=${ZOP_SANDBOX:-}
if [ -z "$inside" ] && [ "$(basename "$HOME")" = home ] && [ -f "$(dirname "$HOME")/env.sh" ]; then
  inside=$(dirname "$HOME")
fi
if [ -n "$inside" ]; then
  echo "you are inside a sandbox ($inside), where ~ is the sandbox's home." >&2
  echo "run this from a normal shell: type exit, or open a new terminal." >&2
  exit 1
fi

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
[ ! -e "$dir" ] || { echo "$dir already exists" >&2; exit 1; }
[ -x "$root/target/release/zop" ] || { echo "run: cargo build --release" >&2; exit 1; }

# Check the inputs before creating anything.
if [ "$want_fish" = yes ]; then
  fish_src=${fish_src:-${XDG_CONFIG_HOME:-$HOME/.config}/fish}
  [ -d "$fish_src" ] || { echo "no fish config at $fish_src" >&2; exit 1; }
  command -v fish >/dev/null || { echo "fish is not installed" >&2; exit 1; }
fi
wezterm_src=${XDG_CONFIG_HOME:-$HOME/.config}/wezterm
wezterm_plugins="$HOME/Library/Application Support/wezterm/plugins"
if [ "$want_wezterm" = yes ]; then
  command -v wezterm >/dev/null || { echo "wezterm is not installed" >&2; exit 1; }
  [ -f "$wezterm_src/wezterm.lua" ] || { echo "no WezTerm config at $wezterm_src/wezterm.lua" >&2; exit 1; }
fi
rex_src=${XDG_CONFIG_HOME:-$HOME/.config}/rex
rex_app=/Applications/Rex.app
if [ "$want_rex" = yes ]; then
  [ -d "$rex_app" ] || { echo "Rex.app is not in /Applications" >&2; exit 1; }
  [ -f "$rex_src/init.lua" ] || { echo "no Rex config at $rex_src/init.lua" >&2; exit 1; }
  rex_cli=$(ls -d "$HOME/Library/Application Support/com.superlogical.rex/RexServer/Versions/Current/Rex Server.app/Contents/MacOS/rex" 2>/dev/null || command -v rex || true)
  [ -n "$rex_cli" ] || { echo "cannot find the rex command" >&2; exit 1; }
fi
zellij_src=${XDG_CONFIG_HOME:-$HOME/.config}/zellij
if [ "$want_zellij" = yes ]; then
  [ -f "$zellij_src/config.kdl" ] || { echo "no Zellij config at $zellij_src/config.kdl" >&2; exit 1; }
fi
printf '%s\n' "$copies" | while IFS= read -r f; do
  [ -z "$f" ] || [ -f "$f" ] || { echo "--copy: no such file: $f (use an absolute path from your real home)" >&2; exit 1; }
done
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
printf '%s\n' "$configs" | while IFS= read -r c; do
  [ -z "$c" ] || [ -e "$config_home/$c" ] || { echo "--config: no such config: $config_home/$c" >&2; exit 1; }
done

mkdir -p "$dir"
dir=$(cd "$dir" && pwd -P)
# The config directory is $HOME/.config, because configs such as fish's set
# XDG_CONFIG_HOME to that path whatever it was before.
mkdir -p "$dir/home/projects" "$dir/home/.config" "$dir/bin" "$dir/state" "$dir/zo" \
  "$dir/tmp" "$dir/data" "$dir/zellij"
cp "$root/target/release/zop" "$dir/bin/"

export GIT_CONFIG_GLOBAL=/dev/null GIT_AUTHOR_NAME=d GIT_AUTHOR_EMAIL=d@d \
  GIT_COMMITTER_NAME=d GIT_COMMITTER_EMAIL=d@d
(
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
)

printf '%s\n' "$copies" | while IFS= read -r f; do
  [ -z "$f" ] || { cp "$f" "$dir/bin/"; chmod +x "$dir/bin/$(basename "$f")"; }
done

tool_dir() { dirname "$(command -v "$1")"; }
base_path="$dir/bin:$(tool_dir zoxide):$(tool_dir fzf):$(tool_dir jq):$(tool_dir git)"
if [ "$want_fish" = yes ]; then
  # fish's config calls starship, atuin, carapace and the like, so keep the real PATH
  # after the sandbox's. Everything in it still sees the sandbox HOME and data.
  path_line="$base_path:$PATH"
else
  path_line="$base_path:/usr/bin:/bin:/usr/sbin:/sbin"
fi

cat > "$dir/env.sh" <<ENV
# Source this. It points everything at the sandbox, never at your real data.
export ZOP_SANDBOX=$dir
export HOME=$dir/home
export XDG_STATE_HOME=$dir/state XDG_CONFIG_HOME=$dir/home/.config XDG_DATA_HOME=$dir/data
export _ZO_DATA_DIR=$dir/zo
export ZOP_TMP_ROOT=$dir/tmp ZOP_INDEX=$dir/state/zop/projects ZOP_ZOXIDE=$(command -v zoxide)
export ZELLIJ_CONFIG_DIR=$dir/home/.config/zellij ZELLIJ_SOCKET_DIR=$dir/zellij
export PATH="$path_line"
# mise looks for config in every parent directory. Stop it at the sandbox, so a sandbox
# placed under your real home does not pick up your real mise config.
export MISE_CEILING_PATHS=$dir
unset RIPGREP_CONFIG_PATH
[ ! -f "\$XDG_CONFIG_HOME/ripgrep/ripgreprc" ] || export RIPGREP_CONFIG_PATH="\$XDG_CONFIG_HOME/ripgrep/ripgreprc"
cd "\$HOME"
ENV

printf '%s\n' "$configs" | while IFS= read -r c; do
  [ -z "$c" ] || cp -RL "$config_home/$c" "$dir/home/.config/"
done

if [ "$want_rex" = yes ]; then
  mkdir -p "$dir/home/.config/rex" "$dir/state/rex"
  cp -RL "$rex_src/." "$dir/home/.config/rex/"
fi

if [ "$want_wezterm" = yes ]; then
  # WezTerm finds `require "theme"` and friends next to the config under $HOME/.config,
  # so the whole config directory comes along, not only wezterm.lua.
  wezterm_bin=$(command -v wezterm)
  mkdir -p "$dir/home/.config/wezterm" "$dir/home/.local/share/wezterm"
  cp -RL "$wezterm_src/." "$dir/home/.config/wezterm/"
  # Plugins are cached by URL under Application Support. Copy the ones already fetched.
  mkdir -p "$dir/home/Library/Application Support/wezterm/plugins"
  if [ -d "$wezterm_plugins" ]; then
    cp -RL "$wezterm_plugins/." "$dir/home/Library/Application Support/wezterm/plugins/"
  fi
  # The config calls the mise shim by absolute path under HOME. Point that at zop.
  mkdir -p "$dir/home/.local/share/mise/shims"
  ln -s "$dir/bin/zop" "$dir/home/.local/share/mise/shims/zop"
fi

if [ "$want_zellij" = yes ]; then
  mkdir -p "$dir/home/.config/zellij" "$dir/home/bin"
  cp -RL "$zellij_src/." "$dir/home/.config/zellij/"
  # The config names plugins as file:~/bin/NAME.wasm. In the sandbox ~ is $dir/home, so
  # copy the ones it names, and only those, from your real ~/bin.
  # Scripts copied with --copy name plugins too (zp calls zellij-switch.wasm).
  grep -rhoE 'file:~/bin/[A-Za-z0-9._-]+\.wasm' "$dir/home/.config/zellij" "$dir/bin" 2>/dev/null |
    sed 's|file:~/bin/||' | sort -u | while IFS= read -r w; do
      if [ -f "$HOME/bin/$w" ]; then
        cp -L "$HOME/bin/$w" "$dir/home/bin/$w"
      else
        echo "warning: the Zellij config names ~/bin/$w, which does not exist" >&2
      fi
    done
  # Saved plugin permissions are keyed by the plugin's absolute path. Rewrite your
  # ~/bin paths to the sandbox's, and keep only those, so nothing else is exposed.
  perms_src=$HOME/Library/Caches/org.Zellij-Contributors.Zellij/permissions.kdl
  perms_dir=$dir/home/Library/Caches/org.Zellij-Contributors.Zellij
  mkdir -p "$perms_dir"
  if [ -f "$perms_src" ]; then
    for w in "$dir"/home/bin/*.wasm; do
      [ -f "$w" ] || continue
      name=$(basename "$w")
      awk -v real="\"$HOME/bin/$name\"" -v sand="\"$dir/home/bin/$name\"" '
        $0 == real " {" { print sand " {"; on = 1; next }
        on { print; if ($0 == "}") on = 0 }
      ' "$perms_src" >> "$perms_dir/permissions.kdl"
    done
  fi
fi

if [ "$want_fish" = yes ]; then
  conf=$dir/home/.config
  mkdir -p "$conf/fish"
  # cp -L follows symlinks, so the copy holds real files and never links back to your config.
  for item in config.fish conf.d functions completions themes fish_plugins; do
    [ ! -e "$fish_src/$item" ] || cp -RL "$fish_src/$item" "$conf/fish/"
  done
  starship_src=$(dirname "$fish_src")/starship.toml
  [ ! -e "$starship_src" ] || cp -L "$starship_src" "$conf/"
  [ ! -e "$HOME/.cargo/env.fish" ] || { mkdir -p "$dir/home/.cargo"; : > "$dir/home/.cargo/env.fish"; }
fi

# A login fish shell resets a few variables (it sets ZELLIJ_SOCKET_DIR to a real
# directory), so put the sandbox's back after the config has run.
cat > "$dir/enter" <<ENTER
#!/bin/sh
# Opens a shell inside the sandbox. Leave it with \`exit\`.
. "$dir/env.sh"
if [ -d "$dir/home/.config/fish" ]; then
  exec fish -C "set -gx ZELLIJ_SOCKET_DIR '$dir/zellij'; set -gx ZELLIJ_CONFIG_DIR '$dir/home/.config/zellij'; set -gx ZOP_SANDBOX '$dir'; set -gx HOME '$dir/home'; set -e RIPGREP_CONFIG_PATH; test -f '$dir/home/.config/ripgrep/ripgreprc'; and set -gx RIPGREP_CONFIG_PATH '$dir/home/.config/ripgrep/ripgreprc'"
fi
exec "\${SHELL:-/bin/sh}"
ENTER
chmod +x "$dir/enter"

if [ "$want_wezterm" = yes ]; then
  cat > "$dir/wezterm" <<WEZ
#!/bin/sh
# Opens a WezTerm window inside the sandbox, as its own process, with the config copied
# into the sandbox. --always-new-process keeps it from handing the window to a WezTerm
# you already have open, which would use your real HOME.
. "$dir/env.sh"
exec "$wezterm_bin" --config-file "$dir/home/.config/wezterm/wezterm.lua" \\
  start --always-new-process --class zop-sandbox --cwd "$dir/home" "\$@"
WEZ
  chmod +x "$dir/wezterm"
fi

if [ "$want_rex" = yes ]; then
  cat > "$dir/rex" <<'REX'
#!/bin/sh
# usage: rex [start|stop|status]   (start is the default)
set -eu
REX
  cat >> "$dir/rex" <<REX
dir="$dir"
cli="$rex_cli"
sock="\$dir/rex.sock"
pidfile="\$dir/rex-server.pid"
REX
  cat >> "$dir/rex" <<'REX'

# A real Rex is anything running that is not this sandbox's own server or app.
real_rex_running() {
  pgrep -f "Rex.app/Contents/MacOS/Rex" >/dev/null 2>&1 && return 0
  pgrep -f "Rex Server.app/Contents/MacOS/rex server" 2>/dev/null | while read -r pid; do
    case "$(ps -o command= -p "$pid" 2>/dev/null)" in
      *"$dir/rex.sock"*) ;;
      *) echo "$pid" ;;
    esac
  done | grep -q .
}

sandbox_server_pid() {
  [ -f "$pidfile" ] || return 1
  pid=$(cat "$pidfile")
  case "$(ps -o command= -p "$pid" 2>/dev/null)" in
    *"$dir/rex.sock"*) echo "$pid" ;;
    *) return 1 ;;
  esac
}

stop_sandbox() {
  # Only processes whose command line names this sandbox are ever signalled.
  for pid in $(pgrep -f "Rex.app/Contents/MacOS/Rex" 2>/dev/null || true); do
    if ps -E -p "$pid" -o command= 2>/dev/null | grep -q "REX_SERVER=unix://$sock"; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  if pid=$(sandbox_server_pid); then kill "$pid" 2>/dev/null || true; fi
  rm -f "$pidfile" "$sock"
}

case "${1:-start}" in
  start)
    if sandbox_server_pid >/dev/null; then
      echo "the sandbox's Rex server is already running"
    else
      if real_rex_running; then
        echo "your real Rex is running. Quit the app, and the server it started, then run this again." >&2
        echo "  quit the app, then: rex server stop   (opens again when you launch Rex)" >&2
        exit 1
      fi
      rm -f "$sock"
      (
        . "$dir/env.sh"
        nohup "$cli" server run --listen "unix://$sock" --state-dir "$dir/state/rex"           --config "$dir/home/.config/rex/init.lua" --restore=false           >"$dir/rex-server.log" 2>&1 &
        echo $! > "$pidfile"
      )
      i=0
      while [ ! -S "$sock" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
      [ -S "$sock" ] || { echo "the sandbox server did not start; see $dir/rex-server.log" >&2; exit 1; }
    fi
    . "$dir/env.sh"
    export REX_HOME="unix://$sock" REX_SERVER="unix://$sock"
    exec open -n -a Rex       --env "HOME=$HOME" --env "REX_HOME=$REX_HOME" --env "REX_SERVER=$REX_SERVER"       --env "XDG_CONFIG_HOME=$XDG_CONFIG_HOME" --env "XDG_STATE_HOME=$XDG_STATE_HOME"       --env "PATH=$PATH" --env "ZOP_INDEX=$ZOP_INDEX" --env "ZOP_TMP_ROOT=$ZOP_TMP_ROOT"       --env "ZOP_ZOXIDE=$ZOP_ZOXIDE" --env "_ZO_DATA_DIR=$_ZO_DATA_DIR"
    ;;
  stop)
    stop_sandbox
    echo "stopped the sandbox's Rex"
    ;;
  status)
    if pid=$(sandbox_server_pid); then echo "sandbox server running (pid $pid)"; else echo "sandbox server not running"; fi
    if real_rex_running; then echo "a real Rex is running too"; fi
    ;;
  *) echo "usage: rex [start|stop|status]" >&2; exit 2 ;;
esac
REX
  chmod +x "$dir/rex"
fi

# Build the index now, so `zp` and `p` work the moment you enter.
(
  . "$dir/env.sh"
  zop refresh --root projects >/dev/null 2>&1
)

# Look for anything personal in what was copied. It is only a warning: a line in a
# config does not mean it shows on screen, but check it before recording.
if [ "$want_fish" = yes ] || [ "$want_zellij" = yes ] || [ "$want_wezterm" = yes ] || [ "$want_rex" = yes ] || [ -n "$copies" ] || [ -n "$configs" ]; then
  real_home=${HOME_REAL:-$(cd ~ 2>/dev/null && pwd -P || echo "$HOME")}
  pattern=$(printf '%s|%s|%s' "$real_home" "$(id -un)" "$(hostname -s 2>/dev/null || echo __nohost__)")
  [ -z "${ZOP_SANDBOX_DENY:-}" ] || pattern="$pattern|$ZOP_SANDBOX_DENY"
  found=$(grep -RInE --exclude=zop -e "$pattern" "$dir/home/.config" "$dir/bin" 2>/dev/null | cut -c1-150 || true)
  if [ -n "$found" ]; then
    echo "warning: copied files mention your home, name, host or a denied word:" >&2
    printf '%s\n' "$found" | sed "s|$dir/||" >&2
    echo "  check these before recording, or edit them in the sandbox." >&2
  fi
fi

echo "sandbox ready: $dir"
echo "  12 repos and 2 worktrees under $dir/home/projects"
[ "$want_wezterm" = no ] || echo "  open WezTerm in it with:  $dir/wezterm"
[ "$want_rex" = no ] || echo "  open Rex in it with:  $dir/rex   (stop it with: $dir/rex stop)"
[ "$want_zellij" = no ] || echo "  Zellij config, its plugins and their saved permissions copied"
[ "$want_fish" = no ] || echo "  fish config copied from $fish_src (without fish_variables or history)"
echo "  enter it with:  $dir/enter"
echo "  tear it down with:  rm -rf $dir"
