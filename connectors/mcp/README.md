# OpenTally MCP

One connector serves the OpenTally accounting API for ChatGPT and Claude. The connector uses Node.js 20.3 or newer and the official MCP SDK. The accounting app continues to run with Rust and DuckDB.

## Tools and access

| Capability | Operations |
| --- | --- |
| Read the book | Company settings, groups, ledgers, Day Book, voucher details and revision history |
| Read reports | Trial Balance, ledger statements, cost-centre totals and postings, and bank statement and book rows |
| Preview import | Validate Tally XML or OpenTally JSON and report totals, warnings and duplicates |
| Write to the book | Create groups and ledgers, post or edit and cancel vouchers, commit imports, record opening balances, set the company name, and close periods |
| Reconcile the bank | Import CSV, record grouped matches and undo matches |

MCP also exposes accounting guides as resources, three review prompts, argument completion for their date fields, and a Trial Balance widget with page controls on hosts that support MCP Apps. The connector publishes 13 read tools and another 12 write tools when writes are enabled. The write annotations describe which operations change or remove accounting effects.

Start OpenTally from the repository root with `cargo run --locked`. It uses port 3000 by default. The connector sends its fixed accounting requests to `127.0.0.1` on that port. Set `OPENTALLY_PORT` on the connector if the app uses another port.

## Claude Desktop

Install **OpenTally-Claude-Desktop.mcpb** in Claude Desktop using **Settings → Desktop app → Extensions → Advanced settings → Install Extension**. Configure the OpenTally port. Turn on **Allow changes to the book** to enable write tools. Use the connection in a Claude surface that supports local desktop extensions. [Claude Desktop extensions](https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop) · [Local and remote connectors](https://support.claude.com/en/articles/11725091-when-to-use-desktop-and-web-connectors)

To build the extension from source:

```sh
cd connectors/mcp
npm ci
npm run bundle:desktop
```

The result is `dist/OpenTally-Claude-Desktop.mcpb`. It includes the Node connector. Start the separate OpenTally accounting app before using its tools.

## Claude Code plugin

Install the connector command so that `opentally-mcp` is available on Claude Code’s PATH. Build the package from the repository if you are starting from source.

```sh
cd connectors/mcp
npm ci
npm pack
npm install -g ./opentally-mcp-0.3.0.tgz
```

From the repository root, start Claude Code with the plugin folder.

```sh
claude --plugin-dir ./plugins/claude
```

The repository also contains a marketplace manifest. In Claude Code, run `/plugin marketplace add /absolute/path/to/OpenTally` followed by `/plugin install opentally@opentally-local`. The plugin includes the `/opentally:month-end` skill. Set `OPENTALLY_MCP_WRITES=1` in the environment that starts Claude Code if you want write tools. [Claude Code plugin reference](https://code.claude.com/docs/en/plugins-reference)

For manual local setup in any host that accepts a standard MCP server configuration:

```json
{
  "mcpServers": {
    "opentally": {
      "command": "opentally-mcp",
      "args": ["stdio"],
      "env": {
        "OPENTALLY_PORT": "3000",
        "OPENTALLY_MCP_WRITES": "0",
        "OPENTALLY_API_TOKEN": "ot_..."
      }
    }
  }
}
```

If the host uses a different PATH, give the absolute path to the installed command.

### Books with users

A book without users accepts the connector from the same computer with no token. Once the book has users, set `OPENTALLY_API_TOKEN` (or **OpenTally API token** in Claude Desktop) to a token created in OpenTally. The token acts for the user who created it, and its scope limits it further. A `read` token can use only the read tools. A `draft` token can also create and edit drafts. A `submit` token can also post vouchers, import, close periods and reconcile. Leave out `OPENTALLY_API_TOKEN` for a book without users.

## ChatGPT and Claude web

Start the MCP HTTP connector next to OpenTally and expose the connector through HTTPS. Use the same endpoint for a [ChatGPT custom plugin](../../plugins/chatgpt/README.md) and a [Claude custom web connector](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp). The OpenTally accounting interface remains on its loopback port.

The connector validates OAuth access tokens using the identity provider’s RFC 7662 introspection endpoint. Use a provider that returns `active`, `aud`, and a space-separated `scope`. The audience must include the public MCP resource URL exactly as configured. Give users `opentally.read` for reading and grant `opentally.write` to authorized operators. Enable writes on the connector to advertise the write scope and tools. Use one connector instance per OpenTally book and grant its scopes to users authorized for that book.

| Variable | Meaning |
| --- | --- |
| `OPENTALLY_PORT` | Local Rust app port. Default 3000. |
| `OPENTALLY_API_TOKEN` | OpenTally API token, needed once the book has users. |
| `OPENTALLY_MCP_PORT` | Local MCP listening port. Default 8787. |
| `OPENTALLY_MCP_WRITES` | Set to `1` or `true` to enable write tools. |
| `OPENTALLY_MCP_RESOURCE_URL` | Public HTTPS resource ending in `/mcp`. |
| `OPENTALLY_MCP_ISSUER` | Exact issuer URL advertised by the authorization server. |
| `OPENTALLY_MCP_INTROSPECTION_URL` | HTTPS token introspection endpoint. |
| `OPENTALLY_MCP_CLIENT_ID` | Client ID used by the connector to authenticate introspection requests. |
| `OPENTALLY_MCP_CLIENT_SECRET` | Corresponding client secret. Keep it in the server environment. |
| `OPENTALLY_MCP_ALLOWED_ORIGINS` | Comma-separated approved HTTPS browser origins. Defaults include `chatgpt.com` and `claude.ai`. |

Build and start the connector after configuring those variables.

```sh
cd connectors/mcp
npm ci
npm run build:widget
node src/cli.mjs http
```

Forward the public `/mcp`, `/.well-known/oauth-protected-resource`, and `/.well-known/oauth-protected-resource/mcp` routes to the local connector. Preserve `Authorization`, the request method, and the body. The endpoint publishes its authorization server location and per-tool scopes. The identity provider handles user sign-in and the client registration method used by each host. Its configured token audience must match the resource URL.

In Claude, open **Customize → Connectors → Add → Custom → Web** and enter the public `/mcp` URL. Choose **Sign in now**, then select the OAuth client registration option that your provider supports. [Claude connector setup](https://support.claude.com/en/articles/11176164-use-connectors-to-extend-claude-s-capabilities)

## Verification

Run `npm test` from this directory. With the compiled Rust binary available, run the same tests against a new temporary DuckDB book.

```sh
OPENTALLY_TEST_BIN=/absolute/path/to/opentally npm test
```

Tests exercise the core protocol, tool scopes, OAuth challenge, read-only mode, and the OpenTally API. Run `npm run bundle:desktop` to validate and pack the Claude Desktop manifest.
