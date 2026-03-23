#!/bin/bash
# Usage:
#   Terminal 1: ./test-attach.sh top
#   Terminal 2: ./test-attach.sh bot
set -e
export PATH="$(cd "$(dirname "$0")" && pwd)/target/debug:$PATH"
cargo build --quiet

case "${1:-}" in
  top)
    snag daemon stop 2>/dev/null || true
    sleep 0.5
    snag daemon start 2>/dev/null &
    sleep 1
    echo "=== Registering this shell with snag ==="
    eval "$(snag hook bash)"
    echo "=== Registered as: $SNAG_SESSION ==="
    snag list
    echo ""
    echo "Now open another Konsole tab and run: ./test-attach.sh bot"
    exec bash
    ;;
  bot)
    eval "$(snag hook bash)"
    echo "=== Sessions ==="
    snag list
    echo ""
    # Find the other session (not ours)
    OTHER=$(snag list --json | python3 -c "
import sys,json
sessions = json.loads(sys.stdin.read())['sessions']
mine = '${SNAG_SESSION:-none}'
for s in sessions:
    if s['id'] != mine:
        print(s['id'][:8])
        break
" 2>/dev/null)
    if [ -n "$OTHER" ]; then
      echo "=== Attaching to session $OTHER (Ctrl+q Ctrl+q to detach) ==="
      snag attach "$OTHER"
    else
      echo "No other session found. Run ./test-attach.sh top first."
    fi
    ;;
  *)
    echo "Usage:"
    echo "  Terminal 1: $0 top    # starts daemon + registers"
    echo "  Terminal 2: $0 bot    # registers + attaches to top"
    ;;
esac
