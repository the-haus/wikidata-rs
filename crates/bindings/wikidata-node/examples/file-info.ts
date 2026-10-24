// What a dump holds: entities by type, sizes and speed.
//
//   npx tsx file-info.ts DUMP [--allow-truncated]

import { fileInfo } from "@wikidata-rs/node";

const [input, flag] = process.argv.slice(2);
if (!input) throw new Error("usage: npx tsx file-info.ts DUMP [--allow-truncated]");
const info = await fileInfo(input, { allowTruncated: flag === "--allow-truncated" });
console.log(JSON.stringify(info, null, 2));
