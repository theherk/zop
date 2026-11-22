// Parses every ```mermaid block in the book with mermaid itself.
// Usage: node scripts/docs/check-diagrams.mjs [docs-dir]
import fs from "node:fs";
import path from "node:path";
import { JSDOM } from "jsdom";

// mermaid expects a browser; parsing needs no layout, so a bare DOM is enough.
const dom = new JSDOM("<!doctype html><html><body></body></html>");
globalThis.window = dom.window;
globalThis.document = dom.window.document;
Object.defineProperty(globalThis, "navigator", { value: dom.window.navigator, configurable: true });

const { default: mermaid } = await import("mermaid");
mermaid.initialize({ startOnLoad: false });

const root = path.resolve(process.argv[2] ?? "docs");

const walk = (dir) =>
  fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (entry.isDirectory()) return entry.name === "theme" ? [] : walk(path.join(dir, entry.name));
    return entry.name.endsWith(".md") ? [path.join(dir, entry.name)] : [];
  });

let diagrams = 0;
let failed = 0;
for (const file of walk(root)) {
  const text = fs.readFileSync(file, "utf8");
  for (const match of text.matchAll(/```mermaid\n([\s\S]*?)```/g)) {
    diagrams += 1;
    try {
      await mermaid.parse(match[1]);
    } catch (error) {
      failed += 1;
      const reason = String(error.message).split("\n").slice(0, 4).join(" | ");
      console.error(`${path.relative(root, file)}, diagram ${diagrams}: ${reason}`);
    }
  }
}

console.log(`${diagrams} diagrams, ${failed} failed`);
process.exit(failed ? 1 : 0);
