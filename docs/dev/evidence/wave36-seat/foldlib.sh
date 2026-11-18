# source me: foldone <n>  /  foldcommit <n> <id> "<title>"
W=/root/unaos-bench/scratch/cloud/wave36
foldone() { n=$1; cd /home/user/UnaOS || return 1; bash $W/fold.sh exec-rmbp-$n; for f in $(git diff --name-only --diff-filter=U); do case $f in *.md) python3 /root/unaos-bench/scratch/m3/sheetmerge.py $f exec-rmbp-$n | tail -1;; esac; done; echo "unresolved: $(git diff --name-only --diff-filter=U | tr '\n' ' ')"; for f in $(git diff --name-only --diff-filter=U | grep -v "\.md$"); do echo "== $f"; grep -n "^<<<<<<< \|^=======\|^>>>>>>> " $f; done; }
foldcommit() { n=$1; id=$2; title=$3; cd /home/user/UnaOS || return 1; s=$(git log -1 --format=%h exec-rmbp-$n); sed -i "s|^refs/heads/exec-rmbp-$n .*|refs/heads/exec-rmbp-$n   $s FOLDED-BY-HAND (this commit) 2026-10-27|" docs/dev/exec-branches.txt; if grep -rl "^<<<<<<< " --include=*.rs --include=*.md --include=arroyo --include=*.spec --include=*.toml . | grep -q .; then echo "MARKERS LEFT in $n"; return 1; fi; python3 unaos/scripts/blueprint-check.py 2>&1 | tail -1 | grep -q PASS || { echo "BLUEPRINT RED"; python3 unaos/scripts/blueprint-check.py 2>&1 | grep -v "^BLUEPRINT" | tail -3; return 1; }; git add -A; git commit -q -m "fold $n ($id): $title

exec-rmbp-$n $s folded by hand at the seat. MEASURE: PASS carried in the executor's commit body.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015XT1DePsqSJavM8SkkPbX3"; git log -1 --format="%h $n"; }
