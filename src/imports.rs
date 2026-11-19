use std::collections::{HashMap, HashSet};

use duckdb::{OptionalExt, Transaction, params};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};

use crate::store::{
    Allocation, Audit, LedgerKind, Posting, PostingSide, Revision, Store, StoreError, ensure_open,
    ensure_unmatched, insert_ledger, parse_amount, parse_signed_amount, read_revision,
    required_text, save_revision, validate_postings, validated_date, validated_text,
};

#[derive(Deserialize)]
pub struct ImportRequest {
    format: String,
    content: String,
    company: Option<String>,
    opening_date: Option<String>,
    #[serde(default)]
    commit: bool,
}

#[derive(Default, Serialize)]
pub struct ImportSummary {
    pub(crate) fingerprint: String,
    pub(crate) vouchers: usize,
    pub(crate) postings: usize,
    pub(crate) debit_paise: String,
    pub(crate) credit_paise: String,
    pub(crate) groups: usize,
    pub(crate) ledgers: usize,
    pub(crate) inserted: usize,
    pub(crate) revised: usize,
    pub(crate) duplicates: usize,
    pub(crate) warnings: Vec<String>,
    pub(crate) committed: bool,
}

#[derive(Deserialize)]
struct SourceBook {
    company: String,
    currency: Option<String>,
    opening_date: Option<String>,
    #[serde(default)]
    groups: Vec<SourceGroup>,
    #[serde(default)]
    ledgers: Vec<SourceLedger>,
    #[serde(default)]
    voucher_types: Vec<SourceVoucherType>,
    #[serde(default)]
    vouchers: Vec<SourceVoucher>,
}

#[derive(Deserialize)]
struct SourceVoucherType {
    name: String,
    parent: String,
}

#[derive(Deserialize)]
struct SourceGroup {
    name: String,
    parent: Option<String>,
}

#[derive(Deserialize)]
struct SourceLedger {
    name: String,
    group: Option<String>,
    kind: Option<LedgerKind>,
    opening: Option<String>,
}

#[derive(Deserialize)]
struct SourceVoucher {
    guid: String,
    source_revision: Option<i64>,
    code: String,
    type_name: String,
    base_type: Option<String>,
    date: String,
    #[serde(default)]
    series: String,
    #[serde(default = "posted")]
    status: String,
    #[serde(default)]
    reference: String,
    #[serde(default)]
    narration: String,
    #[serde(default)]
    postings: Vec<SourcePosting>,
}

fn posted() -> String {
    "posted".into()
}

#[derive(Deserialize)]
struct SourcePosting {
    ledger: String,
    amount: String,
    side: PostingSide,
    #[serde(default)]
    narration: String,
    #[serde(default)]
    allocations: Vec<SourceAllocation>,
}

#[derive(Deserialize)]
struct SourceAllocation {
    category: String,
    centre: String,
    amount: String,
}

