// Checks the book's internal links and anchors, and that every page is in SUMMARY.md.
// Usage: node scripts/docs/check-links.mjs [docs-dir]
import fs from "node:fs";
import path from "node:path";

const root = path.resolve(process.argv[2] ?? "docs");

const walk = (dir) =>
  fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.isDirectory()) return entry.name === "theme" ? [] : walk(path.join(dir, entry.name));
    return entry.name.endsWith(".md") ? [path.join(dir, entry.name)] : [];
  });

// mdBook's anchor ids: lowercase, keep letters, digits, _ and -, spaces become -.
const slug = (heading) =>
  heading
    .trim()
    .toLowerCase()
    .replace(/`/g, "")
    .replace(/[^\p{L}\p{N}_\- ]/gu, "")
    .replace(/ /g, "-");

const files = walk(root);
const anchors = new Map(
  files.map((file) => [
    file,
    new Set([...fs.readFileSync(file, "utf8").matchAll(/^#{1,6}\s+(.*)$/gm)].map((m) => slug(m[1]))),
  ]),
);

let checked = 0;
const problems = [];

for (const file of files) {
  const text = fs.readFileSync(file, "utf8").replace(/```[\s\S]*?```/g, "");
  for (const match of text.matchAll(/\]\(([^)\s]+)\)/g)) {
    const link = match[1];
    if (/^(https?:|mailto:)/.test(link)) continue;
    checked += 1;
    const [target, anchor] = link.split("#");
    const resolved = target ? path.resolve(path.dirname(file), target) : file;
    const shown = path.relative(root, file);
    if (!fs.existsSync(resolved)) {
      problems.push(`missing file: ${shown} -> ${link}`);
    } else if (anchor && !anchors.get(resolved)?.has(anchor)) {
      problems.push(`missing anchor: ${shown} -> ${link}`);
    }
  }
}

const summary = fs.readFileSync(path.join(root, "SUMMARY.md"), "utf8");
for (const file of files) {
  const rel = path.relative(root, file);
  if (rel !== "SUMMARY.md" && !summary.includes(`(${rel})`)) problems.push(`not in SUMMARY.md: ${rel}`);
}

for (const problem of problems) console.error(problem);
console.log(`${checked} links checked, ${problems.length} problems`);
process.exit(problems.length ? 1 : 0);
