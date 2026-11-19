import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import {
  CallToolRequestSchema, CompleteRequestSchema, ErrorCode, GetPromptRequestSchema,
  ListPromptsRequestSchema, ListResourcesRequestSchema, ListResourceTemplatesRequestSchema,
  ListToolsRequestSchema, McpError, ReadResourceRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";

import widget from "../dist/widget.mjs";
import { tools } from "./tools.mjs";
import { guides, prompts, widgetUri } from "./workflows.mjs";

export const scopes = { read: "opentally.read", write: "opentally.write" };
const mimeType = "text/html;profile=mcp-app";
const widgetMetadata = { ui: { prefersBorder: true, csp: { connectDomains: [], resourceDomains: [] } } };
const widgetResource = {
  uri: widgetUri, name: "OpenTally Trial Balance",
  description: "View accounting and ledger balances in a paginated table.",
  mimeType,
};

function content(text) {
  return [{ type: "text", text }];
}

function errorResult(message, metadata) {
  return { isError: true, content: content(message), ...(metadata && { _meta: metadata }) };
}

function advertise(tool, remote) {
  const security = remote ? [{ type: "oauth2", scopes: [tool.write ? scopes.write : scopes.read] }] : undefined;
  const metadata = {
    ...(security && { securitySchemes: security }),
    ...(tool.widget && {
      ui: { resourceUri: widgetUri, visibility: ["model", "app"] },
      "openai/outputTemplate": widgetUri,
      "openai/widgetAccessible": true,
    }),
  };
  return {
    name: tool.name,
    title: tool.title,
    description: tool.description,
    inputSchema: tool.inputSchema,
    annotations: {
      readOnlyHint: !tool.write,
      destructiveHint: Boolean(tool.destructive),
      idempotentHint: !tool.write || Boolean(tool.idempotent),
      openWorldHint: false,
    },
    ...(security && { securitySchemes: security }),
    ...(Object.keys(metadata).length && { _meta: metadata }),
  };
}

export function createServer({ api, writes, authorize }) {
  const operations = new Map(tools.filter((tool) => writes || !tool.write).map((tool) => [tool.name, tool]));
  const server = new Server(
    { name: "opentally", title: "OpenTally", version: "0.3.0" },
    {
      capabilities: { tools: {}, resources: {}, prompts: {}, completions: {} },
      instructions: "OpenTally reads and manages a local INR book. Use dates in YYYY-MM-DD format. Report amounts ending in _paise are exact integer strings. Use the stored voucher ID and revision where required.",
    },
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [...operations.values()].map((tool) => advertise(tool, Boolean(authorize))),
  }));

  server.setRequestHandler(CallToolRequestSchema, async (request, context) => {
    const tool = operations.get(request.params.name);
    if (!tool) return errorResult("This OpenTally tool is disabled or unavailable.");
    if (authorize) {
      const access = await authorize(tool.write ? scopes.write : scopes.read);
      if (!access.ok) return errorResult(access.message, access.challenge && {
        "mcp/www_authenticate": [access.challenge],
      });
    }
    const args = tool.schema.safeParse(request.params.arguments ?? {});
    if (!args.success) {
      const issues = args.error.issues.slice(0, 8).map((entry) => {
        const field = entry.path.join(".");
        return (field ? field + ". " : "") + entry.message;
      });
      return errorResult("Invalid tool arguments. " + issues.join(" ").slice(0, 1200));
    }
    try {
      const data = await tool.execute(args.data, (path, body) => api(path, body, context.signal));
      const serialized = JSON.stringify(data);
      if (Buffer.byteLength(serialized, "utf8") > 768 * 1024) {
        if (!tool.write) return errorResult("The result is too large. Narrow the dates or request fewer rows.");
        return { content: content("OpenTally accepted the change. Its full result is too large to return. Review the saved records in OpenTally."),
          structuredContent: { saved: true, id: data?.id, code: data?.code } };
      }
      return { content: content(serialized), structuredContent: data };
    } catch (error) {
      return errorResult(error instanceof ApiError ? error.message : "OpenTally could not complete the operation.");
    }
  });

  server.setRequestHandler(ListResourcesRequestSchema, async () => ({
    resources: [...guides.entries()].map(([uri, { text, ...meta }]) => ({ uri, ...meta })).concat(widgetResource),
  }));
  server.setRequestHandler(ListResourceTemplatesRequestSchema, async () => ({ resourceTemplates: [] }));
  server.setRequestHandler(ReadResourceRequestSchema, async ({ params: { uri } }) => {
    if (uri === widgetUri) {
      return { contents: [{ uri, mimeType, text: widget, _meta: widgetMetadata }] };
    }
    const guide = guides.get(uri);
    if (!guide) throw new McpError(-32002, "Resource not found.");
    return { contents: [{ uri, mimeType: guide.mimeType, text: guide.text }] };
  });

  server.setRequestHandler(ListPromptsRequestSchema, async () => ({
    prompts: prompts.map(({ instructions, ...prompt }) => prompt),
  }));
  server.setRequestHandler(GetPromptRequestSchema, async ({ params }) => {
    const prompt = prompts.find((item) => item.name === params.name);
    if (!prompt) throw new McpError(ErrorCode.InvalidParams, "Unknown OpenTally prompt.");
    const args = params.arguments ?? {};
    const values = (prompt.arguments ?? []).map(({ name }) => {
      const value = args[name];
      const format = name === "ledger_id" ? /^[1-9]\d*$/ : /^\d{4}-\d{2}-\d{2}$/;
      if (!value || !format.test(value)) throw new McpError(ErrorCode.InvalidParams, "Supply a valid " + name + ".");
      return name + " = " + value;
    });
    return { description: prompt.description, messages: [{
      role: "user", content: { type: "text", text: [prompt.instructions, ...values].join("\n") },
    }] };
  });
  server.setRequestHandler(CompleteRequestSchema, async ({ params }) => {
    const isDate = ["from", "to"].includes(params.argument.name);
    const hasDate = params.ref.type === "ref/prompt" && prompts.some((item) =>
      item.name === params.ref.name && item.arguments?.some((arg) => arg.name === params.argument.name));
    const values = isDate && hasDate ? completeDate(params.argument.name, params.argument.value) : [];
    return { completion: { values, total: values.length, hasMore: false } };
  });

  return server;
}

function completeDate(field, prefix) {
  const today = new Date();
  const month = (offset) => new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth() + offset, field === "from" ? 1 : 0));
  const offsets = field === "from" ? [0, -1, -2] : [1, 0, -1];
  return offsets.map((offset) => month(offset).toISOString().slice(0, 10)).filter((date) => date.startsWith(prefix));
}

class ApiError extends Error {}

export function createApi(port) {
  const base = "http://127.0.0.1:" + port;
  return async (path, body, signal) => {
    const timeout = AbortSignal.timeout(60_000);
    let response;
    try {
      response = await fetch(base + path, {
        method: body === undefined ? "GET" : "POST",
        headers: body === undefined ? {} : { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: "error",
        signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
      });
    } catch {
      throw new ApiError(body === undefined
        ? "Could not reach the OpenTally app. Check that it is running and retry."
        : "The local OpenTally request did not return a response. Check the books before retrying.");
    }
    const data = await response.json().catch(() => undefined);
    if (!response.ok) {
      const message = typeof data?.error === "string" ? data.error.slice(0, 750) : "Check the supplied data and the state of the books.";
      throw new ApiError("OpenTally returned HTTP " + response.status + ". " + message);
    }
    if (data === undefined) throw new ApiError("OpenTally returned an unreadable result.");
    return data;
  };
}
