# Plan: Frappe Books features in OpenTally (Rust + DuckDB)

Status: **draft for review**. No code changes yet.

Decided by the owner: **AGPL-3.0, port Frappe Books with all its capabilities** (option B) and **a Rust UI with Leptos**.

Reference: [frappe/frappe-books `version-16`](https://github.com/frappe/frappe-books/tree/version-16) (Frappe Framework v16 app, Python + Vue, AGPL-3.0) and its [server installation guide](https://docs.frappe.io/books/installation).

## 1. Goal

Grow OpenTally from a voucher/ledger book into a full small-business accounting server with the scope of Frappe Books — chart of accounts, parties, items, sales and purchase invoices, payments, GST, inventory, reports, print, POS — while keeping OpenTally's own design:

- one Rust binary (axum) and one DuckDB file per company,
- Tally-style vouchers, keyboard-first entry and Tally XML import,
- immutable revision history, period closing, bank reconciliation,
- the MCP connector for ChatGPT and Claude.

We port **all of Frappe Books' capabilities** — its domain logic, rules, data files and tests — but not the Frappe Framework itself. There is no bench, MariaDB, Redis or Python in the result.

## 2. Licence (decided: option B)

Frappe Books is **AGPL-3.0-only**; OpenTally was **MIT**. Translating Frappe's Python/TypeScript, DocType JSON, chart-of-accounts files, translations or UI assets line by line produces a derivative work that must be AGPL.

| Option | Consequence |
| --- | --- |
| A. Clean reimplementation | Use Frappe Books only as a functional spec: feature list, user docs, observed behaviour, accounting rules that are general knowledge. Write our own schema and code. OpenTally stays MIT. |
| **B. Relicense OpenTally to AGPL-3.0 (chosen)** | We may port code and data files directly. Every network user of a hosted OpenTally is entitled to its source. |

OpenTally is now `AGPL-3.0-only` (`LICENSE`, `Cargo.toml`, `NOTICE`). Releases before the change stay available under MIT. The MCP connector and Claude plugin stay MIT because they are separate programs that use the HTTP API.

Rules that follow from this:

- **Mark derived files.** Each Rust file translated from Frappe Books starts with a header naming the Frappe Books source files it is based on and saying it was modified.
- **Reuse the data files.** We may adapt Frappe's chart-of-accounts templates, translations (20 locales, including `gu.po` Gujarati and `hi.po` Hindi), print formats and GSTR JSON mapping, keeping their copyright notices.
- **Port their tests.** Frappe's ~12k lines of integration tests become Rust tests and act as the acceptance suite for parity.
- **Offer the source.** The app shows a "Source code" link to every user (AGPL §13).
- **Respect the trademark.** Do not use the "Frappe" name or logo as our product name or branding.

## 3. What exists today

| Area | OpenTally today (`src/`) |
| --- | --- |
| Server | axum 0.8, single `Mutex<Store>`, no authentication (`main.rs`) |
| Storage | DuckDB 1.4.4, `schema.sql` + ad-hoc migrations (`migrate_v1.sql`, `legacy.sql`) |
| Masters | `ledgers` (kind), `account_groups` tree, dated `ledger_openings` |
| Vouchers | payment, receipt, journal, contra; ordered `posting_lines`, `cost_allocations`; every edit is a new `voucher_revisions` row |
| Reports | Trial Balance, ledger statement, cost-centre totals (`reports.rs`) |
| Import | Tally XML, OpenTally JSON, duplicate/revision detection (`imports.rs`) |
| Bank | CSV statements, grouped matching, undo with reason (`bank.rs`) |
| Controls | period closing, operator + reason on edit/cancel |
| Money | INR only, integer paise |
| UI | plain JS (`web/`) |
| AI | MCP server in `connectors/mcp` (Node), 25 tools |

## 4. Frappe Books → OpenTally mapping

Frappe Books has 66 DocTypes and 8 reports. Grouped by area:

| Frappe Books | OpenTally target | Notes |
| --- | --- | --- |
| Books Account (tree; `root_type`, `account_type`, `gst_head`) | `accounts` — merge `ledgers` + `account_groups` | Root types Asset/Liability/Equity/Income/Expense; account types Bank, Cash, Receivable, Payable, Tax, Stock, COGS, Round Off, etc. Tally's 28 primary groups map onto these. |
| Books Ledger Entry | `posting_lines` + `party_id` | Already our GL. Keep revision model rather than Frappe's "reverted" entries. |
| Books Journal Entry (+ Account rows) | journal voucher | Exists. Add entry types (opening, depreciation, write-off). |
| Books Payment, Payment For | payment / receipt + `bill_allocations` | Allocate against invoices (Tally bill-wise: new ref, against ref, advance, on account). |
| Books Party, Address, Lead | `parties`, `addresses`, `leads` | Customer / supplier / both; GSTIN, state, default account, currency. |
| Books Item, Item Group, UOM, UOM Conversion | `items`, `item_groups`, `units`, `unit_conversions` | HSN/SAC, income/expense account, default tax. |
| Books Tax, Tax Detail, Tax Summary | `tax_templates`, `tax_template_lines`, `voucher_tax_lines` | GST CGST/SGST/IGST split by place of supply. |
| Sales Quote, Sales Invoice, Purchase Invoice (+items), returns | voucher kinds `sales_quote`, `sales`, `purchase`, `credit_note`, `debit_note` | Item-invoice mode, like Tally. Outstanding amount derived from allocations. |
| Number Series | `number_series` | Per voucher kind, prefix, padding, fiscal-year reset. |
| Accounting / System / Defaults / Print Settings, Setup Wizard | `book_settings` expansion + setup flow | Fiscal year, currency, round-off/discount/write-off accounts, chart template. |
| Location, Stock Movement, Shipment, Purchase Receipt, Stock Ledger Entry | `locations` (godowns), voucher kinds `stock_journal`, `delivery_note`, `receipt_note`, `stock_ledger` | Perpetual inventory postings to Stock / COGS / SRNB. |
| Batch, Serial Number | `batches`, `serial_numbers` | Phase 5b. |
| FIFO valuation | `stock_valuation` (FIFO queue in Rust) | Moving average as a later option. |
| Price List, Pricing Rule, Coupon Code, Loyalty Program/Point Entry | `price_lists`, `pricing_rules`, `coupons`, `loyalty_*` | Phase 7. |
| POS Profile, Opening/Closing Shift, Cash Denominations, Payment Method | `pos_*` | Phase 7. |
| Custom Field / Custom Form | `custom_fields` (JSON values column) | Optional, last. |
| Frappe users, roles, permissions | `users`, `roles`, `sessions` | **Required before any server deployment.** |
| Reports: General Ledger, P&L, Balance Sheet, Trial Balance, Stock Ledger, Stock Balance, GSTR-1, GSTR-2, dashboard | `reports/` module | TB exists. DuckDB window functions and `GROUPING SETS` suit these well. |

## 5. Target architecture

```
TallyPrimeonRust-Duckdb/
├─ Cargo.toml                 # workspace
├─ crates/
│  ├─ ot-core/                # pure domain, no I/O
│  │   money (i128 minor units, currency, rounding), accounts tree,
│  │   posting engine (balanced Dr/Cr, round-off), invoice/tax calc,
│  │   GST rules, FIFO valuation, number series
│  ├─ ot-store/               # DuckDB: migrations, repositories, report SQL
│  ├─ ot-import/              # Tally XML, OpenTally JSON, CSV templates
│  ├─ ot-print/               # HTML templates (minijinja) → PDF
│  ├─ ot-api-types/           # serde types shared by server and UI (WASM-safe)
│  ├─ ot-server/              # axum: REST API, auth, sessions, serves the UI
│  └─ ot-web/                 # Leptos CSR UI → WASM (Trunk)
├─ web/                       # current plain-JS UI, retired screen by screen
├─ connectors/mcp/            # existing connector, new tools per phase
└─ deploy/                    # Dockerfile, systemd unit, Caddy/nginx examples
```

Key choices:

- **Posting engine in `ot-core`.** Every document (invoice, payment, stock move) produces a `Posting { lines: Vec<Line> }` through one function, validated as balanced, rounded per line with a round-off line absorbing ≤ ½ minor unit per line — the same rule Frappe's `LedgerPosting` enforces, written fresh. Fully unit-testable without a database.
- **Keep the revision model.** Submit = revision 1 `posted`; amend = new revision; cancel = revision with status `cancelled`. Reports read the current revision. This is stricter than Frappe's reversal entries and already passes our acceptance tests.
- **Money**: keep integer minor units; widen to `HUGEINT`/`i128`; add `currency`, `exchange_rate` (decimal, stored as scaled integer) and base-currency amounts for multi-currency (phase 2b).
- **Migrations**: replace ad-hoc SQL with numbered, checksummed migrations recorded in `schema_migrations`; run in a transaction at start-up; take a file backup before migrating.
- **Concurrency**: DuckDB allows one writing process per file. One server process owns each company file; reads use cloned connections, writes go through a single writer task (tokio mpsc) instead of a global `Mutex`. Multiple companies = multiple files listed in a small `companies.toml` / registry DB.
- **API**: JSON REST under `/api/v1/{resource}`, document workflow `draft → submit → cancel/amend`, optimistic concurrency with a `revision` field (Frappe uses `modified`). An OpenAPI spec generated with `utoipa` keeps the UI and MCP tools in step.
- **Auth**: argon2 password hashes, HTTP-only session cookies, CSRF token, roles (Admin, Accountant, Sales, Purchase, Stock, POS Cashier, Viewer) checked per route and per voucher kind. API tokens for MCP.
- **UI: Leptos, client-side rendered (CSR)**, built to WebAssembly with Trunk and served as static files by `ot-server`. It calls the same `/api/v1` REST API as the MCP connector, so there is one API for people and AI. Shared request/response types live in an `ot-api-types` crate that compiles for both the server and `wasm32-unknown-unknown`. We do not use Leptos server-side rendering or server functions: DuckDB cannot build for WASM, and CSR avoids hydration bugs. The existing plain-JS UI keeps working until each Leptos screen replaces it.

## 6. Phases

Each phase ends with passing `cargo test`, extended acceptance tests (`src/acceptance.rs` style), updated MCP tools and README.

### Phase 0 — Foundation
- Convert to a Cargo workspace and move current modules into `ot-store` / `ot-server` with no behaviour change.
- Numbered migration runner; migrate existing books in place.
- Users, roles, login, sessions, API tokens; first-run admin creation.
- Single-writer DB task; config file + env vars; structured logging (`tracing`).
- Leptos skeleton: login, app shell, keyboard shortcut layer, one screen (Day Book) on the new API.
- CI: fmt, clippy, tests, WASM build, `cargo deny` licence check.

**Done when** existing books open unchanged, all current tests pass, and the app refuses unauthenticated requests when bound to a non-loopback address.

### Phase 1 — Chart of accounts and masters
- `accounts` tree with root/account types; migrate `ledgers` + `account_groups` into it.
- Indian chart template (Tally-style primary groups) + a minimal generic template; setup wizard (company, fiscal year, currency, GSTIN, chart).
- Parties, addresses, items, item groups, units, tax templates, number series.
- CRUD API + UI lists/forms; import masters from Tally XML.

### Phase 2 — Sales, purchases, payments
- Sales quote → sales invoice → payment; purchase invoice → payment; credit/debit notes (returns) against an invoice.
- Line calc: qty × rate, item and invoice discount, tax templates, rounding, grand total, outstanding.
- Payment allocation to invoices; advances; outstanding and status (Unpaid / Partly Paid / Paid / Overdue / Return).
- Posting rules in `ot-core` with exhaustive unit tests (sale, purchase, return, discount, tax, round-off).
- Multi-currency (party currency, exchange rate, base amounts, exchange gain/loss), as Frappe Books does.

### Phase 3 — Financial reports and dashboard
- General Ledger (filters: account, party, voucher, dates), Profit & Loss, Balance Sheet (periodic columns), Trial Balance on the new tree, receivables/payables ageing, dashboard (cash, sales, expenses, top parties).
- CSV and XLSX export; print view.

### Phase 4 — India GST
- Place-of-supply logic (intra → CGST+SGST, inter → IGST), HSN/SAC summary.
- GSTR-1 and GSTR-2 reports and GSTR-1 JSON export.
- Validate GSTIN format and state code.

### Phase 5 — Inventory
- Locations, stock journal (material receipt/issue/transfer), delivery notes, receipt notes, auto stock transfer from invoices.
- Stock ledger, FIFO valuation, perpetual-inventory GL postings, Stock Ledger and Stock Balance reports.
- 5b: batches, serial numbers, unit conversions.

### Phase 6 — Print and data import
- Print templates (invoice, quote, payment, voucher) in minijinja; PDF via headless Chromium or `typst`.
- CSV import templates per document type with preview (reuse the existing preview/commit flow).
- Extend Tally XML import to inventory vouchers and bill allocations.

### Phase 7 — POS and pricing
- Price lists, pricing rules, coupons, loyalty programmes.
- POS profile, opening/closing shift with cash denominations, barcode lookup, fast checkout screen.

### Phase 7b — Remaining Frappe Books capabilities
- Leads and Item Enquiry; Swiss regional fields.
- Custom Fields and Custom Forms.
- Translations: load Frappe's `.po` files (Gujarati and Hindi first) into the Leptos UI.
- Phone layout with bottom tabs and an installable PWA, as Frappe Books' phone app.
- Parity check: every Frappe Books DocType and report in §4 is implemented and its ported tests pass.

### Phase 8 — Server deployment
- `deploy/Dockerfile` (multi-stage, static binary + bundled DuckDB), `docker-compose.yml`, systemd unit.
- TLS via Caddy or nginx reverse proxy; `OPENTALLY_BIND_ADDR=0.0.0.0` only behind it.
- Backups: `CHECKPOINT` then file copy, or `EXPORT DATABASE` to Parquet on a schedule; restore command.
- Install guide equivalent to Frappe's, but: download binary → set admin password → run.

## 7. Difficulties to expect

### AGPL obligations
- **Track every port.** Every derived file needs its source header, and `NOTICE` must stay accurate.
- **Python and Vue are not Rust.** We translate logic and tests, not run them. Frappe relies on framework features (DocType meta, hooks, permissions, naming, `docstatus`) that we must provide ourselves in Rust.
- **Follow upstream.** Frappe Books keeps changing. Record the upstream commit each port is based on (`953e9b4` on `version-16` for this plan) so later fixes can be pulled across.
- **Check dependencies.** Run `cargo deny` in CI to allow only AGPL-compatible crates (MIT, Apache-2.0, BSD, MPL-2.0 and similar). Leptos, axum and DuckDB are fine.
- **No closed-source edition.** Anyone who uses a hosted OpenTally may ask for its source, including our changes.

### Leptos
- **Two build targets.** The UI compiles to `wasm32`, so anything it shares with the server must avoid DuckDB, tokio and file I/O. Getting the crate split right in phase 0 prevents a lot of `cfg` noise later.
- **Toolchain pinning.** Trunk, `wasm-bindgen` and its CLI must be the same version, and CI needs the WASM target installed.
- **API churn.** Leptos changed its API a lot between 0.6, 0.7 and 0.8. Pin one version and plan upgrades deliberately.
- **Few ready-made components.** There is no mature data grid, date picker or chart library. The keyboard-driven voucher grid (Tally-style entry) must be built ourselves, and it is the heart of the app.
- **Keyboard and focus handling.** Global shortcuts (F4–F9, Ctrl+A, Esc) and focus moves through `web_sys` are verbose. Build one shortcut/focus layer early and reuse it.
- **Bundle size.** A WASM bundle is 1–3 MB. Use release builds, `wasm-opt` and compression; split later if needed.
- **Testing.** Logic goes in plain Rust and is unit-tested; screens get Playwright end-to-end tests.
- **Learning curve.** Signals plus Rust ownership in closures takes time to get comfortable with.

### DuckDB as a live accounting database
- **One writer.** Only one process may open a file for writing, so the server must own it; the MCP connector already goes through HTTP. Concurrent transactions that touch the same rows abort with a conflict, so writes go through one writer task.
- **Limited constraints and ALTER TABLE.** Foreign keys have no cascades, updates to indexed or referenced columns are restricted, and many schema changes need a table rebuild. The append-only revision model suits this; migrations must be tested on real books.
- **Small writes.** DuckDB is built for analytics. Many small commits are fine at small-business scale but must be benchmarked early (100k vouchers).
- **Upgrades and backups.** Moving off the pinned DuckDB version needs a tested export/import path. Back up with `CHECKPOINT` then a file copy, or `EXPORT DATABASE`.
- **Build time.** The bundled DuckDB C++ build takes minutes; cache it in CI.

### Accounting
- **Rounding.** Per-line versus per-invoice GST rounding and round-off must match what users and the GST portal expect.
- **GST edge cases.** Place of supply, reverse charge, exempt and nil-rated supplies, composition dealers and SEZ. E-invoice and e-way bill are out of scope at first.
- **Backdated stock entries.** A backdated entry changes FIFO costs for every later sale, so the stock ledger and COGS postings must be recalculated. This is the hardest technical piece.
- **Revisions with payments.** Editing or cancelling an invoice that already has payments allocated, or that sits in a closed period, needs clear rules.
- **Moving existing books.** Converting today's `ledgers` and `account_groups` into the new account tree without changing any balance.

### Server
- **Security.** The app has no login today. Login, roles, CSRF protection, secure cookies and TLS through a reverse proxy are needed before anyone exposes it on a network.
- **Scope.** Full parity is a lot of work for a small team. Phases 0–3 still make the first usable release; later phases follow in order.

## 8. Risks

| Risk | Mitigation |
| --- | --- |
| Missing attribution on ported code | Source headers and `NOTICE` reviewed in every PR. |
| DuckDB is analytics-first; many small writes | Batch each document in one transaction; single writer; benchmark 100k vouchers early (phase 0). |
| One process per DB file | Fine for SMB; document that horizontal scaling is out of scope. |
| Revision model vs. Frappe's reversal entries | Decided: keep revisions; reports already read current revisions. |
| Scope (Frappe Books is ~16k lines of Python plus a large Vue app) | Ship phases 0–3 as the first usable release; later phases are independent. |

## 9. Open questions for the owner

1. ~~Licence~~: AGPL-3.0, port Frappe Books.
2. ~~UI~~: Leptos.
3. ~~Multi-currency~~: in phase 2, as part of full capability.
4. ~~Scope~~: all Frappe Books capabilities (phases 0–8, including 7b).
5. First release: after phase 3 (accounting server) unless the owner prefers to wait for inventory (phase 5).
