import { mkdir, readFile, writeFile } from "node:fs/promises";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
import { dependencyNotices } from "./notices.mjs";

const path = (name) => new URL(name, import.meta.url);
const template = await readFile(path("./src/trial-balance.html"), "utf8");
const result = await build({
  entryPoints: [fileURLToPath(path("./src/trial-balance.mjs"))],
  bundle: true,
  write: false,
  metafile: true,
  format: "esm",
  target: "es2022",
  minify: true,
  legalComments: "inline",
});
const script = result.outputFiles[0].text.replace(/<\/script/gi, "<\\/script");
await mkdir(path("./dist/"), { recursive: true });
const html = template.replace("/* OPENTALLY_WIDGET_SCRIPT */", () => script);
await writeFile(path("./dist/widget.mjs"), "export default " + JSON.stringify(html) + ";\n");
await writeFile(path("./dist/widget-notices.txt"), await dependencyNotices(result.metafile.inputs));
