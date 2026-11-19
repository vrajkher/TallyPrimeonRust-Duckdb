//! Performance probe (roadmap step 0.11): posts vouchers through the store, one
//! transaction each as the app does, then times the reads people wait on.
//!
//! cargo run --release -p ot-store --example bench -- [vouchers] [book.duckdb]
//!
//! Without a path the book is written to a temporary file, not kept in memory, so the
//! numbers include disk writes.

use std::time::{Duration, Instant};

use ot_store::Store;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("valid request")
}

fn time<T>(label: &str, runs: u32, mut work: impl FnMut() -> T) -> Duration {
    work(); // warm up
    let start = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(work());
    }
    let each = start.elapsed() / runs;
    println!("{label:<38} {:>9.1} ms", each.as_secs_f64() * 1000.0);
    each
}

fn main() {
    let mut args = std::env::args().skip(1);
    let count: u32 = args
        .next()
        .map_or(100_000, |n| n.parse().expect("a voucher count"));
    let path = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let dir = std::env::temp_dir().join(format!("ot-bench-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            dir.join("bench.duckdb")
        });
    let _ = std::fs::remove_file(&path);
    let mut store = Store::open(&path).expect("open the book");

    // 20 expense ledgers and 20 parties, as a small business might have.
    let mut ledgers = Vec::new();
    for n in 0..40 {
        let ledger = store
            .add_ledger(decode(
                json!({"name": format!("Ledger {n}"), "kind": "general"}),
            ))
            .unwrap();
        ledgers.push(
            serde_json::to_value(ledger).unwrap()["id"]
                .as_i64()
                .unwrap(),
        );
    }

    println!("Posting {count} vouchers to {}", path.display());
    let start = Instant::now();
    let mut slowest = Duration::ZERO;
    for n in 0..count {
        let other = ledgers[n as usize % ledgers.len()];
        let day = 1 + n % 28;
        let month = 4 + (n / 28) % 12; // April to March
        let (year, month) = if month > 12 {
            (2026, month - 12)
        } else {
            (2025, month)
        };
        let kind = match n % 10 {
            0..=5 => "payment",
            6..=8 => "receipt",
            _ => "journal",
        };
        let request_id = format!("29daeb83-d7be-4576-aca0-{n:012}");
        let date = format!("{year}-{month:02}-{day:02}");
        let amount = format!("{}.{:02}", 10 + n % 50_000, n % 100);
        let body = if kind == "journal" {
            json!({"kind": kind, "request_id": request_id, "date": date, "postings": [
                {"ledger_id": other, "side": "debit", "amount": amount},
                {"ledger_id": 2, "side": "credit", "amount": amount}]})
        } else {
            json!({"kind": kind, "request_id": request_id, "date": date, "amount": amount,
                   "cash_ledger_id": 1 + (n % 2) as i64, "other_ledger_id": other,
                   "reference": format!("R{n}"), "narration": "Benchmark voucher"})
        };
        let one = Instant::now();
        store.add_voucher(decode(body)).expect("posted");
        slowest = slowest.max(one.elapsed());
        if (n + 1) % 20_000 == 0 {
            println!(
                "  {:>7} posted, {:.0} vouchers/s",
                n + 1,
                f64::from(n + 1) / start.elapsed().as_secs_f64()
            );
        }
    }
    let elapsed = start.elapsed();
    println!(
        "{:<38} {:>9.0} vouchers/s ({:.1} s; mean {:.2} ms, slowest {:.1} ms)",
        "Posting",
        f64::from(count) / elapsed.as_secs_f64(),
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / f64::from(count),
        slowest.as_secs_f64() * 1000.0
    );

    let year = json!({"from": "2025-04-01", "to": "2026-03-31"});
    let month = json!({"from": "2025-09-01", "to": "2025-09-30"});
    let tb = time("Trial Balance, full year", 5, || {
        store.trial_balance(decode(year.clone())).unwrap()
    });
    time("Trial Balance, one month", 5, || {
        store.trial_balance(decode(month.clone())).unwrap()
    });
    time("Ledger statement, one month", 5, || {
        store.statement(ledgers[0], decode(month.clone())).unwrap()
    });
    time("Day Book, first page", 5, || {
        store.register(decode(json!({}))).unwrap()
    });
    time("Day Book, page 2000", 5, || {
        store.register(decode(json!({"page": 2000}))).unwrap()
    });
    time("Day Book, text search", 5, || {
        store.register(decode(json!({"q": "R99999"}))).unwrap()
    });
    drop(store);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("{:<38} {:>9.1} MB", "Book file size", size as f64 / 1e6);
    if tb > Duration::from_secs(2) {
        println!("Trial Balance is over the 2 s target.");
    }
}
