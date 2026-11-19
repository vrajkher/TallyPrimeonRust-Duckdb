"use strict";

const reportsForm = byId("reports-form");
const bankReviewForm = byId("bank-review-form");
const importForm = byId("import-form");
const commitImportButton = byId("commit-import");
const reportResults = byId("report-results");
let stagedImport;
let importRequest = 0;
let reviewedLedger;
let selectedMatch;
let reportRequest = 0;
let bankRequest = 0;

function renderTable(target, headings, rows) {
  const table = element("table", undefined, "book-table");
  const header = table.createTHead().insertRow();
  for (const heading of headings) {
    const th = element("th", heading);
    th.scope = "col";
    header.append(th);
  }
  const body = table.createTBody();
  for (const values of rows) {
    const row = body.insertRow();
    for (const value of values) {
      const cell = row.insertCell();
      if (value instanceof Node) cell.append(value);
      else cell.textContent = value ?? "—";
    }
  }
  if (!rows.length) {
    const cell = body.insertRow().insertCell();
    cell.colSpan = headings.length;
    cell.textContent = "No results for this period.";
  }
  target.replaceChildren(table);
}

function signedBalance(paise) {
  if (paise === null) return "Unconfirmed";
  const value = BigInt(paise);
  if (value === 0n) return money(0n);
  return money(value < 0n ? -value : value) + (value < 0n ? " Cr" : " Dr");
}

function formValues(form) {
  return Object.fromEntries(new FormData(form));
}

function periodQuery(values) {
  return new URLSearchParams({ from: values.from, to: values.to });
}

function listenForm(id, errorTarget, action) {
  const form = byId(id);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const button = event.submitter;
    if (button) button.disabled = true;
    status(errorTarget);
    try {
      await action(formValues(form), form);
    } catch (error) {
      status(errorTarget, error.message, true);
    } finally {
      if (button) button.disabled = false;
    }
  });
}

function syncLedgerChoices() {
  for (const select of document.querySelectorAll("[data-ledger-choices]")) {
    const onlyBank = select.dataset.ledgerChoices === "bank";
    const choices = onlyBank ? ledgers.filter((ledger) => ledger.kind === "bank") : ledgers;
    setLedgerChoices(select, choices, select.value, onlyBank ? "Choose a bank ledger" : "Choose a ledger");
  }
}
window.addEventListener("ledgers-updated", syncLedgerChoices);
syncLedgerChoices();

async function loadGroups() {
  const groups = await api("/api/groups");
  for (const select of document.querySelectorAll("[data-group-choices]")) {
    const selected = select.value;
    setLedgerChoices(select, groups, selected, select.dataset.placeholder);
  }
  const groupList = byId("account-groups");
  const names = new Map(groups.map((group) => [group.id, group.name]));
  groupList.replaceChildren(...groups.map((group) => {
    const row = element("li", group.name);
    row.append(element("span", names.get(group.parent_id) ?? "Root"));
    return row;
  }));
}

function showSettings(settings) {
  byId("book-name").textContent = settings.company_name;
  byId("company-form").elements.name.value = settings.company_name;
  byId("closed-period").textContent = settings.closed_through
    ? "Closed through " + formatDate(settings.closed_through)
    : "All dates are open.";
}

listenForm("company-form", "company-status", async (values) => {
  showSettings(await api("/api/settings", values));
  status("company-status", "Company name saved.");
});

listenForm("close-period-form", "period-status", async (values) => {
  if (!window.confirm("Close through " + formatDate(values.through) + "? Posting and corrections through this date will be blocked.")) return;
  showSettings(await api("/api/settings/close-period", values));
  status("period-status", "Accounting period closed.");
});

listenForm("group-form", "masters-status", async (values, form) => {
  await api("/api/groups", { name: values.name, parent_id: Number(values.parent_id) || null });
  form.reset();
  await loadGroups();
  status("masters-status", "Group added.");
});

listenForm("opening-form", "masters-status", async ({ ledger, ...opening }, form) => {
  if (!ledger) throw new Error("Choose a ledger.");
  await api("/api/ledgers/" + ledger + "/openings", opening);
  form.elements.amount.value = "";
  status("masters-status", "Opening balance recorded.");
});

function updateReportKind() {
  const show = reportsForm.elements.report.value === "ledgers";
  byId("report-ledger-field").hidden = !show;
  reportsForm.elements.ledger.required = show;
}

