#!/bin/bash
# Pack @wikidata-rs/node, install the tarball into a fresh project, and use
# it from ESM, CommonJS and TypeScript.
set -euo pipefail
PKG=$(cd "$(dirname "$0")/.." && pwd)
F="$PKG/../../../fixtures/sample.json"
W=$(mktemp -d)
V=$(node -p "require('$PKG/package.json').version")
cd "$PKG"
rm -f "wikidata-rs-node-$V.tgz"
npm pack --ignore-scripts >/dev/null
mv "wikidata-rs-node-$V.tgz" "$W/"
cd "$W"
npm init -y >/dev/null
npm install "./wikidata-rs-node-$V.tgz" typescript@5 @types/node@22 >/dev/null 2>&1
node -e 'require.resolve("@wikidata-rs/node/package.json")' 
ls node_modules/@wikidata-rs/node
cat > esm.mjs <<'JS'
import { openDump, version } from "@wikidata-rs/node";
const dump = await openDump(process.argv[2]);
const es = await dump.entities({ filter: { claim: "P17=Q55" } }).toArray();
if (es.length !== 9) throw new Error(`expected 9, got ${es.length}`);
console.log("esm", version(), es.length, es[0].label?.text);
JS
cat > cjs.cjs <<'JS'
const { streamEntities } = require("@wikidata-rs/node");
(async () => {
  let n = 0;
  for await (const e of streamEntities(process.argv[2])) n++;
  if (n !== 41) throw new Error(`expected 41, got ${n}`);
  console.log("cjs", n);
})();
JS
cat > app.ts <<'TS'
import { openDump, type EntityRecord } from "@wikidata-rs/node";
async function main(path: string): Promise<number> {
  const dump = await openDump(path);
  let n = 0;
  for await (const e of dump.entities({ languages: ["nl", "en", "mul"] })) {
    const typed: EntityRecord = e;
    if (typed.instanceOf.length > 0) n++;
  }
  return n;
}
main(process.argv[2]!).then((n) => console.log("ts", n));
TS
echo '{"compilerOptions":{"module":"NodeNext","moduleResolution":"NodeNext","target":"ES2022","strict":true,"outDir":"out","rootDir":".","types":["node"]}}' > tsconfig.json
node esm.mjs "$F"
node cjs.cjs "$F"
npx tsc -p .
node out/app.js "$F"
rm -rf "$W"