impl Store {
    pub fn import(&mut self, request: ImportRequest) -> Result<ImportSummary, StoreError> {
        let mut warnings = Vec::new();
        let source: SourceBook = match request.format.as_str() {
            "json" => serde_json::from_str(&request.content).map_err(|e| {
                StoreError::Invalid(format!("Invalid book JSON at line {}. {e}", e.line()))
            })?,
            "xml" => read_tally_xml(&request, &mut warnings)?,
            _ => {
                return Err(StoreError::Invalid(
                    "Choose Tally XML or OpenTally JSON.".into(),
                ));
            }
        };
        let company = required_text(&source.company, "Source company", 150)?;
        if source
            .currency
            .as_deref()
            .is_some_and(|currency| !is_inr(currency))
        {
            return Err(StoreError::Invalid("This book accepts INR amounts.".into()));
        }
        let opening_date = source
            .opening_date
            .as_deref()
            .map(validated_date)
            .transpose()?;
        if source.groups.is_empty()
            && source.ledgers.is_empty()
            && source.voucher_types.is_empty()
            && source.vouchers.is_empty()
        {
            return Err(StoreError::Invalid(
                "The file contains no supported masters or vouchers.".into(),
            ));
        }
        if opening_date.is_none() && source.ledgers.iter().any(|l| l.opening.is_some()) {
            warnings.push("Opening balances require a confirmed effective date. This import will leave them unresolved.".into());
        }
        let tx = self.connection.transaction()?;
        let bound_company: Option<String> = tx.query_row(
            "SELECT source_company FROM book_settings WHERE id = 1",
            [],
            |r| r.get(0),
        )?;
        if bound_company
            .as_deref()
            .is_some_and(|name| !name.eq_ignore_ascii_case(company))
        {
            return Err(StoreError::Conflict(
                "This book is already linked to a different source company.".into(),
            ));
        }
        let fingerprint: String = tx.query_row(
            "SELECT sha256(?)",
            [format!(
                "{}\n{}\n{}\n{}",
                request.format,
                company,
                opening_date.unwrap_or(""),
                request.content
            )],
            |r| r.get(0),
        )?;
        let groups = import_groups(&tx, &source.groups)?;
        let ledgers = import_ledgers(&tx, &groups, &source.ledgers, opening_date)?;
        let types = import_voucher_types(&tx, &source.voucher_types)?;
        let mut summary = ImportSummary {
            fingerprint,
            groups: source.groups.len(),
            ledgers: source.ledgers.len(),
            vouchers: source.vouchers.len(),
            warnings,
            ..Default::default()
        };
        let mut total = 0i128;
        let mut identities = HashSet::new();
        for mut voucher in source.vouchers {
            let guid = required_text(&voucher.guid, "Source voucher GUID", 200)?.to_lowercase();
            if !identities.insert(guid.clone()) {
                return Err(StoreError::Invalid(format!(
                    "Duplicate source identity in this file for voucher {}.",
                    voucher.code
                )));
            }
            let derived = resolve_voucher_type(&types, &voucher.type_name)?;
            if let Some(base) = derived {
                if voucher
                    .base_type
                    .as_deref()
                    .is_some_and(|explicit| !explicit.eq_ignore_ascii_case(base))
                {
                    return Err(StoreError::Invalid(format!(
                        "Voucher {} has a base type that conflicts with its master.",
                        voucher.code
                    )));
                }
                voucher.base_type = Some(base.into());
            }
            let (kind, revision) = prepare_source(&ledgers, voucher)?;
            let amount = validate_postings(&revision.postings, &revision.status)
                .map_err(|e| with_voucher_context(e, &revision.code))?;
            if revision.status == "posted" {
                total += amount;
            }
            summary.postings += revision.postings.len();
            import_voucher(&tx, &guid, kind, &revision, &mut summary)?;
        }
        summary.debit_paise = total.to_string();
        summary.credit_paise = total.to_string();
        if request.commit {
            tx.execute("UPDATE book_settings SET source_company = COALESCE(source_company, ?) WHERE id = 1",
                [company])?;
            tx.execute(
                "INSERT INTO import_batches (fingerprint, source_company, format, payload, inserted, revised, duplicates)
                 VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
                params![summary.fingerprint, company, request.format, request.content, summary.inserted as i64,
                        summary.revised as i64, summary.duplicates as i64],
            )?;
            tx.commit()?;
            summary.committed = true;
        } else {
            tx.rollback()?;
        }
        Ok(summary)
    }
}

struct Group {
    id: i64,
    parent: Option<i64>,
    name: String,
}

