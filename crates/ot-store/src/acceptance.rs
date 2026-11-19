use duckdb::Connection;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    reports::Period,
    store::{Store, StoreError},
};

fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

fn temporary_book() -> Store {
    Store::initialize(Connection::open_in_memory().unwrap()).unwrap()
}

fn september() -> Period {
    Period {
        from: "2025-09-01".into(),
        to: "2025-09-30".into(),
    }
}

fn key(n: u32) -> String {
    format!("29daeb83-d7be-4576-aca0-{n:012}")
}

fn submit(store: &mut Store, input: Value) -> Value {
    serde_json::to_value(store.add_voucher(decode(input)).unwrap().0).unwrap()
}

#[test]
fn import_previews_replays_revises_and_checks_source_identity() {
    let mut store = temporary_book();
    let xml = r#"
      <ENVELOPE><BODY><TALLYMESSAGE>
        <GROUP NAME="Travel"><PARENT>Expenses</PARENT></GROUP>
        <GROUP NAME="Expenses"/><GROUP NAME="Bank Accounts"/>
        <LEDGER NAME="Travel Expense"><PARENT>Travel</PARENT><OPENINGBALANCE>0.00</OPENINGBALANCE></LEDGER>
        <LEDGER NAME="Current Account"><PARENT>Bank Accounts</PARENT><OPENINGBALANCE>-42.00</OPENINGBALANCE></LEDGER>
        <VOUCHERTYPE NAME="SNA"><PARENT>Payment</PARENT></VOUCHERTYPE>
        <VOUCHER VCHTYPE="SNA">
          <GUID>synthetic-source-id</GUID><ALTERID>17</ALTERID><DATE>20250903</DATE>
          <VOUCHERNUMBER>SNA-1</VOUCHERNUMBER><VOUCHERNUMBERSERIES>Default</VOUCHERNUMBERSERIES>
          <ALLLEDGERENTRIES.LIST><LEDGERNAME>Travel Expense</LEDGERNAME><AMOUNT>-2.01</AMOUNT>
            <ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>
          </ALLLEDGERENTRIES.LIST>
          <ALLLEDGERENTRIES.LIST><LEDGERNAME>Current Account</LEDGERNAME><AMOUNT>2.01</AMOUNT></ALLLEDGERENTRIES.LIST>
        </VOUCHER>
      </TALLYMESSAGE></BODY></ENVELOPE>"#;
    let request = |content: &str, company: &str, commit| {
        decode(json!({
            "format":"xml", "content":content, "company":company, "opening_date":"2025-09-01", "commit":commit
        }))
    };

    let preview = store.import(request(xml, "Pilot", false)).unwrap();
    assert_eq!(
        (
            preview.groups,
            preview.ledgers,
            preview.postings,
            preview.inserted
        ),
        (3, 2, 2, 1)
    );
    assert_eq!(preview.debit_paise, "201");
    assert_eq!(
        serde_json::to_value(store.register(Default::default()).unwrap()).unwrap()["count"],
        0
    );

    assert_eq!(
        store.import(request(xml, "Pilot", true)).unwrap().inserted,
        1
    );
    assert_eq!(
        store
            .import(request(xml, "Pilot", true))
            .unwrap()
            .duplicates,
        1
    );
    assert!(matches!(
        store.import(request(xml, "Other source", true)),
        Err(StoreError::Conflict(_))
    ));

    let inconsistent = xml.replace("<ISDEEMEDPOSITIVE>Yes", "<ISDEEMEDPOSITIVE>No");
    assert!(matches!(
        store.import(request(&inconsistent, "Pilot", true)),
        Err(StoreError::Invalid(_))
    ));
    let updated = xml
        .replace("<ALTERID>17", "<ALTERID>18")
        .replace("2.01", "4.22")
        .replace(
            "<VOUCHERTYPE NAME=\"SNA\"><PARENT>Payment</PARENT></VOUCHERTYPE>",
            "",
        );
    assert_eq!(
        store
            .import(request(&updated, "Pilot", true))
            .unwrap()
            .revised,
        1
    );
    assert!(matches!(
        store.import(request(xml, "Pilot", true)),
        Err(StoreError::Conflict(_))
    ));
    let report = serde_json::to_value(store.trial_balance(september()).unwrap()).unwrap();
    assert_eq!(report["debit_paise"], "422");
    let current_account = report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["ledger"] == "Current Account")
        .unwrap();
    assert_eq!(current_account["opening_paise"], "4200");
    assert_eq!(current_account["closing_paise"], "3778");
    assert_eq!(report["closing_debit_paise"], Value::Null);
}

