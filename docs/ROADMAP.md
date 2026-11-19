# OpenTally roadmap

This is a living document: new features are added here as they come up. Background and decisions: [PLAN-frappe-books-port.md](PLAN-frappe-books-port.md).

## Context

OpenTally (`vrajkher/TallyPrimeonRust-Duckdb`) is a Rust + DuckDB voucher book with vouchers, Trial Balance, Tally import, bank reconciliation and MCP tools. The owner decided (PR #4):

- **AGPL-3.0**: port Frappe Books `version-16` (upstream commit `953e9b4`) with **all capabilities**;
- **Leptos** for the UI;
- **multi-currency** in Level 2;
- **first release after Level 3** (accounting and reports, no inventory).

**Second source, ERPNext `version-16`** (upstream commit `886ab6a`, **GPL-3.0**). GPL-3.0 and AGPL-3.0 may be combined: section 13 of each licence allows it. The combined program is distributed under AGPL-3.0, and code taken from ERPNext keeps its GPL-3.0 notice.

- Frappe Books stays the **base**: it is small (~16k lines) and built for small businesses.
- ERPNext's accounts module is far larger: 192 doctypes, 63 reports, ~147k lines of Python and ~70k lines of tests. We **cherry-pick** the accounting features Books lacks. We do **not** port the rest of ERPNext (manufacturing, HR, CRM, projects).
- ERPNext v16 no longer contains India GST. Advanced GST (e-invoice, e-way bill, GSTR-3B, 2B reconciliation) comes from the **India Compliance** app (GPL-3.0, to be confirmed before porting).

This roadmap breaks the work into **10 levels**, plus an **AI integration track (Level A)** that runs alongside them. The track covers the MCP server, the CLI, plugins for Claude and ChatGPT, and AI features inside the app. Each level is one or more PRs, ends with green tests, and leaves the app usable. A level starts only when the one before it is done.

**Database decision:** **DuckDB now, PostgreSQL later.**
- DuckDB, one file per company, until v1.0 and beyond for small installs.
- All database access goes through `ot-store` with portable SQL, so a PostgreSQL backend can be added in Level 9 for many users and POS counters.
- On PostgreSQL, DuckDB attaches the PostgreSQL database for fast reports.

**Rules for every step**
- Port logic **and its tests** from the named source file. Put a header at the top of each derived Rust file: `// Derived from Frappe Books (AGPL-3.0) <path> @953e9b4, modified`, or `// Derived from ERPNext (GPL-3.0) <path> @886ab6a, modified`. List ERPNext in `NOTICE`.
- When Books and ERPNext both have a feature, start from Books and add ERPNext's extra rules only where a Tally-style user needs them.
- Sources are marked **[B]** for Frappe Books and **[E]** for ERPNext (`erpnext/accounts/...`).
- Each document type follows the same pattern, built once in Level 0 and reused:
  1. a migration;
  2. `ot-core` types and rules;
  3. an `ot-store` repository;
  4. `/api/v1` routes;
  5. a Leptos list and form;
  6. MCP tools;
  7. tests.
- Reuse what exists:
  - `Store::initialize` and the migration runner (`crates/ot-store/src/store.rs`, `crates/ot-store/src/migrate.rs`);
  - `Database::read` / `Database::write` (`crates/ot-server/src/db.rs`);
  - the revision model (`voucher_revisions`, `current_vouchers` view in `crates/ot-store/migrations/duckdb/0001_initial/2_schema.sql`);
  - the import preview/commit flow (`crates/ot-store/src/imports.rs`);
  - the acceptance-test style (`crates/ot-store/src/acceptance.rs`, `temporary_book()`).
- Toolchain: rustc 1.97 is installed. Add the `wasm32-unknown-unknown` target and Trunk in Level 0.

---

## Level 0 — Foundation (no new accounting features)

| # | Step | Details | Done when |
|---|---|---|---|
| 0.1 | Cargo workspace | Root `Cargo.toml` workspace with `crates/ot-core`, `ot-store`, `ot-api-types`, `ot-server`, `ot-web`. Move `store.rs`, `reports.rs`, `bank.rs`, `imports.rs` and the SQL files into `ot-store`, and `main.rs` into `ot-server`, **without changing behaviour**. Keep the binary name `opentally`. | `cargo test --locked` passes the same 4 acceptance tests |
| 0.2 | Migration runner | Turn `schema_migrations` into numbered files `ot-store/migrations/NNNN_name.sql` with a checksum. Version 1 = today's legacy + schema + migrate_v1. Copy the DB file to `*.bak-<version>` before migrating. | An old Voucher Desk book and a v1 book both open |
| 0.2b | Storage abstraction | `ot-store` exposes repository traits (`Ledgers`, `Documents`, `Postings`, `Reports`, `Auth`) with a `DuckDbStore` implementation. Rules: no DuckDB-only SQL outside `reports/` and `analytics/`. Use `BIGINT`/`DECIMAL` money, avoid `HUGEINT` in tables, use standard `UUID`/`TIMESTAMP`, and use sequences in place of DuckDB-specific defaults. The migration runner keeps a `duckdb/` and later a `postgres/` folder. | Traits used by the server; a lint test rejects banned SQL in shared files |
| 0.3 | Single writer | Replace `Arc<Mutex<Store>>` with a writer task (tokio mpsc) plus a read connection pool (`Connection::try_clone`). `run_db` keeps its signature. | Concurrent read requests do not block one another |
| 0.4 | Document framework | `ot-core::doc`: `DocStatus {Draft, Submitted, Cancelled}`, `NamingSeries`, `LinkedEntries`, a set-once field check. Port from `frappe_books/series.py`, `status.py`, `linked_entries.py`. | Unit tests |
| 0.5 | Money and currency | `ot-core::money`: `Money(i128 minor units)`, `Currency { code, precision }`, rounding half away from zero (Frappe's `ROUND_HALF_UP`). Port `accounting/money.py`, `currency.py`. | `test_currency.py` cases ported and passing |
| 0.6 | Posting engine | `ot-core::ledger::LedgerPosting` with `debit`/`credit`/`post`, per-line rounding and a round-off line limited to ½ unit per line. Port `accounting/ledger.py`. Keep OpenTally's revision rule for cancel (no reversal rows). | `test_ledger.py` ported and passing |
| 0.7 | Auth and roles | Tables `users`, `roles`, `user_roles`, `sessions`, `api_tokens`. Argon2 password hashing, HTTP-only cookie, CSRF header, first-run admin setup. Route guard by role. Port role rules from `permissions.py`. MCP uses an API token. | Unauthenticated requests get 401; `test_permissions.py` core cases pass |
| 0.8 | API v1 conventions | `/api/v1/{doctype}` list/get/insert/update/submit/cancel. Errors as `{error, field}`. Optimistic `revision` check returns 409. `utoipa` OpenAPI at `/api/v1/openapi.json`. The old `/api/*` routes stay until Level 3. | OpenAPI is generated; the old UI still works |
| 0.9 | Leptos shell | `ot-web` built with Trunk (CSR). Includes: a login page; the app shell (sidebar, top bar, search); a keyboard layer (F4–F9, Ctrl+A save, Esc back, Alt+C create); generic `ListView` and `FormView` components driven by `ot-api-types`; the **Day Book** screen on the v1 API; and a "Source code" link (AGPL §13). `ot-server` serves `ot-web/dist`. | Log in → Day Book shows vouchers |
| 0.10 | CI and quality | GitHub Actions: fmt, clippy `-D warnings`, test, `trunk build --release`, `cargo deny check licenses`. Cache the DuckDB build. Logging with `tracing`. | CI green on the PR |
| 0.11 | Performance probe | Bench: insert 100k vouchers, then run Trial Balance. Record the numbers in `docs/bench.md`. | Numbers recorded; act if Trial Balance takes more than 2 s |
| 0.12 | `opentally` CLI | `clap` subcommands on the one binary: `serve`, `migrate`, `user add/reset`, `token create/revoke`, `backup`, `restore`, `company list`. `--json` output and stable exit codes so AI agents can script it. | `opentally --help`; CLI tests |
| 0.13 | Native Rust MCP server | New crate `ot-mcp` using the official Rust MCP SDK (`rmcp`). It offers stdio (`opentally mcp`) and Streamable HTTP at `/mcp` on `ot-server`. It calls `ot-store` directly through the writer task, with no HTTP hop. Port the existing 25 tools, resources, 3 prompts and the Trial Balance widget from `connectors/mcp/src/*.mjs`, keeping the same tool names. | The connector's test cases pass against `ot-mcp` |

**PRs:** 0.1–0.3 · 0.4–0.6 · 0.7–0.8 · 0.9–0.10 · 0.11 · 0.12–0.13

---

## Level 1 — Setup and masters

| # | Step | Frappe source | Output |
|---|---|---|---|
| 1.1 | Settings | `books_system_settings`, `books_accounting_settings`, `books_defaults`, `books_print_settings`, `settings.py` | `settings` table; screens |
| 1.2 | Chart of accounts | `books_account`, `coa.py`, `data/charts/*.json` (in, standardCOA and 11 others) | `accounts` tree (root_type, account_type, gst_head, is_group); migration from `ledgers` + `account_groups` with **identical balances** |
| 1.3 | Setup wizard | `books_setup_wizard`, `setup.py`, `setup_service.py`, `data/country_info.json` | Company, country, currency, fiscal year, chart, bank account |
| 1.4 | Number series | `books_number_series`, `series.py` | Prefix, padding, next number per doctype |
| 1.5 | Parties | `books_party`, `books_address`, `regional.py` (India: GSTIN, state) | Customers and suppliers, default account, currency |
| 1.6 | Items | `books_item`, `books_item_group`, `books_uom`, `books_uom_conversion_item`, `inventory/units.py` | Items, groups, units, HSN, income/expense account |
| 1.7 | Taxes | `books_tax`, `books_tax_detail` | Tax templates (GST 5/12/18/28, split into CGST/SGST/IGST) |
| 1.8 | Payment methods, currencies, exchange rates | `books_payment_method`, `currency.py` | Masters, rate table |
| 1.9 | Leptos screens | — | Chart of Accounts tree; lists and forms for 1.4–1.8 built from the generic components |
| 1.9b | Fiscal years, accounting periods **[E]** | `doctype/fiscal_year`, `accounting_period` | Fiscal-year master; period locks generalise today's `closed_through` |
| 1.9c | Cost centres and accounting dimensions **[E]** | `doctype/cost_center`, `accounting_dimension`, `cost_center_allocation` | Today's cost categories become cost centres; extra dimensions (branch, project) |
| 1.9d | Payment terms **[E]** | `doctype/payment_term`, `payment_terms_template` | Credit days and instalments on parties and invoices |
| 1.10 | Tally master import | existing `imports.rs` | Tally groups and ledgers mapped onto the new tree |

**AI:** MCP tools and CLI commands for accounts, parties, items, taxes and the setup wizard (`opentally company create --chart in`).
**Tests ported:** `test_settings_accounts`, `test_settings_rules`, `test_installation`, `test_units`, `test_item_series`, `test_set_once_fields`, `test_field_rules`.
**Done when:** a new company created through the wizard has a full Indian chart, and an existing book migrates with an unchanged Trial Balance.

---

## Level 2 — Transactions

| # | Step | Frappe source | Output |
|---|---|---|---|
| 2.1 | Journal Entry v2 | `books_journal_entry`, `books_journal_entry_account` | Journal on new accounts, with party and entry type; existing vouchers keep working |
| 2.2 | Invoice engine | `accounting/invoice.py`, `books_*_invoice_item`, `books_tax_summary` | Line amounts, item and invoice discounts, tax summary, rounding, base/foreign amounts, `preview` endpoint |
| 2.3 | Sales Invoice | `books_sales_invoice` (non-POS parts) | Submit posts the GL through `InvoicePosting` |
| 2.4 | Purchase Invoice | `books_purchase_invoice` | Same, party credited |
| 2.5 | Sales Quote | `books_sales_quote` | Quote → invoice mapping |
| 2.6 | Payments | `accounting/payment.py`, `books_payment`, `books_payment_for`, `settlement.py` | Receive/pay, allocation to invoices, outstanding, status |
| 2.7 | Returns | `accounting/returns.py` | Credit and debit notes against an invoice |
| 2.8 | Multi-currency | `currency.py`, exchange-rate logic | Exchange gain/loss posting |
| 2.9 | Linked documents | `linked_entries.py`, `cancel_with_linked_docs` | Cancel cascade with checks |
| 2.9b | Payment Ledger **[E]** | `doctype/payment_ledger_entry`, `utils.py` (outstanding helpers) | Advances and outstanding per invoice and party, as Tally's bill-wise details |
| 2.9c | Payment Reconciliation tool **[E]** | `doctype/payment_reconciliation`, `unreconcile_payment` | Match unallocated payments to invoices in bulk; undo with a reason |
| 2.9d | Journal templates, opening invoices **[E]** | `journal_entry_template`, `opening_invoice_creation_tool` | Recurring journals; opening receivables and payables |
| 2.10 | Leptos forms | — | Keyboard-driven **item grid** (the main reusable component) for invoices and quotes; payment form; "make payment" and "make return" actions |
| 2.11 | MCP and CLI | — | Tools and `opentally invoice …` commands: draft, preview, submit, cancel invoice; record and allocate payment; outstanding by party. AI writes create **Drafts** by default, and a person submits them. |

**Tests ported:** `test_invoice_posting`, `test_invoice_preview`, `test_invoice_returns`, `test_settlement`, `test_exchange_rate`, `test_linked_entries`, `test_document_actions`, `test_document_save`, `test_status`.

---

## Level 3 — Reports, dashboard, release 1 🚀

| # | Step | Frappe source | Output |
|---|---|---|---|
| 3.1 | Report framework | `reports/filters.py`, `periods.py` | Filters, periods (monthly/quarterly/yearly), column model |
| 3.2 | General Ledger | `reports/general_ledger.py` | Filters: account, party, voucher, dates; group by |
| 3.3 | P&L and Balance Sheet | `reports/financial_statements.py` | Periodic columns, account tree totals |
| 3.4 | Trial Balance v2 | existing `reports.rs` + Frappe TB | On the new tree; keeps OpenTally's opening-evidence logic |
| 3.5 | Receivables/payables ageing | outstanding from 2.6 | 0–30 / 31–60 / 61–90 / 90+ day buckets |
| 3.5b | ERPNext core reports **[E]** | `report/accounts_receivable*`, `accounts_payable*`, `cash_flow`, `day_book`, `sales_register`, `purchase_register`, `customer_ledger_summary`, `supplier_ledger_summary`, `gross_profit` | Tally-equivalent statutory and daily reports |
| 3.5c | Period Closing Voucher **[E]** | `doctype/period_closing_voucher`, `account_closing_balance` | Year-end P&L transfer to retained earnings; fast opening balances |
| 3.6 | Dashboard | `reports/dashboard.py` | Cash, sales, expenses, top parties (chart components) |
| 3.7 | Export | — | CSV and XLSX (`rust_xlsxwriter`) |
| 3.8 | Retire old UI | — | Every old `web/` screen has a Leptos replacement; delete `web/` and the old `/api/*` routes (MCP moved to v1) |
| 3.9 | Basic deploy | — | `deploy/Dockerfile`, systemd unit, Caddy example, backup command (`CHECKPOINT` + copy) |
| 3.9b | Retire Node connector | — | `ot-mcp` covers every tool. The Claude Desktop `.mcpb` bundles the Rust binary (binary server type). `connectors/mcp` is archived. |
| 3.9c | AI v1 packaging | — | Claude Code plugin v2, ChatGPT app with report widgets, and setup docs for any MCP host (see Level A) |
| 3.10 | Release | — | README install guide, changelog, tag `v1.0.0` |

**Tests ported:** `test_reports` (financial parts), `test_dashboard`.
**Done when:** a sample Indian company can be run end to end — setup, masters, invoices, payments, returns, reports — through the Leptos UI and MCP.

---

## Level 4 — India GST

| # | Step | Frappe source |
|---|---|---|
| 4.1 | Place of supply → CGST+SGST or IGST; GSTIN and state validation | `regional.py`, `books_tax` |
| 4.2 | GSTR-1 and GSTR-2 reports | `reports/gst.py` |
| 4.3 | GSTR-1 JSON export (GSTN format) | `reports/gstr_json.py` |
| 4.4 | HSN summary | `reports/gst.py` |
| 4.5 | TDS/TCS (tax withholding) **[E]** | `doctype/tax_withholding_category`, `tax_withholding_rate`, `report/tds_computation_summary` |
| 4.6 | GSTR-3B, GSTR-2B purchase reconciliation | India Compliance |
| 4.7 | e-Invoice (IRN) and e-Way Bill: JSON export first, API later | India Compliance |

**Tests ported:** the GST parts of `test_reports`.

---

## Level 5 — Inventory

| # | Step | Frappe source |
|---|---|---|
| 5.1 | Locations | `books_location`, `test_stock_location` |
| 5.2 | Stock Ledger Entry and quantities | `books_stock_ledger_entry`, `inventory/stock.py`, `transaction.py` |
| 5.3 | FIFO valuation and **backdated repost** | `inventory/valuation.py`, `entry_costs.py` |
| 5.4 | Stock postings to the GL | `inventory/stock_posting.py` |
| 5.5 | Stock Movement, Shipment, Purchase Receipt | matching doctypes |
| 5.6 | Auto transfer from invoices, stock returns | `invoice_transfer.py`, `inventory/returns.py` |
| 5.7 | Availability checks | `availability.py` |
| 5.8 | Batches and serial numbers | `books_batch`, `books_serial_number` |
| 5.9 | Stock Ledger and Stock Balance reports | `reports/stock.py` |

**Tests ported:** `test_stock_*`, `test_valuation`, `test_auto_transfer`.

---

## Level 6 — Print and data import

| # | Step | Frappe source |
|---|---|---|
| 6.1 | Print formats (minijinja) for invoice, quote, payment, voucher, POS receipt | `printing.py`, `print_format/*` |
| 6.2 | PDF output (headless Chromium, already in the container) | — |
| 6.3 | CSV data import with templates and preview | `frontend/src/dataImport.ts`, `importer.ts`, `test_data_import` |
| 6.4 | Tally XML: invoices, inventory, bill allocations | existing `imports.rs` |

**Tests ported:** `test_printing`, `test_data_import`.

---

## Level 7 — POS, pricing, everything else

| # | Step | Frappe source |
|---|---|---|
| 7.1 | Price lists, pricing rules | `commerce/pricing.py`, `books_price_list*`, `books_pricing_rule*` |
| 7.2 | Coupons, loyalty | `commerce/loyalty.py`, `books_coupon_code`, `books_loyalty_*` |
| 7.3 | POS profile, opening/closing shift, cash denominations | `commerce/pos.py`, `books_pos_*` |
| 7.4 | POS checkout screen (barcode, fast keys) | `frontend/src/pages` POS |
| 7.5 | Leads, Item Enquiry | `books_lead`, `books_item_enquiry` |
| 7.6 | Custom Fields and Custom Forms | `customization.py`, `books_custom_*` |
| 7.7 | Translations (Gujarati and Hindi first, then all 20) | `locale/*.po` |
| 7.8 | Search palette | `search.py` |
| 7.9 | Phone layout and PWA | `pwa/`, `frontend/src/mobile` |

**Tests ported:** `test_pricing`, `test_loyalty`, `test_pos_payments`, `test_custom_fields`, `test_search`, `test_pwa`, `test_translation_extractors`.

---

## Level 8 — Advanced accounting from ERPNext **[E]**

| # | Step | ERPNext source (`erpnext/accounts/...`) |
|---|---|---|
| 8.1 | Bank transactions, rules, statement import (merge with OpenTally's `bank.rs`) | `doctype/bank_transaction*`, `bank_statement_import`, `bank_reconciliation_tool`, `report/bank_reconciliation_statement` |
| 8.2 | Budgets and budget variance | `doctype/budget`, `monthly_distribution`, `report/budget_variance_report` |
| 8.3 | Exchange-rate revaluation | `doctype/exchange_rate_revaluation` |
| 8.4 | Deferred revenue and expense | `deferred_revenue.py`, `report/deferred_revenue_and_expense` |
| 8.5 | Dunning (overdue reminders) and statements of account | `doctype/dunning*`, `process_statement_of_accounts` |
| 8.6 | Subscriptions | `doctype/subscription*` |
| 8.7 | Fixed assets and depreciation | `erpnext/assets/` (to be inspected), `report/asset_depreciation*` |
| 8.8 | Multi-company consolidation | `report/consolidated_*` |
| 8.9 | Financial ratios, profitability analysis, custom financial statements | `report/financial_ratios`, `profitability_analysis`, `financial_report_template` |
| 8.10 | Ledger health checks and repost | `doctype/ledger_health*`, `repost_accounting_ledger`, `report/general_and_payment_ledger_comparison` |
| 8.11 | Shareholders and share ledger | `doctype/share*`, `report/share_*` |

**Tests ported:** the matching `test_*.py` files next to each doctype.

---

## Level 9 — Production server

| # | Step | Details |
|---|---|---|
| 9.0 | PostgreSQL backend | `PgStore` with `sqlx` implementing the same traits; `postgres/` migrations; `OPENTALLY_DATABASE_URL=postgres://…`; the whole test suite runs on both backends in CI; reports run in DuckDB attached to PostgreSQL (`postgres` extension). |
| 9.0b | Move between backends | `opentally migrate-db --from duckdb:file --to postgres://…` and back, with Trial Balance checked before and after |
| 9.1 | Multi-company registry (one DuckDB file per company, company switcher) | — |
| 9.2 | Scheduled backups and restore; `EXPORT DATABASE` to Parquet | — |
| 9.3 | Hardening: rate limits, audit log of logins, password reset, 2-factor auth (optional) | — |
| 9.4 | Install guide matching docs.frappe.io/books/installation: binary or Docker → admin → TLS | — |
| 9.5 | Parity checklist: every Frappe Books row in [the plan's mapping](PLAN-frappe-books-port.md#4-frappe-books--opentally-mapping) and every Level 8 item ticked off; tag `v2.0.0` | — |

---

## Level A — AI integration track (runs alongside every level)

Every level adds its own MCP tools and CLI commands. Level A holds the shared AI foundation, the packaging, and the AI features inside the app.

### A1 — Rules for all AI access (built with Level 0)

| # | Step | Details |
|---|---|---|
| A1.1 | Generated tools | Each doctype registered in `ot-core` automatically gets `list_`, `get_`, `draft_`, `preview_`, `submit_` and `cancel_` tools, plus a CLI subcommand, from the same `ot-api-types` schemas. Hand-written tools only for reports and workflows. |
| A1.2 | Scopes | API tokens and OAuth carry scopes: `read`, `draft`, `submit`, `admin`. Writes are off by default, as the connector is today. |
| A1.3 | Draft-first | AI-created documents stay **Draft** until a person, or a token with the `submit` scope, submits them. Closed periods are refused as usual. |
| A1.4 | Audit | Every AI call is logged with actor `ai:<client>/<user>`, tool, arguments hash and result. AI changes are visible in revision history. |
| A1.5 | Safety | Reuse `request_key` idempotency (`crates/ot-store/src/store.rs`). Annotate `destructive` and `idempotent` on tools. Rate limits per token. Bound size and row counts of results. |
| A1.6 | OAuth for remote MCP | `ot-server` acts as an OAuth 2.1 authorisation server: protected-resource metadata, dynamic client registration and PKCE. Users sign in with their OpenTally login. This replaces the external identity provider needed by `connectors/mcp/src/oauth.mjs`. |

### A2 — MCP server features (grow with each level)

| # | Step | Details |
|---|---|---|
| A2.1 | Tools | Masters (L1), invoices and payments (L2), reports (L3), GST returns (L4), stock (L5), print/PDF (L6), POS and pricing (L7), budgets, assets and more (L8) |
| A2.2 | Resources | Chart of accounts, company settings, accounting guides, GST rules, the user's open drafts |
| A2.3 | Prompts | Month-end review, GST return preparation, bank reconciliation, overdue follow-up, year-end close, audit sample, Tally migration check |
| A2.4 | MCP Apps widgets | Trial Balance (existing), P&L, Balance Sheet, invoice preview, ageing, dashboard, GSTR-1 summary. One widget bundle serves ChatGPT and Claude. |
| A2.5 | Completions | Argument completion for accounts, parties, items and dates |
| A2.6 | Read-only SQL tool | `query_books`: read-only DuckDB SQL on documented views, with row and time limits, for ad-hoc analysis |

### A3 — CLI for people and agents

| # | Step | Details |
|---|---|---|
| A3.1 | Remote mode | `opentally --server https://… --token …` runs the same commands against a remote server |
| A3.2 | Data commands | `import tally <file>`, `import csv <doctype> <file>`, `export <doctype>`, `report tb/pl/bs/gl --from --to --format json/csv/xlsx` |
| A3.3 | Document commands | `invoice draft/submit`, `payment record`, `voucher post` from JSON on stdin |
| A3.4 | Agent friendliness | `--json` everywhere, `--dry-run` on writes, `opentally schema <doctype>` prints the JSON Schema |
| A3.5 | Shell completions | bash, zsh, fish and PowerShell |

### A4 — Plugins and hosts

| # | Host | Deliverable |
|---|---|---|
| A4.1 | Claude Code | `plugins/claude` v2. **Skills:** month-end (existing), gst-return, bank-recon, invoice-entry, tally-migration, year-end-close, audit-review, receivables-chase. **Agents:** an auditor subagent. **Hooks:** remind about Drafts left open. The marketplace manifest is updated. |
| A4.2 | Claude Desktop | `.mcpb` with the Rust binary for Windows, macOS and Linux; settings for port, token and writes |
| A4.3 | Claude web and ChatGPT | Remote `/mcp` with OAuth (A1.6); ChatGPT app listing with widgets (`plugins/chatgpt`) |
| A4.4 | Other MCP hosts | Copy-paste configs and tests for VS Code, Cursor, Gemini CLI, Codex CLI and other standard MCP clients |
| A4.5 | Distribution | GitHub release with binaries, `.mcpb`, the plugin zip and checksums |

### A5 — AI features inside the app (after Level 3)

All are optional and off by default. They use the owner's own API key. The provider is pluggable: the Claude API by default, or an OpenAI-compatible endpoint, which also covers local models. Settings show exactly which data is sent.

| # | Feature | Details |
|---|---|---|
| A5.1 | Ask the books | A chat panel in the Leptos UI using the same tools as `ot-mcp` (read scope), with answers linked to the source vouchers |
| A5.2 | Document capture | A bill PDF or photo becomes a Draft purchase invoice (party, items, GST, total) for review |
| A5.3 | Bank categorisation | Suggest the ledger and party for unmatched bank lines; learns from past matches |
| A5.4 | Narration and HSN suggestions | Fill narration, account and HSN/SAC on entry |
| A5.5 | Anomaly checks | Duplicate bills, unusual amounts, wrong GST rate, entries posted to suspense accounts, shown before period close |
| A5.6 | Natural-language reports | "Sales by state last quarter" → read-only query → table and chart |

### A6 — Testing AI integration

- MCP conformance tests (initialise, list tools, call tools) over stdio and HTTP.
- Golden snapshots of every tool's JSON schema, so changes are deliberate.
- Scope and permission tests: a read token cannot write, and a draft token cannot submit.
- An accounting task eval set (for example "post this bill", "reconcile September") run against a fixture book, with balanced-books checks.

## Order of work

```
L0 Foundation ─► L1 Masters ─► L2 Transactions ─► L3 Reports ─► 🚀 v1.0
                                                   │
                     ┌─────────────────────────────┘
                     ▼
                 L4 GST + TDS ─► L5 Inventory ─► L6 Print/Import ─► L7 POS & rest
                                                                         │
                                  v2.0 ◄─ L9 Production ◄─ L8 ERPNext advanced accounting

Level A runs alongside: A1–A3 with L0 · tools grow with L1–L8 · A4 ships at v1.0 · A5 after v1.0
```

## Status

| Level | State |
|---|---|
| Plan, licence, roadmap | ✅ Done (PR #4) |
| Level 0 | ⏳ In progress: 0.1–0.6 done (workspace, migrations, single writer, `ot-core` documents, money, series and posting engine); next 0.7–0.8. Step 0.2b (storage traits) is moved to the start of Level 1, where the first new repositories need it. |
| All later levels | Not started |

## Verification (each level)

- `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --locked`
- `trunk build --release` in `crates/ot-web`
- Ported Frappe tests pass (listed per level)
- Playwright end-to-end test of the level's main flow against `cargo run` (Chromium is pre-installed)
- Old-book migration test: a fixture book keeps the same Trial Balance before and after
- MCP connector tests: `cd connectors/mcp && npm test` until 3.9b, then the `ot-mcp` conformance and schema snapshot tests
- `claude --plugin-dir ./plugins/claude` loads the plugin; the MCP Inspector lists every tool over stdio and HTTP