fn import_voucher_types(
    tx: &Transaction<'_>,
    masters: &[SourceVoucherType],
) -> Result<HashMap<String, String>, StoreError> {
    let mut query = tx.prepare("SELECT name_key, parent FROM source_voucher_types")?;
    let mut parents: HashMap<String, String> = query
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<duckdb::Result<_>>()?;
    for master in masters {
        let name = required_text(&master.name, "Voucher type name", 100)?;
        let parent = validated_text(&master.parent, "Parent voucher type", 100)?.to_lowercase();
        let key = name.to_lowercase();
        if let Some(existing) = parents.get(&key) {
            if *existing != parent {
                return Err(StoreError::Conflict(format!(
                    "Voucher type {name} has a different parent in this book."
                )));
            }
        } else {
            tx.execute(
                "INSERT INTO source_voucher_types VALUES (?, ?, ?)",
                params![key, name, parent],
            )?;
            parents.insert(key, parent);
        }
    }
    for name in parents.keys() {
        resolve_voucher_type(&parents, name)?;
    }
    Ok(parents)
}

fn resolve_voucher_type(
    parents: &HashMap<String, String>,
    name: &str,
) -> Result<Option<&'static str>, StoreError> {
    let key = name.trim().to_lowercase();
    let mut current = key.as_str();
    let mut visited = HashSet::new();
    loop {
        match current {
            "payment" => return Ok(Some("payment")),
            "receipt" => return Ok(Some("receipt")),
            "journal" => return Ok(Some("journal")),
            "contra" => return Ok(Some("contra")),
            _ => {}
        }
        if !visited.insert(current) {
            return Err(StoreError::Invalid(format!(
                "Voucher type {name} has a parent cycle."
            )));
        }
        let Some(parent) = parents.get(current) else {
            return Ok(None);
        };
        current = parent;
    }
}

type Groups = HashMap<String, Group>;

fn import_groups(tx: &Transaction<'_>, entries: &[SourceGroup]) -> Result<Groups, StoreError> {
    let mut stmt = tx.prepare("SELECT id, name, name_key, parent_id FROM account_groups")?;
    let mut groups: Groups = stmt
        .query_map([], |r| {
            Ok((
                r.get(2)?,
                Group {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    parent: r.get(3)?,
                },
            ))
        })?
        .collect::<duckdb::Result<_>>()?;
    let mut seen = HashSet::new();
    let mut pending = Vec::new();
    for item in entries {
        let name = required_text(&item.name, "Group name", 100)?;
        let key = name.to_lowercase();
        if !seen.insert(key.clone()) {
            return Err(StoreError::Invalid(format!("Duplicate group {name}.")));
        }
        let parent = item
            .parent
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("Primary"))
            .map(str::to_lowercase);
        pending.push((key, name, parent));
    }

    while !pending.is_empty() {
        let count = pending.len();
        let mut unresolved = Vec::new();
        for (key, name, parent) in pending {
            let parent_id = if let Some(parent_name) = &parent {
                let Some(parent_group) = groups.get(parent_name) else {
                    unresolved.push((key, name, parent));
                    continue;
                };
                Some(parent_group.id)
            } else {
                None
            };
            if let Some(existing) = groups.get(&key) {
                if existing.parent != parent_id {
                    return Err(StoreError::Conflict(format!(
                        "The existing group {name} has a different parent."
                    )));
                }
                continue;
            }
            let id = tx.query_row(
                "INSERT INTO account_groups (name, name_key, parent_id) VALUES (?, ?, ?) RETURNING id",
                params![name, key, parent_id], |r| r.get(0),
            )?;
            groups.insert(
                key,
                Group {
                    id,
                    name: name.into(),
                    parent: parent_id,
                },
            );
        }
        if unresolved.len() == count {
            return Err(StoreError::Invalid(format!(
                "Group {} has a missing parent or a parent cycle.",
                unresolved[0].1
            )));
        }
        pending = unresolved;
    }
    Ok(groups)
}

type Ledgers = HashMap<String, (i64, String, Option<i64>)>;