#[test]
fn compound_postings_cost_categories_corrections_and_period_locks() {
    let mut store = temporary_book();
    let expense = store
        .add_ledger(decode(json!({"name":"Expense","kind":"general"})))
        .unwrap()
        .id;
    let make_payment = |id, amount: &str| {
        json!({
            "kind":"payment","request_id":key(id),"date":"2025-09-03",
            "postings":[
                {"ledger_id":expense,"amount":amount,"side":"debit","allocations":[
                    {"category":"Programme","centre":"Clinic","amount":amount},
                    {"category":"Region","centre":"West","amount":amount}
                ]},
                {"ledger_id":2,"amount":amount,"side":"credit"}
            ]
        })
    };
    let payment = submit(&mut store, make_payment(1, "10.25"));
    let id = payment["id"].as_i64().unwrap();
    let costs = serde_json::to_value(store.cost_totals(september()).unwrap()).unwrap();
    assert_eq!(costs.as_array().unwrap().len(), 2);
    assert_eq!(costs[0]["debit_paise"], "1025");
    let cost_lines = store
        .cost_postings(decode(json!({
            "from":"2025-09-01", "to":"2025-09-30", "category":"Programme", "centre":"Clinic"
        })))
        .unwrap();
    let cost_lines = serde_json::to_value(cost_lines).unwrap();
    assert_eq!(cost_lines[0]["voucher_id"], id);
    assert_eq!(cost_lines[0]["amount_paise"], "1025");
    let report = serde_json::to_value(store.trial_balance(september()).unwrap()).unwrap();
    assert_eq!(report["debit_paise"], "1025");
    assert_eq!(report["credit_paise"], "1025");
    let edited = store.change_voucher(id, decode(json!({
        "expected_revision":1,"actor":"Reviewer","reason":"Fix amount","voucher":make_payment(2,"15.00")
    }))).unwrap();
    assert_eq!(serde_json::to_value(edited).unwrap()["revision"], 2);
    assert!(matches!(
        store.change_voucher(
            id,
            decode(json!({
                "expected_revision":1,"actor":"Reviewer","reason":"Stale action"
            }))
        ),
        Err(StoreError::Conflict(_))
    ));
    let cancelled = store
        .change_voucher(
            id,
            decode(json!({
                "expected_revision":2,"actor":"Reviewer","reason":"Duplicate expense"
            })),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(cancelled).unwrap()["status"],
        "cancelled"
    );
    let old = serde_json::to_value(store.history_revision(id, 1).unwrap()).unwrap();
    assert_eq!(old["postings"][0]["amount_paise"], "1025");
    assert_eq!(
        serde_json::to_value(store.trial_balance(september()).unwrap()).unwrap()["debit_paise"],
        "0"
    );

    let transfer = submit(
        &mut store,
        json!({
            "kind":"contra","request_id":key(3),"date":"2025-09-10","postings":[
                {"ledger_id":2,"amount":"31.50","side":"debit"},{"ledger_id":1,"amount":"31.50","side":"credit"}
            ]
        }),
    );
    assert_eq!(transfer["code"], "CON-000001");
    store
        .close_period(decode(
            json!({"through":"2025-09-30","actor":"Reviewer","reason":"Month end"}),
        ))
        .unwrap();
    assert!(matches!(
        store.add_voucher(decode(make_payment(4, "1"))),
        Err(StoreError::Conflict(_))
    ));
}

