import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { configFromEnvironment } from "../src/config.mjs";
import { createHttp } from "../src/http.mjs";
import { createAuthorizer } from "../src/oauth.mjs";
import { createServer } from "../src/server.mjs";
import { tools } from "../src/tools.mjs";
import { widgetUri } from "../src/workflows.mjs";

const periods = { from: "2025-09-01", to: "2025-09-30" };
const testClient = () => new Client({ name: "OpenTally test client", version: "1.0.0" });
const call = (client, name, args = {}) => client.callTool({ name, arguments: args });

test("MCP advertises tools, resources, prompts and completions with writes off by default", async (t) => {
  let calls = 0;
  const server = createServer({ writes: false, api: async () => { calls++; return { company_name: "Test book" }; } });
  const [left, right] = InMemoryTransport.createLinkedPair();
  const client = testClient();
  await server.connect(left);
  await client.connect(right);
  t.after(() => Promise.all([client.close(), server.close()]));
  const capabilities = client.getServerCapabilities();
  for (const capability of ["tools", "resources", "prompts", "completions"]) assert.ok(capabilities[capability]);
  const listed = (await client.listTools()).tools;
  assert.equal(listed.length, 13);
  assert.ok(listed.every((tool) => tool.annotations.readOnlyHint === true));
  assert.equal(listed.find((tool) => tool.name === "trial_balance")._meta.ui.resourceUri, widgetUri);
  assert.deepEqual((await call(client, "get_company")).structuredContent, { company_name: "Test book" });
  assert.equal((await call(client, "close_period", { through: "2025-09-30", actor: "Test", reason: "Test" })).isError, true);
  assert.equal((await call(client, "trial_balance", { from: "invalid", to: "2025-09-30" })).isError, true);
  assert.equal(calls, 1);
  assert.equal((await client.listResources()).resources.length, 3);
  const guide = await client.readResource({ uri: "opentally://guide/accounting" });
  assert.match(guide.contents[0].text, /INR company/);
  const widget = await client.readResource({ uri: widgetUri });
  assert.equal(widget.contents[0].mimeType, "text/html;profile=mcp-app");
  assert.match(widget.contents[0].text, /OpenTally Trial Balance/);
  assert.equal((await client.listPrompts()).prompts.length, 3);
  const prompt = await client.getPrompt({ name: "review_period", arguments: periods });
  assert.match(prompt.messages[0].content.text, /from = 2025-09-01/);
  const completion = await client.complete({ ref: { type: "ref/prompt", name: "review_period" }, argument: { name: "from", value: "" } });
  assert.match(completion.completion.values[0], /^\d{4}-\d{2}-01$/);
});

const oauthEnv = {
  OPENTALLY_MCP_RESOURCE_URL: "https://books.example.test/mcp",
  OPENTALLY_MCP_ISSUER: "https://login.example.test/",
  OPENTALLY_MCP_INTROSPECTION_URL: "https://login.example.test/introspect",
  OPENTALLY_MCP_CLIENT_ID: "introspection-client",
  OPENTALLY_MCP_CLIENT_SECRET: "fictional-test-only-secret",
  OPENTALLY_MCP_WRITES: "true",
};

test("HTTP configuration requires HTTPS resource discovery and scoped OAuth", () => {
  assert.deepEqual(configFromEnvironment({}, "stdio"), { apiPort: 3000, writes: false });
  assert.throws(() => configFromEnvironment({}, "http"), /OPENTALLY_MCP_RESOURCE_URL/);
  assert.throws(() => configFromEnvironment({ ...oauthEnv, OPENTALLY_MCP_RESOURCE_URL: "http://example.test/mcp" }, "http"), /HTTPS/);
});

