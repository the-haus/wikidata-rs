// The plain-JavaScript way: decompressed dump lines on stdin
// (`bzip2 -dc dump.json.bz2 | node baseline-filter.mjs MODE`), split
// into lines by hand (faster than readline), then
//   claim:  keep lines containing `"P17":[` and `"id":"Q55"`, JSON.parse
//           them and check the P17 claims (like `--claim P17=Q55`);
//   parse:  JSON.parse every line;
//   count:  count lines.

const mode = process.argv[2] ?? "count";
let carry = "";
let lines = 0;
let matched = 0;
const handle = (line) => {
  if (line.length < 2 || line[0] !== "{") return;
  lines++;
  if (line.endsWith(",")) line = line.slice(0, -1);
  if (mode === "parse") {
    const e = JSON.parse(line);
    if (e.id) matched++;
  } else if (mode === "claim") {
    if (!line.includes('"P17":[') || !line.includes('"id":"Q55"')) return;
    const e = JSON.parse(line);
    const p17 = e.claims?.P17 ?? [];
    if (p17.some((s) => s.rank !== "deprecated" && s.mainsnak?.datavalue?.value?.id === "Q55")) matched++;
  }
};
process.stdin.setEncoding("utf8");
for await (const chunk of process.stdin) {
  const text = carry + chunk;
  let start = 0;
  for (;;) {
    const nl = text.indexOf("\n", start);
    if (nl < 0) break;
    handle(text.slice(start, nl));
    start = nl + 1;
  }
  carry = text.slice(start);
}
handle(carry);
console.log(JSON.stringify({ lines, matched }));