fn import_ledgers(
    tx: &Transaction<'_>,
    groups: &Groups,
    entries: &[SourceLedger],
    opening_date: Option<&str>,
) -> Result<Ledgers, StoreError> {
    let mut stmt = tx.prepare(
        "SELECT l.name_key, l.id, l.kind, g.group_id FROM ledgers l LEFT JOIN ledger_groups g ON l.id = g.ledger_id"
    )?;
    let mut ledgers: Ledgers = stmt
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
        .collect::<duckdb::Result<_>>()?;
    let by_id = groups
        .values()
        .map(|g| (g.id, g))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    for item in entries {
        let name = required_text(&item.name, "Ledger name", 100)?;
        let key = name.to_lowercase();
        if !seen.insert(key.clone()) {
            return Err(StoreError::Invalid(format!("Duplicate ledger {name}.")));
        }
        let group = item
            .group
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|group| {
                groups.get(&group.to_lowercase()).ok_or_else(|| {
                    StoreError::Invalid(format!("Ledger {name} has an unresolved group."))
                })
            })
            .transpose()?;
        let kind = item
            .kind
            .map(LedgerKind::as_str)
            .unwrap_or_else(|| infer_ledger_kind(name, group, &by_id));
        let group_id = group.map(|g| g.id);
        let (id, existing_group) = if let Some((id, saved_kind, saved_group)) = ledgers.get(&key) {
            if saved_kind != kind
                || (saved_group.is_some() && group_id.is_some() && *saved_group != group_id)
            {
                return Err(StoreError::Conflict(format!(
                    "Ledger {name} has a different kind or group in this book."
                )));
            }
            (*id, *saved_group)
        } else {
            (insert_ledger(tx, name, kind)?, None)
        };
        if let (None, Some(group_id)) = (existing_group, group_id) {
            tx.execute(
                "INSERT INTO ledger_groups VALUES (?, ?)",
                params![id, group_id],
            )?;
        }
        ledgers.insert(key, (id, kind.into(), group_id.or(existing_group)));
        if let (Some(date), Some(amount)) = (opening_date, &item.opening) {
            let value = parse_signed_amount(amount)?;
            let existing: Option<i64> = tx.query_row(
                "SELECT signed_paise FROM ledger_openings WHERE ledger_id = ? AND opening_date = ?::DATE",
                params![id, date], |r| r.get(0),
            ).optional()?;
            match existing {
                Some(saved) if saved == value => {}
                Some(_) => {
                    return Err(StoreError::Conflict(format!(
                        "Ledger {name} already has a different opening on {date}."
                    )));
                }
                None => {
                    ensure_open(tx, date)?;
                    tx.execute(
                        "INSERT INTO ledger_openings VALUES (?, ?::DATE, ?, 'Imported opening')",
                        params![id, date, value],
                    )?;
                }
            }
        }
    }
    Ok(ledgers)
}

fn infer_ledger_kind(
    name: &str,
    group: Option<&Group>,
    groups: &HashMap<i64, &Group>,
) -> &'static str {
    let mut current = group;
    while let Some(group) = current {
        match group.name.to_lowercase().as_str() {
            "cash-in-hand" => return "cash",
            "bank accounts" | "bank od a/c" | "bank occ a/c" => return "bank",
            _ => current = group.parent.and_then(|id| groups.get(&id).copied()),
        }
    }
    match name.to_lowercase().as_str() {
        "cash" => "cash",
        "bank" => "bank",
        _ => "general",
    }
}

