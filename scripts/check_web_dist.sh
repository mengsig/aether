#!/bin/sh
# Fails when the committed web/dist/ differs from the build just written over it.
# Run it right after a fresh build: `make web-dist-check`, or the web workflow.
set -eu
cd "$(dirname "$0")/.."

[ -f web/dist/index.html ] || { echo "check_web_dist: web/dist/index.html is missing; build first" >&2; exit 2; }
git ls-files --error-unmatch web/dist/index.html >/dev/null 2>&1 \
  || { echo "check_web_dist: web/dist/ is not committed in this checkout" >&2; exit 2; }

# Modified, deleted and new files alike: a build that drops or adds one is drift too.
changes=$(git status --porcelain --untracked-files=all -- web/dist)
if [ -n "$changes" ]; then
  printf 'check_web_dist: the committed web/dist/ differs from a fresh build:\n%s\n' "$changes" >&2
  echo "Rebuild it with 'make web' and commit web/dist/." >&2
  exit 1
fi
printf 'web/dist/ matches a fresh build (%s files)\n' "$(git ls-files -- web/dist | wc -l | tr -d ' ')"