#[test]
fn bank_matching_checks_splits_duplicates_and_reversible_selection() {
    let mut store = temporary_book();
    let voucher = submit(
        &mut store,
        json!({
            "kind":"contra","request_id":key(1),"date":"2025-09-10","postings":[
                {"ledger_id":2,"amount":"10.25","side":"debit"},{"ledger_id":1,"amount":"10.25","side":"credit"}
            ]
        }),
    );
    let csv = "transaction_id,date,amount,reference,description\na,2025-09-10,3.25,A,Deposit\nb,2025-09-10,7.00,B,Deposit\n";
    let import = || decode(json!({"content":csv}));
    let first = serde_json::to_value(store.import_statement(2, import()).unwrap()).unwrap();
    assert_eq!(first["inserted"], 2);
    assert_eq!(
        serde_json::to_value(store.import_statement(2, import()).unwrap()).unwrap()["duplicates"],
        2
    );
    let review = serde_json::to_value(store.reconciliation(2, september()).unwrap()).unwrap();
    let posting = json!({"voucher_id":voucher["id"],"revision":1,"line_no":1});
    let match_request =
        |ids| decode(json!({"statement_ids":ids,"postings":[posting],"actor":"Reviewer"}));
    let a = &review["statement"][0]["id"];
    let b = &review["statement"][1]["id"];
    assert!(matches!(
        store.reconcile(2, match_request(json!([a]))),
        Err(StoreError::Invalid(_))
    ));
    let matched =
        serde_json::to_value(store.reconcile(2, match_request(json!([a, b]))).unwrap()).unwrap();
    assert_eq!(matched["signed_paise"], "1025");
    assert!(matches!(
        store.reconcile(2, match_request(json!([a, b]))),
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store.change_voucher(
            voucher["id"].as_i64().unwrap(),
            decode(json!({
                "expected_revision":1,"actor":"Reviewer","reason":"Reverse"
            }))
        ),
        Err(StoreError::Conflict(_))
    ));
    store
        .undo_reconciliation(
            matched["match_id"].as_i64().unwrap(),
            decode(json!({
                "actor":"Reviewer","reason":"Review deposit split"
            })),
        )
        .unwrap();
    assert!(store.reconcile(2, match_request(json!([a, b]))).is_ok());
}

#[test]
fn migration_preserves_legacy_voucher_codes_and_replays() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(include_str!(
        "../migrations/duckdb/0001_initial/1_legacy.sql"
    ))
    .unwrap();
    db.execute_batch("
        INSERT INTO ledgers (name,name_key,kind) VALUES ('Travel','travel','general');
        INSERT INTO vouchers (request_id,kind,number,voucher_date,cash_ledger_id,other_ledger_id,amount_paise,reference,narration)
        SELECT '29daeb83-d7be-4576-aca0-000000000001','payment',8,'2025-09-01',2,id,291,'Ref','Trip'
        FROM ledgers WHERE name = 'Travel';
        INSERT INTO journal_vouchers (request_id, number, voucher_date, amount_paise, reference, narration)
        VALUES ('29daeb83-d7be-4576-aca0-000000000002',4,'2025-09-02',130,'Day','Adjustment');
        INSERT INTO journal_postings (voucher_id,line_no,ledger_id,side,amount_paise) VALUES
        (2,1,1,'debit',130),(2,2,2,'credit',130);
    ").unwrap();
    let mut store = Store::initialize(db).unwrap();
    let payment = serde_json::to_value(store.voucher(1).unwrap()).unwrap();
    assert_eq!(payment["code"], "PAY-000008");
    assert_eq!(payment["postings"][0]["ledger"], "Travel");
    assert_eq!(payment["postings"][1]["side"], "credit");
    let previous = serde_json::to_value(store.voucher(2).unwrap()).unwrap();
    assert_eq!(previous["code"], "JRN-000004");
    assert_eq!(previous["postings"][0]["amount_paise"], "130");
    assert_eq!(previous["debit"], "");
    let replay = json!({"kind":"payment","request_id":key(1),"date":"2025-09-01",
        "reference":"Ref","narration":"Trip","amount":"2.91","cash_ledger_id":2,
        "other_ledger_id":payment["postings"][0]["ledger_id"]});
    assert!(!store.add_voucher(decode(replay)).unwrap().1);
}