test("Streamable HTTP advertises both scopes and checks audience, origin and permission before book access", async (t) => {
  const config = configFromEnvironment(oauthEnv, "http");
  const effects = [];
  const authorize = createAuthorizer(config, async (url, request) => {
    assert.equal(url, config.introspection);
    assert.equal(request.redirect, "error");
    assert.match(request.headers.Authorization, /^Basic /);
    const token = request.body.get("token");
    return Response.json({
      active: token !== "inactive", iss: config.issuer,
      exp: token === "expired" ? 1 : Math.floor(Date.now() / 1000) + 60,
      aud: [token === "other" ? "https://other.example.test/mcp" : config.resource],
      scope: token === "all" ? "opentally.read opentally.write" : "opentally.read",
    });
  });
  const server = createHttp(config, {
    authorize,
    api: async (path, body) => { effects.push({ path, body }); return { company_name: body?.name ?? "Test book" }; },
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const base = "http://127.0.0.1:" + server.address().port;
  const clients = [];
  t.after(async () => {
    await Promise.all(clients.map((client) => client.close()));
    await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  });
  async function connect(token) {
    const client = testClient();
    const headers = token ? { Authorization: "Bearer " + token } : {};
    await client.connect(new StreamableHTTPClientTransport(new URL(base + "/mcp"), { requestInit: { headers } }));
    clients.push(client);
    return client;
  }
  const metadata = await (await fetch(base + "/.well-known/oauth-protected-resource/mcp")).json();
  assert.equal(metadata.resource, config.resource);
  assert.deepEqual(metadata.scopes_supported, ["opentally.read", "opentally.write"]);
  assert.equal((await fetch(base + "/health", { headers: { Origin: "https://untrusted.example" } })).status, 403);

  const guest = await connect();
  const list = (await guest.listTools()).tools;
  assert.equal(list.length, 25);
  assert.deepEqual(list.find((tool) => tool.name === "post_voucher")._meta.securitySchemes, [
    { type: "oauth2", scopes: ["opentally.write"] },
  ]);
  assert.equal(list.find((tool) => tool.name === "close_period").annotations.destructiveHint, true);
  assert.match((await call(guest, "get_company"))._meta["mcp/www_authenticate"][0], /oauth-protected-resource\/mcp/);
  for (const token of ["other", "expired", "inactive"]) assert.equal((await call(await connect(token), "get_company")).isError, true);
  const reader = await connect("reader");
  const rejected = await call(reader, "set_company_name", { name: "Denied" });
  assert.equal(rejected.isError, true);
  assert.match(rejected._meta["mcp/www_authenticate"][0], /insufficient_scope/);
  assert.equal(effects.length, 0);
  assert.equal((await call(reader, "get_company")).structuredContent.company_name, "Test book");
  const writer = await connect("all");
  assert.equal((await call(writer, "set_company_name", { name: "Allowed" })).structuredContent.company_name, "Allowed");
  assert.deepEqual(effects, [{ path: "/api/settings", body: undefined }, { path: "/api/settings", body: { name: "Allowed" } }]);
});

const liveBinary = process.env.OPENTALLY_TEST_BIN;

test("installed stdio connector reads and writes a temporary Rust/DuckDB book", { skip: !liveBinary }, async (t) => {
  const folder = await mkdtemp(join(tmpdir(), "opentally-mcp-test-"));
  const app = spawn(liveBinary, [], {
    env: { ...process.env, OPENTALLY_PORT: "0", OPENTALLY_DB: join(folder, "test.duckdb") },
    stdio: ["ignore", "pipe", "inherit"],
  });
  t.after(async () => {
    if (app.exitCode === null) {
      const exit = once(app, "exit");
      app.kill("SIGINT");
      await exit;
    }
    await rm(folder, { recursive: true, force: true });
  });
  const apiPort = await new Promise((resolve, reject) => {
    let stdout = "";
    const timer = setTimeout(() => reject(new Error("OpenTally did not start")), 15000);
    app.stdout.on("data", (chunk) => {
      stdout += chunk;
      const result = stdout.match(/http:\/\/127\.0\.0\.1:(\d+)/);
      if (result) { clearTimeout(timer); resolve(result[1]); }
    });
    app.once("error", (error) => { clearTimeout(timer); reject(error); });
    app.once("exit", (code) => { clearTimeout(timer); reject(new Error("OpenTally exited: " + code)); });
  });
  const executable = process.env.OPENTALLY_TEST_MCP ?? fileURLToPath(new URL("../src/cli.mjs", import.meta.url));
  const transport = new StdioClientTransport({
    command: process.execPath, args: [executable, "stdio"],
    env: { OPENTALLY_PORT: apiPort, OPENTALLY_MCP_WRITES: "true" },
  });
  const client = testClient();
  await client.connect(transport);
  t.after(() => client.close());
  assert.equal((await client.listTools()).tools.length, tools.length);

  const exercised = new Set();
  async function run(name, args) {
    const result = await call(client, name, args);
    assert.equal(result.isError, undefined, JSON.stringify(result));
    exercised.add(name);
    return result.structuredContent;
  }
  assert.equal((await run("get_company")).company_name, "Local book");
  await run("set_company_name", { name: "MCP fixture" });
  const group = await run("create_group", { name: "Test expenses" });
  assert.equal((await run("list_groups")).rows[0].id, group.id);
  const ledger = await run("create_ledger", { name: "Test rent", kind: "general", group_id: group.id });
  assert.ok((await run("list_ledgers")).rows.some((row) => row.id === ledger.id && row.group_id === group.id));
  const posted = { voucher: {
    kind: "payment", request_id: "212a5c77-b931-4d4f-8cf5-f50fcb07e3a5", date: "2025-09-03",
    amount: "150.25", cash_ledger_id: 2, other_ledger_id: ledger.id,
  } };
  const voucher = await run("post_voucher", posted);
  assert.equal((await run("post_voucher", posted)).id, voucher.id);
  const item = await run("get_voucher", { voucher_id: voucher.id });
  assert.deepEqual(item.postings.map((line) => [line.side, line.amount_paise]), [["debit", "15025"], ["credit", "15025"]]);
  const trial = await run("trial_balance", { ...periods, limit: 1 });
  assert.equal(trial.rows.length, 1);
  assert.equal(trial.debit_paise, "15025");
  assert.equal(trial.credit_paise, "15025");
  assert.equal(trial.closing_debit_paise, null);
  assert.equal(trial.pagination.rows.next_offset, 1);
  assert.deepEqual(trial.period, periods);
  assert.equal((await run("list_vouchers", { ...periods, kind: "payment", q: voucher.code })).count, 1);
  assert.equal((await run("ledger_statement", { ...periods, ledger_id: ledger.id })).rows[0].amount_paise, "15025");
  await run("record_opening", { ledger_id: ledger.id, date: periods.from, amount: "0.00", source: "Test ledger report" });
  await run("import_bank_statement", { ledger_id: 2, content: "transaction_id,date,amount\nbank-test,2025-09-03,-150.25\n" });
  const review = await run("bank_reconciliation", { ...periods, ledger_id: 2 });
  const matched = await run("match_bank_rows", {
    actor: "Test operator", ledger_id: 2, statement_ids: [review.statement[0].id],
    postings: review.book.map(({ voucher_id, revision, line_no }) => ({ voucher_id, revision, line_no })),
  });
  await run("undo_bank_match", { match_id: matched.match_id, actor: "Test operator", reason: "Fixture check" });
  const edit = await run("edit_voucher", {
    voucher_id: voucher.id, expected_revision: voucher.revision, actor: "Test operator", reason: "Fixture edit",
    voucher: { ...posted.voucher, amount: "125.25", request_id: "c29c22ca-10f0-4ed6-bc31-9ad567a50b2b" },
  });
  const original = await run("get_voucher_revision", { voucher_id: voucher.id, revision: voucher.revision });
  assert.equal(original.postings[0].amount_paise, "15025");
  await run("cancel_voucher", { voucher_id: voucher.id, expected_revision: edit.revision, actor: "Test operator", reason: "Fixture check" });
  const history = await run("list_voucher_revisions", { voucher_id: voucher.id });
  assert.equal(history.rows[0].action, "cancelled");
  assert.equal((await run("trial_balance", periods)).debit_paise, "0");

  const source = {
    format: "json", content: JSON.stringify({ company: "MCP fixture", currency: "INR", vouchers: [{
      guid: "mcp-test-import", code: "JRN-EXAMPLE", type_name: "Journal", date: "2025-09-04", postings: [
        { ledger: "Test rent", side: "debit", amount: "4.25", allocations: [{ category: "Region", centre: "North", amount: "4.25" }] },
        { ledger: "Bank", side: "credit", amount: "4.25" },
      ],
    }] }),
  };
  assert.equal((await run("preview_import", source)).committed, false);
  assert.equal((await run("commit_import", source)).inserted, 1);
  assert.equal((await run("cost_center_totals", periods)).rows[0].debit_paise, "425");
  assert.equal((await run("cost_center_postings", { ...periods, category: "Region", centre: "North" })).rows[0].code, "JRN-EXAMPLE");
  assert.equal((await run("close_period", { through: periods.to, actor: "Test operator", reason: "Month end" })).closed_through, periods.to);
  assert.equal((await call(client, "post_voucher", { voucher: { ...posted.voucher, request_id: "bb8af5c9-69fe-4baf-8771-91ec45c94bb0" } })).isError, true);
  assert.deepEqual([...exercised].sort(), tools.map((tool) => tool.name).sort());
});
