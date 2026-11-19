#!/usr/bin/env python3
"""Generate crates/ot-core/src/permissions_table.rs from Frappe Books DocType permissions.

Usage: python3 scripts/gen_frappe_permissions.py /path/to/frappe-books > crates/ot-core/src/permissions_table.rs

Reads every non-child DocType in frappe_books/frappe_books/doctype and keeps the
permission level 0 rules of the four staff roles. Portal roles (Customer, Supplier)
are left out.
"""

import json
import pathlib
import subprocess
import sys

ROLES = ["System Manager", "Books Manager", "Books User", "Books Cashier"]
PERMS = ["read", "write", "create", "delete", "submit", "cancel", "amend", "report", "import", "export", "print"]


def main(root: str) -> None:
    root_path = pathlib.Path(root)
    commit = subprocess.run(
        ["git", "-C", root, "rev-parse", "--short", "HEAD"], capture_output=True, text=True, check=False
    ).stdout.strip() or "unknown"
    rows = []
    for path in sorted((root_path / "frappe_books" / "frappe_books" / "doctype").glob("*/*.json")):
        if path.stem != path.parent.name:
            continue
        doc = json.loads(path.read_text())
        if doc.get("istable"):
            continue
        masks = []
        for role in ROLES:
            mask = 0
            for rule in doc.get("permissions", []):
                if rule.get("role") == role and not rule.get("permlevel"):
                    for bit, perm in enumerate(PERMS):
                        if rule.get(perm):
                            mask |= 1 << bit
            masks.append(mask)
        rows.append((doc["name"], bool(doc.get("issingle")), doc.get("is_submittable", 0) == 1, masks))

    out = sys.stdout
    out.write(f"// Generated from Frappe Books (AGPL-3.0) DocType permissions @{commit}\n")
    out.write("// by scripts/gen_frappe_permissions.py. Do not edit by hand.\n\n")
    out.write("/// (DocType, is single, is submittable, permission bits for\n")
    out.write(f"/// [{', '.join(ROLES)}]). Bits follow [`crate::auth::Perm`].\n")
    out.write("pub(crate) const DOCTYPE_PERMISSIONS: &[(&str, bool, bool, [u16; 4])] = &[\n")
    for name, single, submittable, masks in rows:
        bits = ", ".join(f"0x{m:03x}" for m in masks)
        out.write(f'    ("{name}", {str(single).lower()}, {str(submittable).lower()}, [{bits}]),\n')
    out.write("];\n")


if __name__ == "__main__":
    main(sys.argv[1])
