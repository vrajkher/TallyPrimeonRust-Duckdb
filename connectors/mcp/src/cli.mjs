#!/usr/bin/env node
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { configFromEnvironment } from "./config.mjs";
import { createHttp } from "./http.mjs";
import { createApi, createServer } from "./server.mjs";

async function start() {
  const args = process.argv.slice(2);
  const mode = args[0] ?? "stdio";
  if (mode === "--help" && args.length === 1) {
    process.stdout.write("Usage: opentally-mcp [stdio|http]\n");
    return;
  }
  if (args.length > 1 || !["stdio", "http"].includes(mode)) throw new Error("Usage: opentally-mcp [stdio|http]");
  const config = configFromEnvironment(process.env, mode);
  if (mode === "stdio") {
    const server = createServer({ api: createApi(config.apiPort, config.token), writes: config.writes });
    await server.connect(new StdioServerTransport());
    process.stdin.once("end", () => { void server.close(); });
    return;
  }
  const server = createHttp(config);
  server.on("error", (error) => {
    process.stderr.write("OpenTally MCP could not listen. " + (error.code ?? "") + "\n");
    process.exitCode = 1;
  });
  server.listen(config.listenPort, "127.0.0.1", () => {
    process.stderr.write("OpenTally MCP listening at http://127.0.0.1:" + config.listenPort + "/mcp\n");
  });
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.once(signal, () => server.close());
  }
}

start().catch((error) => {
  process.stderr.write(error.message + "\n");
  process.exitCode = 1;
});