listenForm("reports-form", "report-status", async (values) => {
  const request = ++reportRequest;
  reportResults.replaceChildren();
  if (values.report === "ledgers" && !values.ledger) throw new Error("Choose a ledger.");
  const suffix = values.report === "ledgers" ? "ledgers/" + values.ledger : values.report;
  const report = await api("/api/reports/" + suffix + "?" + periodQuery(values));
  if (request !== reportRequest) return;
  if (values.report === "trial-balance") {
    const rows = report.rows.map((row) => {
      const ledger = element("button", row.ledger, "voucher-link");
      ledger.type = "button";
      ledger.addEventListener("click", () => {
        reportsForm.elements.report.value = "ledgers";
        reportsForm.elements.ledger.value = row.ledger_id;
        updateReportKind();
        reportsForm.requestSubmit();
      });
      return [ledger, row.group, signedBalance(row.opening_paise), money(row.debit_paise),
        money(row.credit_paise), signedBalance(row.closing_paise)];
    });
    renderTable(reportResults, ["Ledger", "Group", "Opening", "Debit movement", "Credit movement", "Closing"], rows);
    const movement = "Movement totals " + money(report.debit_paise) + " Dr · " + money(report.credit_paise) + " Cr.";
    const closing = report.missing_openings
      ? " Opening balances are unconfirmed for " + report.missing_openings + " ledgers."
      : " Closing totals " + money(report.closing_debit_paise) + " Dr · " + money(report.closing_credit_paise) + " Cr."
         + (BigInt(report.opening_difference_paise) !== 0n ? " Opening difference " + signedBalance(report.opening_difference_paise) + "." : "");
    status("report-status", movement + closing);
  } else if (values.report === "ledgers") {
    const totals = report.totals;
    renderTable(reportResults, ["Voucher", "Date", "Line", "Debit", "Credit", "Narration"], report.rows.map((row) => [
      voucherButton(row.voucher_id, row.code), formatDate(row.date), row.line_no,
      row.side === "debit" ? money(row.amount_paise) : "—",
      row.side === "credit" ? money(row.amount_paise) : "—", row.narration,
    ]));
    status("report-status", totals.ledger + " · Opening " + signedBalance(totals.opening_paise)
      + " · Closing " + signedBalance(totals.closing_paise));
  } else {
    const detail = element("div", undefined, "book-section");
    detail.hidden = true;
    renderTable(reportResults, ["Category", "Cost centre", "Allocated debit", "Allocated credit"], report.map((row) => {
      const button = element("button", row.centre, "voucher-link");
      button.type = "button";
      button.addEventListener("click", async () => {
        button.disabled = true;
        try {
          const query = periodQuery(values);
          query.set("category", row.category);
          query.set("centre", row.centre);
          const lines = await api("/api/reports/cost-centres/postings?" + query);
          const table = element("div", undefined, "table-scroll");
          renderTable(table, ["Voucher", "Date", "Line", "Allocated debit", "Allocated credit"], lines.map((line) => [
            voucherButton(line.voucher_id, line.code), formatDate(line.date), line.line_no,
            line.side === "debit" ? money(line.amount_paise) : "—",
            line.side === "credit" ? money(line.amount_paise) : "—",
          ]));
          detail.replaceChildren(element("h3", row.category + " / " + row.centre), table);
          detail.hidden = false;
        } catch (error) { status("report-status", error.message, true); }
        finally { button.disabled = false; }
      });
      return [row.category, button, money(row.debit_paise), money(row.credit_paise)];
    }));
    reportResults.append(detail);
  }
});
reportsForm.elements.report.addEventListener("change", updateReportKind);

async function fileText(file) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const encoding = bytes[0] === 0xff && bytes[1] === 0xfe ? "utf-16le"
    : bytes[0] === 0xfe && bytes[1] === 0xff ? "utf-16be" : "utf-8";
  return new TextDecoder(encoding, { fatal: true }).decode(bytes);
}

function showImportSummary(result) {
  status("import-status", (result.committed ? "Imported. " : "Preview. ")
    + result.groups + " groups, " + result.ledgers + " ledger masters, " + result.vouchers
    + " vouchers and " + result.postings + " postings in the file. Posted debits "
    + money(result.debit_paise) + " · credits " + money(result.credit_paise)
    + ". New " + result.inserted + " · revised " + result.revised + " · duplicates " + result.duplicates + ".");
  const warnings = byId("import-warnings");
  warnings.replaceChildren(...result.warnings.map((warning) => element("li", warning)));
  warnings.hidden = result.warnings.length === 0;
}

listenForm("import-form", "import-status", async (values) => {
  const request = ++importRequest;
  stagedImport = undefined;
  commitImportButton.disabled = true;
  const file = values.file;
  const format = file.name.toLowerCase().endsWith(".xml") ? "xml" : "json";
  const payload = {
    format, content: await fileText(file),
    company: values.company || null, opening_date: values.opening_date || null,
  };
  const preview = await api("/api/import", payload);
  if (request !== importRequest) return;
  stagedImport = payload;
  commitImportButton.disabled = false;
  showImportSummary(preview);
});

importForm.addEventListener("input", () => {
  importRequest++;
  stagedImport = undefined;
  commitImportButton.disabled = true;
  byId("import-warnings").hidden = true;
  status("import-status");
});

