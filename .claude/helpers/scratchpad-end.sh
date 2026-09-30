#!/bin/sh
# SessionEnd hook: remove the ending session's Claude Code temp directory
# (its `scratchpad/` and `tasks/`). This is the backstop for `CLAUDE.md`'s
# rule "The session scratchpad is not a home", not the rule itself: by the
# time a session ends, what a later one needs has already moved.
#
# The hook reads the event's JSON on stdin and deletes only
# `/tmp/claude-<uid>/<project>/<session_id>`, and only when `session_id` is a
# UUID, so a malformed event cannot widen the path.
set -eu

sid=$(jq -r '.session_id // empty')
case "$sid" in
  ????????-????-????-????-????????????) ;;
  *) exit 0 ;;
esac
case "$sid" in
  *[!0-9a-f-]*) exit 0 ;;
esac

project=$(printf '%s' "${CLAUDE_PROJECT_DIR:?}" | sed 's/[^A-Za-z0-9]/-/g')
dir="/tmp/claude-$(id -u)/$project/$sid"
[ -d "$dir" ] && rm -rf -- "$dir"
exit 0
