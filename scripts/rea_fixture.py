#!/usr/bin/env python3
"""Convert the supplied REA voucher model to OpenTally's private import format."""

import argparse
import json
import sqlite3
import sys
from pathlib import Path
from zipfile import ZipFile


def read_database(path):
    if path.suffix.casefold() == ".zip":
        with ZipFile(path) as archive:
            entry = archive.getinfo("voucher-data-model/accounting-voucher.sqlite")
            if entry.file_size > 16 * 1024 * 1024:
                raise ValueError("The investigation database exceeds the fixture size limit.")
            return archive.read(entry)
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError("The investigation database exceeds the fixture size limit.")
    return path.read_bytes()


def rupees(paise):
    return f"{paise // 100}.{paise % 100:02d}"


def convert(path, company, redact):
    with sqlite3.connect(":memory:") as db:
        db.deserialize(read_database(path))
        db.execute("PRAGMA trusted_schema = OFF")
        db.execute("PRAGMA query_only = ON")
        if db.execute("PRAGMA quick_check").fetchone()[0] != "ok":
            raise ValueError("The SQLite evidence did not pass its integrity check.")
        db.row_factory = sqlite3.Row
        groups = db.execute("SELECT id, name, parent_id FROM account_group ORDER BY id").fetchall()
        ledgers = db.execute("SELECT id, name, group_id FROM ledger ORDER BY id").fetchall()
        types = {
            row["voucher_id"]: row
            for row in db.execute("SELECT * FROM voucher_type_lookup")
        }
        group_names = {row["id"]: f"Group {i}" if redact else row["name"]
                       for i, row in enumerate(groups, 1)}
        ledger_names = {row["id"]: f"Ledger {i}" if redact else row["name"]
                        for i, row in enumerate(ledgers, 1)}
        vouchers = []
        for index, voucher in enumerate(db.execute("SELECT * FROM voucher ORDER BY id"), 1):
            source_type = types[voucher["id"]]
            postings = []
            for line_no, row in enumerate(db.execute(
                "SELECT * FROM voucher_postings WHERE voucher_id = ? ORDER BY line_number",
                (voucher["id"],),
            ), 1):
                debit, credit = row["debit_paise"], row["credit_paise"]
                if (
                    line_no != row["line_number"]
                    or (debit > 0) == (credit > 0)
                    or row["amount_paise"] != credit - debit
                    or row["is_deemed_positive"] != ("Yes" if debit else "No")
                ):
                    raise ValueError(f"Posting {line_no} has inconsistent source controls.")
                postings.append({
                    "ledger": ledger_names[row["ledger_id"]],
                    "side": "debit" if debit else "credit",
                    "amount": rupees(debit or credit),
                })
            date = voucher["voucher_date"]
            if len(date) == 8 and date.isdigit():
                date = f"{date[:4]}-{date[4:6]}-{date[6:]}"
            alteration = str(voucher["alter_id"] or "").strip()
            vouchers.append({
                "guid": f"redacted-rea-{index}" if redact else voucher["guid"],
                "source_revision": int(alteration) if alteration else None,
                "code": f"SNA-FIXTURE-{index}" if redact else voucher["voucher_number"],
                "type_name": source_type["voucher_type_name"],
                "base_type": source_type["parent_type"],
                "series": source_type["series_name"] or "",
                "date": date,
                "narration": "" if redact else (voucher["narration"] or ""),
                "postings": postings,
            })
        return {
            "company": company,
            "currency": "INR",
            "groups": [{"name": group_names[row["id"]], "parent": group_names.get(row["parent_id"])}
                       for row in groups],
            "ledgers": [{"name": ledger_names[row["id"]], "group": group_names[row["group_id"]]}
                        for row in ledgers],
            "voucher_types": [
                {"name": row["name"], "parent": row["parent_type"] or ""}
                for row in db.execute("SELECT name, parent_type FROM voucher_type_master ORDER BY name")
            ],
            "vouchers": vouchers,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path, help="The supplied REA ZIP or accounting-voucher.sqlite")
    parser.add_argument("--company", required=True, help="Source company identifier or private pilot alias")
    parser.add_argument("--redact", action="store_true", help="Replace names, narration and voucher identifiers")
    args = parser.parse_args()
    try:
        data = convert(args.fixture, args.company, args.redact)
    except (OSError, ValueError, KeyError, sqlite3.Error) as error:
        parser.exit(1, f"Could not convert fixture. {error}\n")
    json.dump(data, sys.stdout, ensure_ascii=True, indent=2)
    print()


if __name__ == "__main__":
    main()
