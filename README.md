# Voucher Desk

Voucher Desk is a local accounting app for recording payments, receipts, and journal entries in Indian rupees. Rust runs the application and embeds both the browser interface and DuckDB.

## Run

Install Rust 1.90 or newer and a C++ build toolchain. On Windows, the default Rust toolchain uses the Visual Studio C++ Build Tools. On macOS, use the Xcode Command Line Tools. The first build compiles DuckDB from source and can take several minutes.

```sh
cargo run --locked
```

Open [http://127.0.0.1:3000](http://127.0.0.1:3000). The server listens on the local computer. Use **Ctrl+C** in the terminal to stop it.

| Environment variable | Default |
| --- | --- |
| `VOUCHER_DB` | `data/vouchers.duckdb`, relative to the directory where the app starts |
| `VOUCHER_PORT` | `3000` |

For a backup, stop the app and copy the DuckDB file. To use a copy, point `VOUCHER_DB` at it when starting the app.

## Record a payment or receipt

1. Choose **Payment** (F5) or **Receipt** (F6).
2. Enter the voucher date and amount. Use a decimal point for paise, such as `1250.50`.
3. Select a cash or bank ledger and a corresponding party, expense, or income ledger. **Cash** and **Bank** are available initially. Create more accounts under **Ledgers**, or use **Add ledger** while entering a voucher.
4. Optionally enter a reference and narration. Review the debit and credit, then select **Save** or press **Ctrl+Enter**.
5. Filter the voucher register by type, date, or search term. Open a voucher number to review and print the saved voucher.

| Voucher | Debit | Credit |
| --- | --- | --- |
| Payment | Party or expense ledger | Cash or bank ledger |
| Receipt | Cash or bank ledger | Party or income ledger |

Payment and receipt vouchers each record one cash or bank ledger and one corresponding ledger. The app records transactions in one local book, and entries stay as posted.

## Record a journal

1. Choose **Journal** (F7) and enter the voucher date.
2. Choose the side, ledger, and amount for each line. Use **Add line** for a compound entry. For example, debit Salary Expense and credit Salary Payable to accrue unpaid salaries.
3. Review the debit and credit totals. When they match, select **Save journal** or press **Ctrl+Enter**. Open the saved voucher to see and print every posting line.

Journals use the `JRN-` numbering sequence. Select **Journals** in the register to find them by date, ledger, reference, or voucher number. The register totals add up the matching payment and receipt vouchers. Existing databases gain journal support when the app opens them.

## Verification

```sh
cargo test --locked
```

The tests check exact paise arithmetic, the ledger and side of each posting, voucher numbering, register filters, and duplicate-safe retries. The payment fixture uses the two Conveyance payments described in the supplied *TallyPrime · Rust and DuckDB Research* reference. [TallyHelp payment case study](https://help.tallysolutions.com/article/DeveloperReference/integration-capabilities/case_study_1.htm)

The entry workflow follows the documented accounting behavior for payments, receipts, and journal entries. [Payments](https://help.tallysolutions.com/record-payment-transactions/) · [Receipts](https://help.tallysolutions.com/record-receipt-transactions/) · [Journal entries](https://help.tallysolutions.com/journal-entries/)

The app uses the DuckDB Rust client. [DuckDB Rust client](https://duckdb.org/docs/current/clients/rust/overview)
