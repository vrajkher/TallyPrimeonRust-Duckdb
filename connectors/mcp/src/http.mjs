import { createServer as createHttpServer } from "node:http";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { createApi, createServer } from "./server.mjs";
import { createAuthorizer, protectedResourceMetadata } from "./oauth.mjs";

const maxBody = 8 * 1024 * 1024;

function reply(response, status, data) {
  response.writeHead(status, { "Content-Type": "application/json" });
  response.end(JSON.stringify(data));
}

function rpcError(response, status, id, code, message) {
  reply(response, status, { jsonrpc: "2.0", id: id ?? null, error: { code, message } });
}

async function readRequest(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > maxBody) throw new RangeError("MCP request is too large.");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

export function createHttp(config, { api = createApi(config.apiPort, config.token), authorize = createAuthorizer(config) } = {}) {
  const resource = new URL(config.resource);
  const server = createHttpServer({ requestTimeout: 75_000, headersTimeout: 20_000 }, async (request, response) => {
    const address = server.address();
    const allowedHosts = new Set([resource.host, "127.0.0.1:" + address?.port, "localhost:" + address?.port]);
    if (!allowedHosts.has(request.headers.host)) {
      reply(response, 403, { error: "Unapproved HTTP host." });
      return;
    }
    const origin = request.headers.origin;
    if (origin !== undefined && !config.origins.has(origin)) {
      reply(response, 403, { error: "Unapproved HTTP origin." });
      return;
    }
    response.setHeader("Cache-Control", "no-store");
    if (origin) {
      response.setHeader("Access-Control-Allow-Origin", origin);
      response.setHeader("Vary", "Origin");
      response.setHeader("Access-Control-Expose-Headers", "WWW-Authenticate, MCP-Protocol-Version");
    }

    const path = (request.url ?? "").split("?")[0];
    if (request.method === "GET" && (
      path === "/.well-known/oauth-protected-resource" || path === "/.well-known/oauth-protected-resource/mcp"
    )) {
      reply(response, 200, protectedResourceMetadata(config));
      return;
    }
    if (request.method === "GET" && path === "/health") {
      reply(response, 200, { status: "ok" });
      return;
    }
    if (path !== "/mcp") {
      reply(response, 404, { error: "Resource not found." });
      return;
    }
    if (request.method === "OPTIONS") {
      response.writeHead(204, {
        "Access-Control-Allow-Methods": "POST, GET, DELETE, OPTIONS",
        "Access-Control-Allow-Headers": "Authorization, Content-Type, Accept, MCP-Protocol-Version, MCP-Session-Id, Last-Event-ID",
      });
      response.end();
      return;
    }
    let body;
    if (request.method === "POST") {
      if (!/^application\/json(?:\s*;|$)/i.test(request.headers["content-type"] ?? "")) {
        rpcError(response, 415, null, -32600, "Use application/json.");
        return;
      }
      if (Number(request.headers["content-length"]) > maxBody) {
        rpcError(response, 413, null, -32600, "MCP request is too large.");
        return;
      }
      try {
        body = await readRequest(request);
      } catch (error) {
        const large = error instanceof RangeError;
        rpcError(response, large ? 413 : 400, null, large ? -32600 : -32700,
          large ? "MCP request is too large." : "Invalid JSON.");
        return;
      }
      if (!body || typeof body !== "object" || Array.isArray(body)) {
        rpcError(response, 400, null, -32600, "Send one JSON-RPC object per request.");
        return;
      }
    }
    const mcp = createServer({
      api, writes: config.writes,
      authorize: (scope) => authorize(request.headers.authorization, scope),
    });
    const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined, enableJsonResponse: true });
    response.once("close", () => { void mcp.close().catch(() => {}); });
    try {
      await mcp.connect(transport);
      await transport.handleRequest(request, response, body);
    } catch {
      if (!response.headersSent) rpcError(response, 500, body?.id, -32603, "The MCP request failed.");
      else if (!response.writableEnded) response.end();
    }
  });
  return server;
}
