#!/bin/sh
# Regenerates tests/golden, tests/js.json and tests/reader from upstream at the commit in UPSTREAM.lock.
# Needs Node 20.19.4, upstream's .tool-versions (fnm exec --using 20.19.4 oracle/run.sh).
set -eu
cd "$(dirname "$0")/.."
[ "$(node --version)" = v20.19.4 ] || { echo "need node v20.19.4, have $(node --version)" >&2; exit 1; }
sha=$(awk '$1 == "CMSgov/hpt-validator" { print $2 }' UPSTREAM.lock)
up=target/upstream
[ -d $up ] || git clone -q https://github.com/CMSgov/hpt-validator.git $up
if [ "$(cat $up/.built 2>/dev/null)" != "$sha" ]; then
  git -C $up checkout -q "$sha"
  (cd $up && npm ci --silent && npm run build --silent)
  echo "$sha" > $up/.built
fi
rm -rf tests/cases tests/golden tests/js.json tests/reader
mkdir -p tests/cases
cp -R $up/test/fixtures tests/cases/upstream
node oracle/cases.mjs $up
node oracle/oracle.mjs $up
node oracle/tables.mjs $up
