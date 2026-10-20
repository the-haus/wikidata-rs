// What a dump holds: entities by type, sizes and speed.
//
//   node file-info.mjs DUMP [--allow-truncated]

import { fileInfo } from "@wikidata-rs/node";

const [input, flag] = process.argv.slice(2);
const info = await fileInfo(input, { allowTruncated: flag === "--allow-truncated" });
console.log(JSON.stringify(info, null, 2));
