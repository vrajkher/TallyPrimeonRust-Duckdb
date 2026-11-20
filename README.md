# OpenTally

OpenTally is a local accounting app built with Rust and DuckDB. It runs in your browser and stores one INR company per DuckDB file. You can use it on its own or connect ChatGPT and Claude to the same books through MCP.

## Features

- **Vouchers** for payments, receipts, journals and transfers between cash and bank accounts. Use multiple debit and credit lines, add references and narration, and review or print saved entries from the Day Book.
- **Account setup and reports** with groups, ledgers, dated opening balances, Trial Balance, ledger statements and cost-centre reports.
- **Tally import** with a preview of Tally XML or OpenTally JSON, source voucher references, duplicate detection and revision handling.
- **Bank reconciliation** with CSV statement import, grouped matching and a recorded reason when a match is undone.
- **Book controls** with voucher revision history and period closing. Editing or cancelling a local voucher records the operator and reason.
- **ChatGPT and Claude** integration with 25 MCP tools, including reports, import previews, voucher posting and bank reconciliation. Write tools are off by default.

## Install and run

1. Install Git and [Rust 1.90 or newer](https://www.rust-lang.org/tools/install).
2. Install a C++ build toolchain. On Windows, use Visual Studio Build Tools with **Desktop development with C++**. On macOS, run `xcode-select --install`. On Debian or Ubuntu, install `build-essential`.
3. Clone the repository and start OpenTally.

```sh
git clone https://github.com/vrajkher/TallyPrimeonRust-Duckdb.git
cd TallyPrimeonRust-Duckdb
cargo run --locked
```

The first build compiles the bundled DuckDB library. When the app is running, open [http://127.0.0.1:3000](http://127.0.0.1:3000). Keep the terminal open while you work. Press **Ctrl+C** to stop the app.

### The new interface

OpenTally is moving to a new interface written in Rust with [Leptos](https://leptos.dev) and compiled to WebAssembly. It runs at [http://127.0.0.1:3000/app/](http://127.0.0.1:3000/app/) next to the classic screens, and screens move to it one at a time. It already has sign-in, creating the first user, the company setup wizard, the Day Book with date, type and text filters, the Chart of Accounts, and Number Series.

The first time a System Manager opens it, the setup wizard asks for the company name, country, currency, chart of accounts, fiscal year and bank. Choosing a country fills in its currency, chart and fiscal year: India gets Tally's reserved groups and an April–March year. The wizard runs once.

**Number Series** sets how documents are named, e.g. `SINV-1001`. Each document type has a default series; add your own prefix, start number and digits, or make another series the default. A series that has named a document cannot be deleted or restarted below its last number. The same is available from the command line as `opentally series list|add|set|delete`.

Build it once, and again after changing it:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd crates/ot-web
trunk build --release
```

Then start OpenTally from the repository root as usual. Set `OPENTALLY_UI_DIR` to serve a built copy from somewhere else.

| Key | Action |
| --- | --- |
| F4, F5, F6, F7 | Contra, Payment, Receipt or Journal entry (on the classic screens for now) |
| Alt+D | Day Book |
| Alt+A | Chart of Accounts |
| Alt+N | Number Series |
| / | Search the list |
| Page Up, Page Down | Previous or next page |
| Esc | Close a message |

### Choose a book file, bind address or port

| Setting | Default |
| --- | --- |
| `OPENTALLY_DB` | `data/vouchers.duckdb`, relative to the starting directory |
| `OPENTALLY_BIND_ADDR` | `127.0.0.1`. Accepts an IPv4 or IPv6 address without a port. |
| `OPENTALLY_PORT` | `3000` |
| `OPENTALLY_UI_DIR` | `crates/ot-web/dist`, relative to the starting directory. Where the built new interface is served from. |
| `OPENTALLY_SECURE_COOKIES` | Off. Set to `1` when users reach OpenTally over HTTPS, so sign-in cookies are sent only over HTTPS. |

For Docker, set `OPENTALLY_BIND_ADDR=0.0.0.0` inside the container so Kiri can reach OpenTally on the container network. Create the first user before you do this: until a book has users, OpenTally answers only requests from the same computer.

When a book needs a schema upgrade, OpenTally first copies it to `<book>.bak-v<version>` next to the original, then upgrades it. A book upgraded by a newer OpenTally is refused rather than changed.

The previous `VOUCHER_DB` and `VOUCHER_PORT` settings are accepted. The default path also matches Voucher Desk. When OpenTally first opens an existing file, it migrates vouchers to ordered posting lines while preserving their IDs and numbers. Use OpenTally to post new entries after migrating a book.

Run this command on macOS or Linux to practice with a separate book.

```sh
OPENTALLY_DB=data/practice.duckdb cargo run --locked
```

Use these commands in PowerShell.

```powershell
$env:OPENTALLY_DB = "data/practice.duckdb"
cargo run --locked
```

## Users and sign-in

A new book has no users. Until you create one, OpenTally works as before for requests from the same computer and refuses requests from anywhere else, including requests relayed by a proxy.

Create the first user from the computer that runs OpenTally. That user is a System Manager and is signed in straight away.

```sh
curl -X POST http://127.0.0.1:3000/api/v1/setup -H 'Content-Type: application/json' \
  -d '{"username": "asha", "full_name": "Asha Patel", "password": "use a long passphrase"}'
```

From then on, the web interface asks everyone to sign in, and every API request needs a sign-in session or an API token. Passwords need at least 10 characters. Sign-ins expire after 12 hours, and five wrong passwords for a user name block sign-in for 15 minutes.

Roles follow Frappe Books.

| Role | Can do |
| --- | --- |
| `system_manager` | Everything, including users and tokens |
| `books_manager` | All accounting: accounts, journals, payments, imports, reports, bank reconciliation, settings and period closing |
| `books_user` | Payments and receipts. No journals, accounts or reports |
| `books_cashier` | Point of sale (in a later release). Reads payments |
| `viewer` | Reads and reports everything a Books Manager can, and changes nothing |

A System Manager adds users with `POST /api/v1/users` and changes roles, names, passwords or access with `POST /api/v1/users/{id}`. OpenTally always keeps at least one enabled System Manager. Changing someone's password or roles signs them out everywhere.

API tokens let the ChatGPT and Claude connector, scripts and other tools act for a user. Create one while signed in with `POST /api/v1/tokens` and a body such as `{"name": "Claude", "scope": "read"}`. The response shows the secret, which starts with `ot_`, once. Send it as `Authorization: Bearer ot_...`. A `read` token reads only, `draft` can also create drafts, `submit` can also post, import and reconcile, and `admin` can also manage users. A token never has more rights than its user. Revoke a token with `DELETE /api/v1/tokens/{id}`.

Every API route is available under both `/api` and `/api/v1`. New clients should use `/api/v1`.

## Chart of accounts

Groups are group accounts and ledgers are ledger accounts, as in Tally. Each group has a root type (Asset, Liability, Equity, Income or Expense), and each ledger can have an account type such as Bank, Receivable, Tax or Round Off, as in Frappe Books. Groups with Tally's reserved names, such as Sundry Debtors, are classified automatically; others take their parent's root type, or one you set on the top-level group with `POST /api/v1/groups/{id}/root-type`. A root type or account type cannot be changed once set.

To start from a template, list them with `opentally chart list` (or `GET /api/v1/accounts/charts`) and apply one with `opentally chart apply "Tally (India) - Reserved Groups"` (or `POST /api/v1/accounts/chart`). The templates are Tally's 28 reserved groups, Frappe Books' standard chart, and its charts for India and 11 other countries. Applying a template keeps existing groups and ledgers, places ungrouped Cash and Bank ledgers in the right groups, and fills the default round-off, write-off, discount, receivable and payable accounts in `GET /api/v1/settings/accounting`. It never changes postings or balances.

## Record your first voucher

1. Open **Company & import**, enter your company name, and select **Save company**.
2. Open **Ledgers**, enter `Office Expenses`, select **Party / income / expense**, and click **Create ledger**. A new book already has **Cash** and **Bank** ledgers.
3. Open **Voucher entry → Payment**. Enter a date and `250.00`, choose **Cash** under **Paid from** and **Office Expenses** under **Paid to / expense**, then select **Save payment**. This debits Office Expenses and credits Cash by ₹250.
4. Open **Day Book** to find the payment. Select its voucher number to see the details or print it. In **Reports**, choose **Ledger statement** or **Trial Balance** and use a date range that includes the payment.

### Other day-to-day tasks

- Open **Company & import → Account groups and opening balances** to create groups and record openings with an effective date and source. Enter an opening debit as a positive amount and an opening credit as a negative amount. Use `0.00` for an explicitly confirmed zero.
- Use **Payment** (F5) for money paid, **Receipt** (F6) for money received, **Journal** (F7) for manual debit and credit entries, or **Contra** for a transfer between distinct cash or bank ledgers. Select **Enter multiple posting lines** for a compound payment or receipt. Add line narration and cost allocations through each line’s details. Review the debit and credit totals, then save with **Ctrl+Enter**.
- Open **Day Book** to search by type, status, dates, voucher number, or ledger. Open the voucher number to print or inspect its history. Edit or cancel an open-period local voucher using an operator name and reason. In a book with users, OpenTally records the signed-in user as the operator. In a book without users, the operator name is what the local user types. Edit and cancel preserve the prior revisions. Updates to imported vouchers come through a later source revision.
- Run **Trial Balance**, **Ledger statement**, or **Cost centres** for a date range. Trial Balance uses the latest confirmed opening on or before the start date, then applies posted movements to that opening. When opening evidence is missing, OpenTally shows the movement and marks the balances unconfirmed.
- Close a reviewed period from **Company & import** to prevent subsequent postings or revisions through its closing date.

## Import Tally data

Use **Preview import** under **Company & import** to check counts, posted debits and credits, duplicates, and warnings. Select **Import into books** to write the full batch. Each book is linked to one source company. Source voucher GUIDs identify vouchers across imports, while the source type, series, and display number remain visible. An identical source revision is a duplicate. A higher alteration ID can update an open-period voucher.

The XML adapter reads Tally `TALLYMESSAGE` group, ledger, voucher type, and voucher objects, including ordered ledger entries and their nested cost allocations. Supply the source company if the file omits it. To adopt the exported ledger openings, enter their source effective date. The adapter retains the supplied XML text for audit and reports populated stock, bill, or bank allocation details that require separate processing. Use INR exports with supported ledger side and amount fields. Malformed XML, unresolved masters, and conflicting source revisions are reported before the batch is written. [Tally sample XML](https://help.tallysolutions.com/sample-xml/)

The supplied REA SQLite model can be converted with Python 3.11 or newer. Use a separate DuckDB file for the replay.

```sh
python3 scripts/rea_fixture.py "/path/to/REA.zip" --company "REA replay" --redact > rea-fixture.json
```

Upload the result as **OpenTally JSON**. The replay should show one SNA Payment, 12 source ledger masters, and 25 ordered postings. Compare both posting totals against the source fixture. The fixture supplies voucher movements. Use paired source openings and complete period exports for a Trial Balance reconciliation.

## Reconcile a bank account

Choose a bank ledger and review dates under **Bank reconciliation**. Import a CSV with these headers:

```csv
transaction_id,date,amount,reference,description
bank-tx-001,2025-09-03,-125.50,REF-001,Travel payment
```

Use a stable statement transaction ID, an ISO date, a positive amount for deposits, and a negative amount for withdrawals. Select one or more statement lines and the matching ledger postings. A reviewed match requires equal signed totals. Undo a match with an operator name and reason before editing a matched voucher.

## Command line

The `opentally` program also looks after a book directly. Stop the server first, because DuckDB lets one program at a time open a book for writing. Every command accepts `--db FILE` and `--json`, and exits with a fixed code: 0 for success, 2 for a usage error, 3 for invalid input, 4 for a conflict, 5 when something is missing, and 6 when the book is open in another program.

| Command | What it does |
| --- | --- |
| `opentally` or `opentally serve` | Start the server |
| `opentally migrate` | Upgrade the book's schema, keeping a backup first |
| `opentally info` | Company, schema version and number of users |
| `opentally user add NAME --name "Full Name" --role ROLE` | Add a user; the password is read from standard input |
| `opentally user list` / `user set NAME [--role ...] [--disable] [--password]` | List or change users |
| `opentally token create --user NAME --name LABEL --scope read` | Create an API token; the secret is shown once |
| `opentally token list` / `token revoke ID` | List or revoke tokens |
| `opentally backup FILE` / `restore FILE --yes` | Copy the book, or replace it from a copy (the old book is kept as `<book>.before-restore`) |
| `opentally mcp` | Run the MCP server for Claude and other MCP clients (see below) |

For example, to add the first user from a script:

```sh
printf '%s\n' "$PASSWORD" | opentally user add asha --name "Asha Patel" --role system_manager
```

## ChatGPT and Claude

OpenTally has a built-in MCP server at `/mcp` (Streamable HTTP), with the same 25 tools, prompts, guides and Trial Balance view as the Node connector. Tools run inside OpenTally with the caller's own rights: send an API token as `Authorization: Bearer ot_...` once the book has users. Write tools appear only when the client sends `x-opentally-mcp-writes: 1`. Requests must use a loopback host name, or one listed in `OPENTALLY_MCP_ALLOWED_HOSTS`.

For clients that start a local command, such as Claude Desktop and Claude Code, `opentally mcp` relays standard input and output to a running OpenTally. It reads `OPENTALLY_PORT`, `OPENTALLY_API_TOKEN` and `OPENTALLY_MCP_WRITES`, like the Node connector:

```json
{
  "mcpServers": {
    "opentally": {
      "command": "opentally",
      "args": ["mcp"],
      "env": { "OPENTALLY_PORT": "3000", "OPENTALLY_MCP_WRITES": "0" }
    }
  }
}
```

The Node connector below keeps working and will be retired once the Rust server has replaced it everywhere, including the Claude Desktop extension.

Start the OpenTally accounting app before using the connector. Building the connector or plugins from source requires [Node.js 20.3 or newer](https://nodejs.org/) and npm. Use the same local app port in the connector if you changed `OPENTALLY_PORT`.

| Client | How to connect |
| --- | --- |
| Claude Desktop | Build the `.mcpb` bundle below and install it from **Settings → Desktop app → Extensions → Advanced settings → Install Extension**. |
| Claude Code | Install the local `opentally-mcp` command and launch Claude Code with the plugin folder using the steps below. |
| ChatGPT | Run the remote OAuth MCP connector and add its public HTTPS `/mcp` endpoint as a custom MCP plugin. [ChatGPT setup](plugins/chatgpt/README.md) |
| Claude web | Run the same remote OAuth MCP connector and add its endpoint as a custom web connector. [Remote connector setup](connectors/mcp/README.md#chatgpt-and-claude-web) |

### Build the Claude Desktop extension

Run these commands from the repository root in a second terminal.

```sh
cd connectors/mcp
npm ci
npm run bundle:desktop
```

Install `connectors/mcp/dist/OpenTally-Claude-Desktop.mcpb` in Claude Desktop. Set the port to `3000` if you kept the default. Leave **Allow changes to the book** off for read-only access.

### Install the Claude Code plugin

Run these commands from the repository root. Claude Code must be installed, and `opentally-mcp` must be on its PATH after the npm install.

```sh
cd connectors/mcp
npm ci
npm pack
npm install -g ./opentally-mcp-0.3.0.tgz
cd ../..
claude --plugin-dir ./plugins/claude
```

The plugin includes the `/opentally:month-end` skill. [Claude Code marketplace setup](connectors/mcp/README.md#claude-code-plugin)

### Use the MCP tools

Try asking your connected assistant to show the Trial Balance for a date range in your book, list payments for the same period, or show a voucher’s revision history. Trial Balance includes an interactive view on clients that support MCP Apps. The connector also provides accounting guides, three review prompts and date suggestions for those prompts.

With write tools enabled, the assistant can post, edit and cancel vouchers, add ledger masters, commit imports, match bank statement lines and close reviewed periods. For a first write, use a practice book and check the saved entry in **Day Book**. For ChatGPT and Claude web, a public HTTPS MCP endpoint and an OAuth provider are required. Grant `opentally.read` for reading and `opentally.write` to operators authorized to change the books. [Connector setup and tool list](connectors/mcp/README.md)

## Backup and verification

Stop the app and copy the DuckDB file to back it up. Start the app with `OPENTALLY_DB` pointing to a separate copy to test the restore and compare the Day Book and reports.

```sh
cargo test --workspace --locked
```

GitHub Actions runs the same checks on every pull request: formatting, Clippy with warnings as errors, all tests, the MCP connector tests against the built app, a release build of the new interface, and a licence check of every dependency with `cargo deny` (see `deny.toml`).

The automated checks cover legacy migration, exact paise arithmetic, balanced and compound postings, XML import and re-import, period locks, auditable revisions, cost categories, and bank matching. The application’s accounting scope is one local book. Site-specific acceptance uses the matching Tally exports and reports for the chosen period.

## License

OpenTally is licensed under the [GNU Affero General Public License v3.0 only](LICENSE) (`AGPL-3.0-only`). Releases before this change were published under the MIT License and remain available under it.

If you run a modified OpenTally for users over a network, you must offer them its complete source code.

OpenTally's new features are being ported from [Frappe Books](https://github.com/frappe/frappe-books), © Frappe Technologies Pvt. Ltd., AGPL-3.0. Some accounting features are ported from [ERPNext](https://github.com/frappe/erpnext), © Frappe Technologies Pvt. Ltd., GPL-3.0, which may be combined with AGPL-3.0 code. See [NOTICE](NOTICE). OpenTally is not affiliated with or endorsed by Frappe.

The ChatGPT and Claude connector in `connectors/mcp` and the plugin in `plugins/claude` remain under the MIT License. They are separate programs that talk to OpenTally over HTTP.
