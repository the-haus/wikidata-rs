#!/bin/sh
# Install the packed @wikidata-rs/node from packages/ (see release-node.yml)
# into an empty project, with the platform package for one triple, and use it
# from ESM and CommonJS. Run from the repository root:
#
#   sh crates/bindings/wikidata-node/scripts/install-check.sh linux-x64-gnu
set -eu
TRIPLE="$1"
rm -rf install-check
mkdir install-check
cd install-check
npm init -y >/dev/null
# Relative paths, which npm on Windows understands too. In a dry run the
# other platform packages are not on the registry; npm skips optional
# dependencies it cannot fetch.
npm install --no-audit --no-fund ../packages/wikidata-rs-node-[0-9]*.tgz "../packages/wikidata-rs-node-$TRIPLE-"*.tgz
ls node_modules/@wikidata-rs
cp ../fixtures/sample.json .
cat > esm.mjs <<'JS'
import { openDump, version } from "@wikidata-rs/node";
const dump = await openDump("sample.json");
const n = (await dump.entities({ filter: { claim: "P17=Q55" } }).toArray()).length;
if (n !== 9) throw new Error(`expected 9 entities in the Netherlands, got ${n}`);
console.log(`esm: ${version()} on ${process.platform}-${process.arch}, node ${process.version}: ${n} entities`);
JS
cat > cjs.cjs <<'JS'
const { streamEntities } = require("@wikidata-rs/node");
(async () => {
  let n = 0;
  for await (const e of streamEntities("sample.json")) n++;
  if (n !== 41) throw new Error(`expected 41 entities, got ${n}`);
  console.log(`cjs: ${n} entities`);
})();
JS
node esm.mjs
node cjs.cjs