fn prepare_source(
    ledgers: &Ledgers,
    item: SourceVoucher,
) -> Result<(&'static str, Revision), StoreError> {
    let code = required_text(&item.code, "Source voucher number", 100)?;
    let type_name = required_text(&item.type_name, "Source voucher type", 100)?;
    let kind = match item
        .base_type
        .as_deref()
        .unwrap_or(type_name)
        .to_lowercase()
        .as_str()
    {
        "payment" => "payment",
        "receipt" => "receipt",
        "journal" => "journal",
        "contra" => "contra",
        _ => "other",
    };
    if !matches!(item.status.as_str(), "posted" | "cancelled" | "optional") {
        return Err(StoreError::Invalid(format!(
            "Voucher {code} has an unsupported status."
        )));
    }
    let mut liquid = HashSet::new();
    let postings = item
        .postings
        .into_iter()
        .map(|p| {
            let (ledger_id, ledger_kind, _) = ledgers
                .get(&p.ledger.trim().to_lowercase())
                .ok_or_else(|| {
                    StoreError::Invalid(format!(
                        "Voucher {code} refers to unresolved ledger {}.",
                        p.ledger
                    ))
                })?;
            if matches!(ledger_kind.as_str(), "cash" | "bank")
                && matches!(
                    (kind, p.side),
                    ("payment", PostingSide::Credit) | ("receipt", PostingSide::Debit)
                )
            {
                liquid.insert(*ledger_id);
            }
            let allocations = p
                .allocations
                .into_iter()
                .map(|a| {
                    Ok(Allocation {
                        category: required_text(&a.category, "Cost category", 100)?.into(),
                        centre: required_text(&a.centre, "Cost centre", 100)?.into(),
                        amount: parse_amount(&a.amount)?,
                    })
                })
                .collect::<Result<Vec<_>, StoreError>>()?;
            Ok(Posting {
                ledger_id: *ledger_id,
                side: p.side,
                amount: parse_amount(&p.amount)?,
                narration: validated_text(&p.narration, "Posting narration", 1000)?.into(),
                allocations,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    let cash_ledger_id = if liquid.len() == 1 {
        liquid.into_iter().next()
    } else {
        None
    };
    Ok((
        kind,
        Revision {
            code: code.into(),
            type_name: type_name.into(),
            series: validated_text(&item.series, "Source series", 100)?.into(),
            date: validated_date(&item.date)?.into(),
            status: item.status,
            cash_ledger_id,
            reference: validated_text(&item.reference, "Source reference", 250)?.into(),
            narration: validated_text(&item.narration, "Source narration", 5000)?.into(),
            source_revision: item.source_revision,
            postings,
        },
    ))
}

fn import_voucher(
    tx: &Transaction<'_>,
    guid: &str,
    kind: &str,
    doc: &Revision,
    summary: &mut ImportSummary,
) -> Result<(), StoreError> {
    let existing: Option<(i64, i32, String)> = tx
        .query_row(
            "SELECT id, revision, kind FROM current_vouchers WHERE source_guid = ?",
            [guid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (id, version, action) = if let Some((id, version, saved_kind)) = existing {
        let saved = read_revision(tx, id, version)?;
        if saved_kind == kind && saved == *doc {
            summary.duplicates += 1;
            return Ok(());
        }
        if saved_kind != kind
            || !matches!((saved.source_revision, doc.source_revision), (Some(a), Some(b)) if b > a)
        {
            return Err(StoreError::Conflict(format!(
                "Voucher {} has a conflicting or stale source revision.",
                doc.code
            )));
        }
        ensure_open(tx, &saved.date)?;
        ensure_unmatched(tx, id, version)?;
        summary.revised += 1;
        (id, version + 1, "reimported")
    } else {
        let id = tx.query_row(
            "INSERT INTO book_vouchers (kind, source_guid) VALUES (?, ?) RETURNING id",
            params![kind, guid],
            |r| r.get(0),
        )?;
        summary.inserted += 1;
        (id, 1, "imported")
    };
    ensure_open(tx, &doc.date)?;
    save_revision(
        tx,
        id,
        version,
        None,
        doc,
        Audit {
            actor: "Source import",
            action,
            reason: "Recorded from the supplied export",
        },
    )
}

fn with_voucher_context(error: StoreError, code: &str) -> StoreError {
    match error {
        StoreError::Invalid(message) => StoreError::Invalid(format!("Voucher {code}. {message}")),
        other => other,
    }
}

fn read_tally_xml(
    request: &ImportRequest,
    warnings: &mut Vec<String>,
) -> Result<SourceBook, StoreError> {
    let xml = Document::parse(request.content.trim_start_matches('\u{feff}'))
        .map_err(|e| StoreError::Invalid(format!("Invalid XML. {e}")))?;
    let source_company = xml
        .descendants()
        .find(|n| tag(*n, "SVCURRENTCOMPANY"))
        .and_then(|node| node.text())
        .map(str::trim);
    let company = request
        .company
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or(source_company)
        .ok_or_else(|| {
            StoreError::Invalid("Enter the source company name for this XML export.".into())
        })?;
    if xml
        .descendants()
        .filter(|n| tag(*n, "SVCURRENTCOMPANY"))
        .filter_map(|n| n.text())
        .any(|name| !name.trim().is_empty() && !name.trim().eq_ignore_ascii_case(company))
    {
        return Err(StoreError::Invalid(
            "The company in the XML differs from the selected source company.".into(),
        ));
    }
    let mut source = SourceBook {
        company: company.into(),
        currency: xml
            .descendants()
            .find(|n| tag(*n, "BASECURRENCYNAME") || tag(*n, "SVCURRENCYNAME"))
            .and_then(|n| n.text())
            .map(str::to_owned),
        opening_date: request.opening_date.clone(),
        groups: vec![],
        ledgers: vec![],
        voucher_types: vec![],
        vouchers: vec![],
    };
    let entries = xml
        .descendants()
        .filter(|n| tag(*n, "TALLYMESSAGE"))
        .flat_map(|n| n.children())
        .filter(Node::is_element);
    let mut vouchers = Vec::new();
    let mut unsupported = HashSet::new();
    for node in entries {
        let name = || {
            node.attribute("NAME")
                .unwrap_or_else(|| field(node, "NAME"))
        };
        match node.tag_name().name() {
            "GROUP" => source.groups.push(SourceGroup {
                name: name().into(),
                parent: optional_field(node, "PARENT").map(str::to_owned),
            }),
            "LEDGER" => {
                let opening = optional_field(node, "OPENINGBALANCE")
                    .map(|text| parse_signed_amount(text).map(|value| to_rupees(-value)))
                    .transpose()?;
                source.ledgers.push(SourceLedger {
                    name: name().into(),
                    group: optional_field(node, "PARENT").map(str::to_owned),
                    kind: None,
                    opening,
                });
            }
            "VOUCHERTYPE" => source.voucher_types.push(SourceVoucherType {
                name: name().into(),
                parent: field(node, "PARENT").into(),
            }),
            "VOUCHER" => vouchers.push(node),
            "CURRENCY" => {}
            other => {
                unsupported.insert(other);
            }
        }
    }
    for node in vouchers {
        let voucher = read_xml_voucher(node, warnings)?;
        source.vouchers.push(voucher);
    }
    let mut unsupported = unsupported.into_iter().collect::<Vec<_>>();
    unsupported.sort_unstable();
    if !unsupported.is_empty() {
        warnings.push(format!(
            "Other XML objects are kept with the source file. {}",
            unsupported.join(", ")
        ));
    }
    Ok(source)
}

fn read_xml_voucher(
    node: Node<'_, '_>,
    warnings: &mut Vec<String>,
) -> Result<SourceVoucher, StoreError> {
    let code = field(node, "VOUCHERNUMBER");
    let date = field(node, "DATE");
    let date = if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
    } else {
        date.to_owned()
    };
    let type_name = optional_field(node, "VOUCHERTYPENAME")
        .or(node.attribute("VCHTYPE"))
        .unwrap_or_default();
    let status = if yes(node, "ISCANCELLED")
        || yes(node, "ISDELETED")
        || node
            .attribute("ACTION")
            .is_some_and(|v| v.eq_ignore_ascii_case("Delete"))
    {
        "cancelled"
    } else if yes(node, "ISOPTIONAL") {
        "optional"
    } else {
        "posted"
    };
    let mut postings = Vec::new();
    for line in node
        .children()
        .filter(|n| tag(*n, "ALLLEDGERENTRIES.LIST") || tag(*n, "LEDGERENTRIES.LIST"))
    {
        let amount = parse_signed_amount(field(line, "AMOUNT"))
            .map_err(|e| with_voucher_context(e, code))?;
        let side = if amount < 0 {
            PostingSide::Debit
        } else {
            PostingSide::Credit
        };
        if amount == 0
            || (optional_field(line, "ISDEEMEDPOSITIVE").is_some()
                && yes(line, "ISDEEMEDPOSITIVE") != (side == PostingSide::Debit))
        {
            return Err(StoreError::Invalid(format!(
                "Voucher {code} has a zero posting or a conflicting source sign."
            )));
        }
        let mut allocations = Vec::new();
        for category in line
            .children()
            .filter(|n| tag(*n, "CATEGORYALLOCATIONS.LIST"))
        {
            for centre in category
                .children()
                .filter(|n| tag(*n, "COSTCENTREALLOCATIONS.LIST"))
            {
                let assigned = parse_signed_amount(field(centre, "AMOUNT"))?;
                if assigned.signum() != amount.signum() {
                    return Err(StoreError::Invalid(format!(
                        "Voucher {code} has a cost allocation with a different side."
                    )));
                }
                allocations.push(SourceAllocation {
                    category: field(category, "CATEGORY").into(),
                    centre: field(centre, "NAME").into(),
                    amount: to_rupees(assigned.abs()),
                });
            }
        }
        postings.push(SourcePosting {
            ledger: field(line, "LEDGERNAME").into(),
            side,
            amount: to_rupees(amount.abs()),
            narration: field(line, "NARRATION").into(),
            allocations,
        });
    }
    if node.descendants().any(|n| {
        n.is_element()
            && matches!(
                n.tag_name().name(),
                "BILLALLOCATIONS.LIST"
                    | "BANKALLOCATIONS.LIST"
                    | "ALLINVENTORYENTRIES.LIST"
                    | "INVENTORYALLOCATIONS.LIST"
            )
            && n.children().any(|child| child.is_element())
    }) {
        let warning = "Populated bill, bank, or stock details are retained in the source XML. Accounting reports use the voucher’s ledger lines.".to_owned();
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    let source_revision = optional_field(node, "ALTERID")
        .map(str::parse)
        .transpose()
        .map_err(|_| {
            StoreError::Invalid(format!("Voucher {code} has an invalid alteration ID."))
        })?;
    Ok(SourceVoucher {
        guid: field(node, "GUID").into(),
        source_revision,
        code: code.into(),
        type_name: type_name.into(),
        base_type: None,
        date,
        series: field(node, "VOUCHERNUMBERSERIES").into(),
        status: status.into(),
        reference: field(node, "REFERENCE").into(),
        narration: field(node, "NARRATION").into(),
        postings,
    })
}

fn tag(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}

fn field<'a>(node: Node<'a, '_>, name: &str) -> &'a str {
    node.children()
        .find(|n| tag(*n, name))
        .and_then(|n| n.text())
        .unwrap_or_default()
        .trim()
}

fn optional_field<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    let value = field(node, name);
    (!value.is_empty()).then_some(value)
}

fn yes(node: Node<'_, '_>, name: &str) -> bool {
    field(node, name).eq_ignore_ascii_case("Yes")
}

fn is_inr(currency: &str) -> bool {
    matches!(
        currency.trim().to_lowercase().as_str(),
        "inr" | "₹" | "rs" | "rs." | "indian rupee" | "indian rupees"
    )
}

fn to_rupees(paise: i64) -> String {
    let absolute = paise.unsigned_abs();
    format!(
        "{}{}.{:02}",
        if paise < 0 { "-" } else { "" },
        absolute / 100,
        absolute % 100
    )
}
