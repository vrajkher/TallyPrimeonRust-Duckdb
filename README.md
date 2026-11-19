# OpenTally

OpenTally is a local accounting application for one INR company per DuckDB file. Rust serves the browser interface on your computer. It supports compound vouchers, Tally export import, accounting reports, and bank reconciliation.

## Run

Install Rust 1.90 or newer and a C++ toolchain. Windows builds with the default Rust toolchain require the Visual Studio C++ Build Tools. macOS builds require the Xcode Command Line Tools. The first build compiles the bundled DuckDB library.

```sh
cargo run --locked
```

Open [http://127.0.0.1:3000](http://127.0.0.1:3000). Use **Ctrl+C** in the terminal to stop the app.

| Setting | Default |
| --- | --- |
| `OPENTALLY_DB` | `data/vouchers.duckdb`, relative to the starting directory |
| `OPENTALLY_PORT` | `3000` |

The previous `VOUCHER_DB` and `VOUCHER_PORT` settings are accepted. The default path also matches Voucher Desk. When OpenTally first opens an existing file, it migrates vouchers to ordered posting lines while preserving their IDs and numbers. Use OpenTally to post new entries after migrating a book.

## Use the books

- Create account groups and ledgers, enter openings with their effective date and source, and set the company name under **Company & import**. Enter an opening debit as a positive amount and an opening credit as a negative amount. Use `0.00` for an explicitly confirmed zero.
- Use **Payment** (F5), **Receipt** (F6), **Journal** (F7), or **Contra**. Select **Enter multiple posting lines** for a compound payment or receipt. Contra transfers use distinct cash or bank ledgers. Add line narration and cost allocations through each line’s details. Review the debit and credit totals, then save with **Ctrl+Enter**.
- Open **Day Book** to search by type, status, dates, voucher number, or ledger. Open the voucher number to print or inspect its history. Edit or cancel an open-period local voucher using an operator name and reason. The operator name is entered by the local user. Edit and cancel preserve the prior revisions. Updates to imported vouchers come through a later source revision.
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

## ChatGPT and Claude

OpenTally has a common MCP connector for ChatGPT and Claude, a Claude Desktop extension, and a Claude Code plugin. The connector has reader and writer modes and includes an interactive Trial Balance view. [Connector setup](connectors/mcp/README.md) · [ChatGPT plugin setup](plugins/chatgpt/README.md)

## Backup and verification

Stop the app and copy the DuckDB file to back it up. Start the app with `OPENTALLY_DB` pointing to a separate copy to test the restore and compare the Day Book and reports.

```sh
cargo test --locked
```

The automated checks cover legacy migration, exact paise arithmetic, balanced and compound postings, XML import and re-import, period locks, auditable revisions, cost categories, and bank matching. The application’s accounting scope is one local book. Site-specific acceptance uses the matching Tally exports and reports for the chosen period.