commitImportButton.addEventListener("click", async () => {
  if (!stagedImport) return;
  commitImportButton.disabled = true;
  try {
    const result = await api("/api/import", { ...stagedImport, commit: true });
    stagedImport = undefined;
    showImportSummary(result);
    await Promise.all([loadLedgers(), loadGroups(), api("/api/settings").then(showSettings)]);
    void refreshRegister(1);
  } catch (error) {
    status("import-status", error.message, true);
    commitImportButton.disabled = !stagedImport;
  }
});

function loadBankSelection() {
  const selected = (field) => Array.from(document.querySelectorAll('[data-bank-row="' + field + '"]:checked'));
  const statement = selected("statement");
  const book = selected("book");
  const total = (rows) => rows.reduce((sum, row) => sum + BigInt(row.dataset.paise), 0n);
  byId("bank-selected").textContent = "Selected statement " + money(total(statement)) + " · ledger " + money(total(book));
  return { statement, book };
}

function selectionOrMatch(row, type) {
  if (row.match_id !== null) {
    const button = element("button", "Undo match " + row.match_id, "voucher-link");
    button.type = "button";
    button.addEventListener("click", () => {
      selectedMatch = row.match_id;
      const form = byId("bank-undo-form");
      form.hidden = false;
      form.elements.actor.focus();
    });
    return button;
  }
  const input = element("input");
  input.type = "checkbox";
  input.setAttribute("aria-label", "Select " + (row.transaction_id ?? row.code + " line " + row.line_no));
  input.dataset.bankRow = type;
  input.dataset.paise = row.signed_paise;
  input.value = type === "statement" ? row.id : JSON.stringify({
    voucher_id: row.voucher_id, line_no: row.line_no, revision: row.revision,
  });
  input.addEventListener("change", loadBankSelection);
  return input;
}

async function refreshBank(values) {
  if (!values.ledger) throw new Error("Choose a bank ledger.");
  const request = ++bankRequest;
  reviewedLedger = undefined;
  byId("bank-review").hidden = true;
  const review = await api("/api/bank/" + values.ledger + "?" + periodQuery(values));
  if (request !== bankRequest) return;
  reviewedLedger = values.ledger;
  selectedMatch = undefined;
  byId("bank-undo-form").hidden = true;
  renderTable(byId("bank-statement"), ["Select / match", "Date", "Reference", "Amount"], review.statement.map((row) => [
    selectionOrMatch(row, "statement"), formatDate(row.date), row.reference || row.description || row.transaction_id, money(row.signed_paise),
  ]));
  renderTable(byId("bank-book"), ["Select / match", "Date", "Voucher", "Amount"], review.book.map((row) => [
    selectionOrMatch(row, "book"), formatDate(row.date), voucherButton(row.voucher_id, row.code), money(row.signed_paise),
  ]));
  byId("bank-review").hidden = false;
  loadBankSelection();
}

listenForm("bank-review-form", "bank-status", refreshBank);
bankReviewForm.addEventListener("input", () => {
  bankRequest++;
  reviewedLedger = undefined;
  byId("bank-review").hidden = true;
});
listenForm("bank-import-form", "bank-status", async (values) => {
  const ledger = bankReviewForm.elements.ledger.value;
  if (!ledger) throw new Error("Choose a bank ledger above.");
  const result = await api("/api/bank/" + ledger, { content: await fileText(values.file) });
  await refreshBank(formValues(bankReviewForm));
  status("bank-status", result.inserted + " statement rows added. " + result.duplicates + " duplicates found.");
});

listenForm("bank-match-form", "bank-status", async (values) => {
  if (!reviewedLedger) throw new Error("Review the bank ledger first.");
  const { statement, book } = loadBankSelection();
  const result = await api("/api/bank/" + reviewedLedger + "/matches", {
    ...values, statement_ids: statement.map((input) => Number(input.value)),
    postings: book.map((input) => JSON.parse(input.value)),
  });
  await refreshBank(formValues(bankReviewForm));
  status("bank-status", "Match " + result.match_id + " recorded for " + money(result.signed_paise) + ".");
});

listenForm("bank-undo-form", "bank-status", async (values, form) => {
  if (!selectedMatch) throw new Error("Choose a match to undo.");
  await api("/api/bank/matches/" + selectedMatch + "/undo", values);
  form.reset();
  await refreshBank(formValues(bankReviewForm));
  status("bank-status", "Bank match undone.");
});

for (const form of [reportsForm, bankReviewForm]) {
  form.elements.to.value = localDate();
  form.elements.from.value = localDate().slice(0, 8) + "01";
}
api("/api/settings").then(showSettings).catch((error) => status("company-status", error.message, true));
loadGroups().catch((error) => status("masters-status", error.message, true));
