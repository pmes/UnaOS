#!/bin/bash
# arcpatch.sh <arc-sha>: at a fold STOP on a STALE branch — every UU non-doc file := HEAD's, then the arc commit's own diff applied with patch -F3. Prints rejects. Does not git add.
arc=$1; cd /home/user/UnaOS || exit 9
uu=$(git diff --name-only --diff-filter=U | grep -v '^docs/')
for f in $uu; do git show HEAD:$f > $f; done
for f in $(git show --name-only --format= $arc | grep -v '^docs/'); do
  echo "$uu" | grep -qxF "$f" || continue   # only files that conflicted: an auto-merged file already carries the arc's hunks
  git diff $arc^ $arc -- $f | patch -p1 --no-backup-if-mismatch -F3 2>&1 | grep -v '^patching\|succeeded'
done
find . -name '*.rej' -not -path './target/*' | head; echo "UU left: $(git diff --name-only --diff-filter=U | tr '\n' ' ')"; grep -c '^<<<<<<<' $(git diff --name-only --diff-filter=U) 2>/dev/null
