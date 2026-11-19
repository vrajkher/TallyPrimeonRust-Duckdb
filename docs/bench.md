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

The roadmap's target is a Trial Balance under 2 seconds on 100,000 vouchers. The 100,000-voucher run takes close to an hour at this posting rate; its results will be added here when it finishes. At 10,000 vouchers the Trial Balance takes 13 ms.

## What this shows

- **Reports are fast.** DuckDB is built for this: Trial Balance, ledger statements and Day Book pages stay well under a second.
- **Posting is the cost.** About 20 ms per voucher, almost all of it fixed overhead per statement: the same run on a RAM disk is as slow, so disk syncs are not the cause. Posting one voucher runs about ten statements (checks, numbering, insert, and reading the saved voucher back through the `book_register` view), and DuckDB spends a millisecond or more planning each one. The cost grows slightly with the size of the book.
- **This is enough for people, not for bulk loads.** 40–50 vouchers a second is far beyond what a person or a POS counter enters. Bulk loads must not post one voucher per transaction: the Tally import already writes a whole batch in one transaction, and every future importer must do the same.

## Follow-up

These are recorded for Level 2, where Journal Entry v2 rewrites the posting path anyway:

1. Read the saved voucher back by id from the base tables, not through the `book_register` view, which aggregates every voucher.
2. Reuse prepared statements (`prepare_cached`) on the writer connection.
3. Add a batch posting API (many vouchers, one transaction) for imports, the CLI and MCP.
4. Re-run this probe and add the numbers here, so regressions show.
