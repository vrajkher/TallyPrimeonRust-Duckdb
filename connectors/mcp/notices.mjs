import { readFile, readdir } from "node:fs/promises";
import { join, resolve } from "node:path";

export async function dependencyNotices(inputs) {
  const names = new Set();
  const notices = [];
  for (const input of Object.keys(inputs)) {
    const matches = /(?:^|[/\\])node_modules[/\\]((?:@[^/\\]+[/\\])?[^/\\]+)/g;
    let match;
    let last;
    while ((match = matches.exec(input))) last = match;
    if (!last) continue;
    const root = resolve(input.slice(0, last.index + last[0].length));
    if (names.has(root)) continue;
    names.add(root);
    const packages = JSON.parse(await readFile(join(root, "package.json"), "utf8"));
    const files = await readdir(root, { withFileTypes: true });
    const licenses = files.filter((file) => file.isFile() && /^(?:license|licence|copying|notice)(?:[.-].*)?$/i.test(file.name));
    const texts = await Promise.all(licenses.map((file) => readFile(join(root, file.name), "utf8")));
    notices.push(packages.name + " " + packages.version + "\n" + (texts.join("\n") || (packages.license ?? "See package metadata.")));
  }
  return notices.sort((a, b) => a.localeCompare(b)).join("\n\n");
}
