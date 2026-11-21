// Export the Node MCP connector's tools, prompts, guides and Trial Balance widget for
// the Rust MCP server, so both serve the same catalogue.
//
// cd connectors/mcp && npm ci && npm run build:widget
// node ../../scripts/export_mcp_catalog.mjs ../../crates/ot-server/src/mcp
import { writeFile, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const out = resolve(process.argv[2]);
const connector = resolve(process.cwd());
const load = (path) => import(pathToFileURL(join(connector, path)).href);
const { tools } = await load("src/tools.mjs");
const { prompts, guides, widgetUri } = await load("src/workflows.mjs");
const widget = (await load("dist/widget.mjs")).default;

const catalog = {
  widget_uri: widgetUri,
  tools: tools.map((tool) => ({
    name: tool.name,
    title: tool.title,
    description: tool.description,
    input_schema: tool.inputSchema,
    write: Boolean(tool.write),
    destructive: Boolean(tool.destructive),
    idempotent: Boolean(tool.idempotent),
    widget: Boolean(tool.widget),
  })),
  prompts,
  guides: [...guides.entries()].map(([uri, guide]) => ({ uri, ...guide })),
};
await writeFile(join(out, "catalog.json"), JSON.stringify(catalog, null, 1) + "\n");
await writeFile(join(out, "trial-balance-widget.html"), widget);
await writeFile(join(out, "trial-balance-widget-notices.txt"), await readFile(join(connector, "dist/widget-notices.txt"), "utf8"));
console.log(`Exported ${catalog.tools.length} tools, ${prompts.length} prompts and ${catalog.guides.length} guides.`);
