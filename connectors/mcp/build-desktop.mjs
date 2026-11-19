import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { dependencyNotices } from "./notices.mjs";

const dir = (name) => new URL(name, import.meta.url);
await mkdir(dir("./dist/desktop/server/"), { recursive: true });
const result = await build({
  entryPoints: [fileURLToPath(dir("./src/cli.mjs"))],
  outfile: fileURLToPath(dir("./dist/desktop/server/index.js")),
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
  minify: true,
  metafile: true,
  legalComments: "inline",
});
await copyFile(dir("./desktop/manifest.json"), dir("./dist/desktop/manifest.json"));
await copyFile(dir("../../LICENSE"), dir("./dist/desktop/LICENSE"));
const widget = await readFile(dir("./dist/widget-notices.txt"), "utf8");
const notices = await dependencyNotices(result.metafile.inputs);
await writeFile(dir("./dist/desktop/THIRD_PARTY_NOTICES.txt"), notices + "\n\nWidget dependencies\n\n" + widget);
