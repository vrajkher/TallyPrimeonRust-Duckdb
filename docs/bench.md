# Performance probe (roadmap step 0.11)

Measured with `crates/ot-store/examples/bench.rs`, which posts vouchers through the real store code (one transaction per voucher, as the app does) and then times the reads people wait on.

```sh
cargo run --release -p ot-store --example bench -- 100000
```

Machine: the 4-core cloud container used for development, release build, DuckDB 1.4.4 (bundled), book file on local disk unless stated.

## Results

| Vouchers in the book | Posting rate | Mean per voucher | Trial Balance, full year | Ledger statement, one month | Day Book page | Day Book text search | Book file |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1,000 | 47/s | 21.4 ms | — | — | — | — | — |
| 1,000 (RAM disk) | 50/s | 20.2 ms | 10.3 ms | — | — | — | — |
| 3,000 | 45/s | 22.1 ms | — | — | — | — | — |
| 10,000 | 40/s | 25.3 ms | 13.2 ms | 19.5 ms | 53.8 ms | 63.0 ms | 7.9 MB |
| 100,000 | 39/s | 25.6 ms (slowest 366 ms) | 38.2 ms | 52.2 ms | 260.5 ms (page 2000: 314.7 ms) | 363.8 ms | 51.4 MB |

**The roadmap's target is met:** a full-year Trial Balance over 100,000 vouchers takes 38 ms, against a limit of 2 seconds. Posting the 100,000 vouchers one at a time took 43 minutes.

## What this shows

- **Reports are fast.** DuckDB is built for this: Trial Balance and ledger statements take tens of milliseconds even at 100,000 vouchers.
- **The Day Book grows with the book.** A page takes 54 ms at 10,000 vouchers and 260 ms at 100,000, roughly in step with size, because the `book_register` view aggregates every voucher before the page is cut. It is still well under a second, but it is the first read to fix.
- **Posting is the cost.** About 20 ms per voucher, almost all of it fixed overhead per statement: the same run on a RAM disk is as slow, so disk syncs are not the cause. Posting one voucher runs about ten statements (checks, numbering, insert, and reading the saved voucher back through the `book_register` view), and DuckDB spends a millisecond or more planning each one. The cost grows slightly with the size of the book.
- **This is enough for people, not for bulk loads.** 40–50 vouchers a second is far beyond what a person or a POS counter enters. Bulk loads must not post one voucher per transaction: the Tally import already writes a whole batch in one transaction, and every future importer must do the same.

## Follow-up

These are recorded for Level 2, where Journal Entry v2 rewrites the posting path anyway:

1. Stop going through the `book_register` view, which aggregates every voucher: read a saved voucher back by id from the base tables, and filter and page the Day Book before aggregating.
2. Reuse prepared statements (`prepare_cached`) on the writer connection.
3. Add a batch posting API (many vouchers, one transaction) for imports, the CLI and MCP.
4. Re-run this probe and add the numbers here, so regressions show.
