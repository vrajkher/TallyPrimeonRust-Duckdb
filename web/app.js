"use strict";

const byId = (id) => document.getElementById(id);
const voucherForm = byId("voucher-form");
const voucherFields = byId("voucher-fields");
const cashFields = byId("cash-fields");
const journalFields = byId("journal-fields");
const journalLines = byId("journal-lines");
const addLineButton = byId("add-journal-line");
const compoundCheckbox = byId("compound-entry");
const typeButtons = document.querySelectorAll("[data-voucher-kind]");
const registerForm = byId("register-form");
const ledgerForm = byId("ledger-form");
const ledgerDialog = byId("ledger-dialog");
const detailDialog = byId("detail-dialog");
const saveButton = byId("save-voucher");
const cashSelect = voucherForm.elements.cash_ledger_id;
const otherSelect = voucherForm.elements.other_ledger_id;
const dateField = voucherForm.elements.date;
const amountField = voucherForm.elements.amount;

const integerFormat = new Intl.NumberFormat("en-IN");
const dateFormat = new Intl.DateTimeFormat("en-IN", {
  day: "2-digit", month: "short", year: "numeric", timeZone: "UTC",
});
const kindNames = { payment: "Payment", receipt: "Receipt", journal: "Journal", contra: "Contra", other: "Imported" };
const kindDescriptions = {
  payment: "Record money paid from cash or bank.",
  receipt: "Record money received into cash or bank.",
  journal: "Record an adjustment with balanced debit and credit lines.",
  contra: "Transfer money between your cash and bank accounts.",
};
const shortcutKinds = { F5: "payment", F6: "receipt", F7: "journal" };
const maxJournalLines = 50;
const ledgerKindNames = { cash: "Cash", bank: "Bank", general: "Party / income / expense" };

let kind = "payment";
let ledgers = [];
let requestId;
let editingVoucher;
let displayedVoucher;
let savingVoucher = false;
let savingLedger = false;
let inlineLedger;
let registerRequest = 0;
let currentPage = 1;
let activeFilters = new URLSearchParams();

function localDate() {
  const now = new Date();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return [now.getFullYear(), month, day].join("-");
}

function formatDate(value) {
  const date = new Date(value + "T00:00:00Z");
  return value && Number.isFinite(date.getTime()) ? dateFormat.format(date) : "Choose a date";
}

function money(value) {
  const paise = BigInt(value);
  const magnitude = paise < 0n ? -paise : paise;
  return (paise < 0n ? "−" : "") + "₹" + integerFormat.format(magnitude / 100n)
    + "." + String(magnitude % 100n).padStart(2, "0");
}

function draftPaise(value) {
  if (!/^[0-9]+(?:\.[0-9]{1,2})?$/.test(value)) return 0n;
  const [rupees, fraction = ""] = value.split(".");
  const paise = BigInt(rupees) * 100n + BigInt(fraction.padEnd(2, "0"));
  return paise <= 9223372036854775807n ? paise : 0n;
}

function element(tag, text, className) {
  const result = document.createElement(tag);
  if (text !== undefined) result.textContent = text;
  if (className) result.className = className;
  return result;
}

function status(id, message = "", isError = false) {
  const target = byId(id);
  if (target.textContent !== message || target.childElementCount) target.textContent = message;
  target.classList.toggle("error", isError);
  target.hidden = !message;
  return target;
}

function cookieValue(name) {
  const match = document.cookie.split("; ").find((part) => part.startsWith(name + "="));
  return match ? decodeURIComponent(match.slice(name.length + 1)) : "";
}

let pendingSignIn;

// Shows the sign-in dialog and resolves once the user has signed in.
function signIn() {
  pendingSignIn ??= new Promise((resolve) => {
    const dialog = byId("sign-in-dialog");
    const form = byId("sign-in-form");
    status("sign-in-status");
    form.onsubmit = async (event) => {
      event.preventDefault();
      const values = Object.fromEntries(new FormData(form));
      const response = await fetch("/api/v1/session", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(values),
      }).catch(() => undefined);
      const data = await response?.json().catch(() => ({}));
      if (!response?.ok) {
        status("sign-in-status", data?.error || "Could not sign in. Try again.", true);
        return;
      }
      form.reset();
      dialog.close();
      showSession(data);
      pendingSignIn = undefined;
      resolve();
    };
    dialog.addEventListener("cancel", (event) => event.preventDefault());
    if (!dialog.open) dialog.showModal();
    form.elements.username.focus();
  });
  return pendingSignIn;
}

function showSession(session) {
  const user = session?.user;
  byId("session-label").textContent = user ? "Signed in as " + user.full_name : "Local books";
  byId("sign-out").hidden = !user;
}

async function api(path, payload, retried = false) {
  const options = payload === undefined ? {} : {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-CSRF-Token": cookieValue("ot_csrf") },
    body: JSON.stringify(payload),
  };
  let response;
  try {
    response = await fetch(path, options);
  } catch {
    throw new Error("Could not reach the local app. Check that it is running and try again.");
  }
  if (response.status === 401 && !retried) {
    await signIn();
    return api(path, payload, true);
  }
  if (!response.ok) {
    const error = await response.json().catch(() => ({}));
    throw new Error(error.error || "The request could not be completed. Check the entry and try again.");
  }
  return response.json();
}

function selectedLedger(select) {
  return ledgers.find((ledger) => String(ledger.id) === select.value);
}

function journalControl(line, control) {
  return line.querySelector('[data-journal="' + control + '"]');
}

function updateJournalControls() {
  const count = journalLines.children.length;
  for (const [index, line] of Array.from(journalLines.children).entries()) {
    const number = index + 1;
    journalControl(line, "side").setAttribute("aria-label", "Side, line " + number);
    journalControl(line, "ledger").setAttribute("aria-label", "Ledger, line " + number);
    journalControl(line, "amount").setAttribute("aria-label", "Amount, line " + number);
    const remove = journalControl(line, "remove");
    remove.setAttribute("aria-label", "Remove line " + number);
    remove.disabled = count <= 2;
  }
  addLineButton.disabled = count >= maxJournalLines;
}

function addJournalLine(side) {
  const line = byId("journal-line-template").content.firstElementChild.cloneNode(true);
  const sideSelect = journalControl(line, "side");
  sideSelect.value = side;
  sideSelect.selectedOptions[0].defaultSelected = true;
  const ledger = journalControl(line, "ledger");
  setLedgerChoices(ledger, eligiblePostingLedgers(), "", "Select a ledger");
  journalLines.append(line);
  updateJournalControls();
  return ledger;
}

function resetJournalLines() {
  journalLines.replaceChildren();
  addJournalLine("debit");
  addJournalLine("credit");
}

function readJournalLines() {
  return Array.from(journalLines.children, (line) => ({
    ledger_id: Number(journalControl(line, "ledger").value),
    side: journalControl(line, "side").value,
    amount: journalControl(line, "amount").value.trim(),
    narration: journalControl(line, "narration").value.trim(),
    allocations: Array.from(line.querySelectorAll(".allocation-entry"), (allocation) => ({
      category: allocation.querySelector("[data-category]").value.trim(),
      centre: allocation.querySelector("[data-centre]").value.trim(),
      amount: allocation.querySelector("[data-allocation-amount]").value.trim(),
    })),
  }));
}

function journalTotals(postings) {
  const totals = { debit: 0n, credit: 0n };
  for (const posting of postings) totals[posting.side] += draftPaise(posting.amount);
  return totals;
}

function updatePreview() {
  byId("preview-mode").textContent = kindNames[kind];
  byId("preview-date").textContent = formatDate(dateField.value);
  if (isLineEntry()) {
    const postings = readJournalLines();
    const { debit, credit } = journalTotals(postings);
    byId("journal-debit").textContent = money(debit);
    byId("journal-credit").textContent = money(credit);
    byId("journal-difference").textContent = money(debit >= credit ? debit - credit : credit - debit);
    if (debit !== credit) status("journal-balance", "Debits and credits must match.", true);
    else status("journal-balance", postings.every((line) => line.ledger_id && draftPaise(line.amount)) ? "Balanced" : "");
    return;
  }
  const cash = selectedLedger(cashSelect)?.name || "Choose a ledger";
  const other = selectedLedger(otherSelect)?.name || "Choose a ledger";
  byId("preview-debit").textContent = kind === "receipt" ? cash : other;
  byId("preview-credit").textContent = kind === "receipt" ? other : cash;
  const amount = draftPaise(amountField.value.trim());
  document.querySelectorAll(".preview-amount").forEach((target) => { target.textContent = amount ? money(amount) : "—"; });
}

function isLineEntry() {
  return kind === "journal" || kind === "contra" || compoundCheckbox.checked;
}

function eligiblePostingLedgers() {
  return kind === "contra" ? ledgers.filter((ledger) => ledger.kind !== "general") : ledgers;
}

function switchKind(nextKind) {
  if (savingVoucher || editingVoucher || kind === nextKind) return;
  kind = nextKind;
  updateEntryMode();
  status("voucher-status");
}

function updateEntryMode() {
  const receipt = kind === "receipt";
  const lines = isLineEntry();
  for (const button of typeButtons) {
    button.setAttribute("aria-pressed", String(button.dataset.voucherKind === kind));
    button.disabled = Boolean(editingVoucher);
  }
  voucherForm.dataset.entry = lines ? "lines" : "single";
  byId("compound-option").hidden = kind === "journal" || kind === "contra" || Boolean(editingVoucher);
  cashFields.hidden = cashFields.disabled = lines;
  journalFields.hidden = journalFields.disabled = !lines;
  byId("cash-preview").hidden = lines;
  byId("journal-preview").hidden = !lines;
  byId("edit-fields").hidden = !editingVoucher;
  voucherForm.elements.edit_actor.required = Boolean(editingVoucher);
  voucherForm.elements.edit_reason.required = Boolean(editingVoucher);
  byId("voucher-form-title").textContent = editingVoucher ? "Edit " + editingVoucher.code : "New voucher";
  byId("clear-voucher").textContent = editingVoucher ? "Discard edit" : "Clear form";
  byId("mode-description").textContent = kindDescriptions[kind];
  byId("cash-label").textContent = receipt ? "Received into" : "Paid from";
  byId("party-label").textContent = receipt ? "Received from / income" : "Paid to / expense";
  voucherForm.elements.reference.placeholder = kind === "journal" || kind === "contra"
    ? "Document or adjustment reference" : "Cheque, UTR, or document number";
  saveButton.textContent = editingVoucher ? "Save revision" : "Save " + kind;
  updatePostingChoices();
  updateSaveAvailability();
  updatePreview();
}

function updateSaveAvailability() {
  const ready = isLineEntry() ? eligiblePostingLedgers().length > 0 : ledgers.some((ledger) => ledger.kind === "general");
  saveButton.disabled = savingVoucher || !ready;
}

function updatePostingChoices() {
  for (const line of journalLines.children) {
    const select = journalControl(line, "ledger");
    setLedgerChoices(select, eligiblePostingLedgers(), select.value, "Select a ledger");
  }
}

function setLedgerChoices(select, options, selected, emptyLabel) {
  select.replaceChildren(new Option(emptyLabel, ""));
  for (const ledger of options) select.add(new Option(ledger.name, String(ledger.id)));
  select.value = options.some((ledger) => String(ledger.id) === selected) ? selected : "";
  select.disabled = options.length === 0;
}

function showLedgers(preferredOther = otherSelect.value) {
  const cashLedgers = ledgers.filter((ledger) => ledger.kind !== "general");
  const otherLedgers = ledgers.filter((ledger) => ledger.kind === "general");
  const defaultCash = cashLedgers.find((ledger) => ledger.kind === "cash") ?? cashLedgers[0];

  setLedgerChoices(cashSelect, cashLedgers, cashSelect.value || String(defaultCash?.id), "Select cash or bank");
  setLedgerChoices(otherSelect, otherLedgers, preferredOther, otherLedgers.length ? "Select a ledger" : "Add a ledger first");
  updatePostingChoices();

  const list = byId("ledger-list");
  list.replaceChildren();
  for (const ledger of ledgers) {
    const item = element("li");
    item.append(element("span", ledger.name), element("span", ledger.group ?? ledgerKindNames[ledger.kind]));
    list.append(item);
  }
  updateSaveAvailability();
  updatePreview();
  window.dispatchEvent(new Event("ledgers-updated"));
}

async function loadLedgers() {
  const saved = await api("/api/ledgers");
  ledgers = [...new Map([...ledgers, ...saved].map((ledger) => [ledger.id, ledger])).values()];
  ledgers.sort((a, b) => a.name.localeCompare(b.name, "en-IN"));
  showLedgers();
}

function resetVoucher({ preserveCash = false } = {}) {
  const cash = cashSelect.value;
  editingVoucher = undefined;
  voucherForm.reset();
  resetJournalLines();
  if (preserveCash) cashSelect.value = cash;
  dateField.value = localDate();
  requestId = undefined;
  showLedgers();
  updateEntryMode();
}

function openLedgers(mode) {
  inlineLedger = mode;
  byId("ledger-dialog-title").textContent = mode ? "Add a ledger" : "Ledgers";
  byId("ledger-list-section").hidden = Boolean(mode);
  ledgerForm.reset();
  ledgerForm.elements.kind.disabled = mode === "cash";
  status("ledger-status");
  ledgerDialog.showModal();
  ledgerForm.elements.name.focus();
  if (!ledgers.length) loadLedgers().catch((error) => status("ledger-status", error.message, true));
}

ledgerForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (savingLedger) return;
  savingLedger = true;
  const createButton = ledgerForm.querySelector("button[type=submit]");
  createButton.disabled = true;
  status("ledger-status");
  try {
    const ledger = await api("/api/ledgers", {
      name: ledgerForm.elements.name.value,
      kind: ledgerForm.elements.kind.value,
      group_id: Number(ledgerForm.elements.group_id.value) || null,
    });
    ledgers = ledgers.filter((existing) => existing.id !== ledger.id);
    ledgers.push(ledger);
    ledgers.sort((a, b) => a.name.localeCompare(b.name, "en-IN"));
    showLedgers(inlineLedger === "cash" ? String(ledger.id) : otherSelect.value);
    if (inlineLedger) {
      ledgerDialog.close();
      if (inlineLedger === "cash") {
        otherSelect.focus();
      } else {
        const emptyLedger = Array.from(journalLines.querySelectorAll('[data-journal="ledger"]'))
          .find((select) => !select.value);
        if (emptyLedger && eligiblePostingLedgers().some((item) => item.id === ledger.id)) {
          emptyLedger.value = String(ledger.id);
          emptyLedger.focus();
          updatePreview();
        }
      }
    } else {
      ledgerForm.reset();
      ledgerForm.elements.name.focus();
      status("ledger-status", ledger.name + " created.");
    }
  } catch (error) {
    status("ledger-status", error.message, true);
  } finally {
    savingLedger = false;
    createButton.disabled = false;
  }
});

voucherForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (saveButton.disabled || savingVoucher) return;
  const { edit_actor, edit_reason, ...values } = Object.fromEntries(new FormData(voucherForm));
  requestId ??= crypto.randomUUID();
  const payload = {
    ...values,
    kind,
    request_id: requestId,
  };
  if (isLineEntry()) {
    const postings = readJournalLines();
    const invalid = postings.findIndex((posting) => !draftPaise(posting.amount));
    if (invalid !== -1) {
      status("voucher-status", "Enter a valid positive amount on line " + (invalid + 1) + ".", true);
      journalControl(journalLines.children[invalid], "amount").focus();
      return;
    }
    const { debit, credit } = journalTotals(postings);
    if (debit !== credit) {
      status("voucher-status", (kind === "journal" ? "Journal" : "Voucher") + " debits and credits must match.", true);
      return;
    }
    if (postings.some((line) => line.allocations.some((a) => !a.category || !a.centre || !draftPaise(a.amount)))) {
      status("voucher-status", "Complete each cost allocation with a category, centre, and positive amount.", true);
      return;
    }
    payload.postings = postings;
  } else {
    payload.cash_ledger_id = Number(values.cash_ledger_id);
    payload.other_ledger_id = Number(values.other_ledger_id);
  }
  savingVoucher = true;
  voucherFields.disabled = true;
  updateSaveAvailability();
  status("voucher-status");
  byId("draft-status").textContent = "Saving…";

  try {
    const voucher = editingVoucher
      ? await api("/api/vouchers/" + editingVoucher.id + "/changes", {
          expected_revision: editingVoucher.revision, actor: edit_actor, reason: edit_reason, voucher: payload,
        })
      : await api("/api/vouchers", payload);
    resetVoucher({ preserveCash: true });
    const notice = status("voucher-status", voucher.code + " saved. ");
    notice.append(voucherButton(voucher.id, "View voucher"));
    void refreshRegister(1);
  } catch (error) {
    status("voucher-status", error.message, true);
  } finally {
    savingVoucher = false;
    voucherFields.disabled = false;
    byId("draft-status").textContent = "Draft";
    updateSaveAvailability();
  }
});

function voucherButton(id, label) {
  const button = element("button", label, "voucher-link");
  button.type = "button";
  button.addEventListener("click", async () => {
    try {
      showVoucher(await api("/api/vouchers/" + id));
    } catch (error) {
      status(button.closest("#voucher-status") ? "voucher-status" : "register-status", error.message, true);
    }
  });
  return button;
}

function showVoucher(voucher) {
  displayedVoucher = voucher;
  byId("detail-title").textContent = (voucher.type_name ?? kindNames[voucher.kind]) + " " + voucher.code;
  byId("start-cancellation").hidden = byId("edit-voucher").hidden = voucher.status !== "posted" || Boolean(voucher.source_guid);
  byId("cancel-voucher-form").hidden = true;
  byId("cancel-voucher-form").reset();
  byId("voucher-history").hidden = true;
  status("voucher-detail-status");
  byId("detail-subtitle").textContent = formatDate(voucher.date);
  const fields = byId("detail-fields");
  fields.replaceChildren();
  const details = [
    ["Cash / bank", voucher.cash_account],
    ["Amount", money(voucher.amount_paise)],
    ["Status", voucher.status],
    ["Series", voucher.series],
    ["Reference", voucher.reference],
    ["Narration", voucher.narration],
  ];
  for (const [label, value] of details) {
    if (!value) continue;
    const detail = element("div", undefined, label === "Narration" ? "wide" : "");
    detail.append(element("dt", label), element("dd", value));
    fields.append(detail);
  }
  renderPostingRows(byId("detail-postings"), voucher.postings ?? []);
  if (!detailDialog.open) detailDialog.showModal();
}

function renderPostingRows(target, postings) {
  target.replaceChildren();
  for (const posting of postings) {
    const row = element("tr");
    const amount = money(posting.amount_paise);
    const ledger = element("td", posting.ledger);
    if (posting.narration) ledger.append(element("small", posting.narration, "posting-note"));
    for (const allocation of posting.allocations ?? []) {
      ledger.append(element("small", allocation.category + " / " + allocation.centre + " · " + money(allocation.amount_paise), "posting-note"));
    }
    row.append(
      ledger,
      element("td", posting.side === "debit" ? amount : "—"),
      element("td", posting.side === "credit" ? amount : "—"),
    );
    target.append(row);
  }
}

function amountText(paise) {
  return (BigInt(paise) / 100n).toString() + "." + (BigInt(paise) % 100n).toString().padStart(2, "0");
}

function addAllocation(line, initial) {
  const row = byId("allocation-template").content.firstElementChild.cloneNode(true);
  if (initial) {
    row.querySelector("[data-category]").value = initial.category;
    row.querySelector("[data-centre]").value = initial.centre;
    row.querySelector("[data-allocation-amount]").value = amountText(initial.amount_paise);
  }
  line.querySelector(".allocations").append(row);
  return row;
}

function editDisplayedVoucher() {
  const voucher = displayedVoucher;
  if (!voucher || voucher.status !== "posted" || voucher.source_guid || savingVoucher) return;
  resetVoucher();
  switchKind(voucher.kind);
  editingVoucher = voucher;
  compoundCheckbox.checked = true;
  journalLines.replaceChildren();
  for (const posting of voucher.postings) {
    const ledger = addJournalLine(posting.side);
    const line = ledger.closest(".journal-line");
    ledger.value = String(posting.ledger_id);
    journalControl(line, "amount").value = amountText(posting.amount_paise);
    journalControl(line, "narration").value = posting.narration;
    for (const allocation of posting.allocations) addAllocation(line, allocation);
  }
  dateField.value = voucher.date;
  voucherForm.elements.reference.value = voucher.reference;
  voucherForm.elements.narration.value = voucher.narration;
  detailDialog.close();
  location.hash = "entry";
  updateEntryMode();
  dateField.focus();
}

byId("edit-voucher").addEventListener("click", editDisplayedVoucher);
byId("start-cancellation").addEventListener("click", () => {
  const form = byId("cancel-voucher-form");
  form.hidden = false;
  form.elements.actor.focus();
});
byId("cancel-voucher-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const data = Object.fromEntries(new FormData(form));
  const voucher = displayedVoucher;
  const button = form.querySelector("button");
  button.disabled = true;
  try {
    const updated = await api("/api/vouchers/" + voucher.id + "/changes", {
      ...data, expected_revision: voucher.revision,
    });
    if (displayedVoucher === voucher && detailDialog.open) {
      showVoucher(updated);
      status("voucher-detail-status", "Voucher cancelled.");
    }
    void refreshRegister();
  } catch (error) {
    status("voucher-detail-status", error.message, true);
  } finally {
    button.disabled = false;
  }
});

byId("show-voucher-history").addEventListener("click", async () => {
  const voucher = displayedVoucher;
  const target = byId("voucher-history");
  if (!target.hidden) { target.hidden = true; return; }
  try {
    const path = "/api/vouchers/" + voucher.id + "/changes";
    const events = await api(path);
    if (displayedVoucher !== voucher) return;
    target.replaceChildren(element("h3", "Voucher history"));
    for (const entry of events) {
      const record = element("div", undefined, "history-event");
      const button = element("button", "Revision " + entry.revision + " · " + entry.action, "voucher-link");
      button.type = "button";
      const preview = element("div");
      preview.hidden = true;
      button.addEventListener("click", async () => {
        if (!preview.hidden) { preview.hidden = true; return; }
        try {
          const version = await api(path + "/" + entry.revision);
          const table = element("table", undefined, "detail-table");
          const header = table.createTHead().insertRow();
          for (const label of ["Ledger", "Debit", "Credit"]) header.append(element("th", label));
          renderPostingRows(table.createTBody(), version.postings ?? []);
          preview.replaceChildren(element("p", "Voucher date " + formatDate(version.date)), table);
          preview.hidden = false;
        } catch (error) { status("voucher-detail-status", error.message, true); }
      });
      record.append(button, element("p", entry.actor + " · " + entry.recorded_at), element("p", entry.reason), preview);
      target.append(record);
    }
    target.hidden = false;
  } catch (error) { status("voucher-detail-status", error.message, true); }
});

function renderRegister(register) {
  const rows = byId("register-rows");
  rows.replaceChildren();
  if (!register.rows.length) {
    const row = rows.insertRow();
    const cell = row.insertCell();
    cell.colSpan = 6;
    cell.className = "empty";
    cell.textContent = activeFilters.size ? "No vouchers match these filters." : "Saved vouchers will appear here.";
  }
  for (const voucher of register.rows) {
    const row = rows.insertRow();
    row.insertCell().append(voucherButton(voucher.id, voucher.code));
    row.insertCell().textContent = formatDate(voucher.date);
    const typeCell = row.insertCell();
    typeCell.append(element("span", voucher.type_name ?? kindNames[voucher.kind], "kind-tag " + voucher.kind));
    if (voucher.status && voucher.status !== "posted") typeCell.append(element("small", voucher.status, "posting-note"));
    const particulars = row.insertCell();
    if (!voucher.cash_account || voucher.source_guid) {
      particulars.append(element("span", voucher.other_account, "journal-particulars"));
    } else {
      particulars.textContent = voucher.other_account;
    }
    row.insertCell().textContent = voucher.cash_account || "—";
    row.insertCell().textContent = money(voucher.amount_paise);
  }

  const count = integerFormat.format(register.count);
  byId("register-count").textContent = count + (register.count === 1 ? " voucher" : " vouchers");
  byId("receipt-total").textContent = money(register.receipt_paise);
  byId("payment-total").textContent = money(register.payment_paise);
  byId("net-total").textContent = money(BigInt(register.receipt_paise) - BigInt(register.payment_paise));
  const pages = Math.max(1, Math.ceil(register.count / register.page_size));
  currentPage = register.page;
  byId("pagination").hidden = pages <= 1;
  byId("page-label").textContent = "Page " + currentPage + " of " + pages;
  byId("previous-page").disabled = currentPage <= 1;
  byId("next-page").disabled = currentPage >= pages;
}

async function refreshRegister(page = currentPage) {
  const request = ++registerRequest;
  const filters = new URLSearchParams(activeFilters);
  filters.set("page", page);
  status("register-status");
  try {
    const register = await api("/api/vouchers?" + filters.toString());
    if (request === registerRequest) renderRegister(register);
  } catch (error) {
    if (request === registerRequest) {
      const count = byId("register-count");
      if (count.textContent === "Loading vouchers…") count.textContent = "Could not load vouchers";
      status("register-status", error.message, true);
    }
  }
}

registerForm.addEventListener("submit", (event) => {
  event.preventDefault();
  const filters = Object.fromEntries(new FormData(registerForm));
  if (filters.from && filters.to && filters.from > filters.to) {
    status("register-status", "The start date must be on or before the end date.", true);
    return;
  }
  activeFilters = new URLSearchParams();
  for (const [name, value] of Object.entries(filters)) {
    if (value.trim()) activeFilters.set(name, value.trim());
  }
  void refreshRegister(1);
});

byId("clear-filters").addEventListener("click", () => {
  registerForm.reset();
  activeFilters = new URLSearchParams();
  void refreshRegister(1);
});
byId("reload-register").addEventListener("click", () => void refreshRegister());
byId("previous-page").addEventListener("click", () => void refreshRegister(currentPage - 1));
byId("next-page").addEventListener("click", () => void refreshRegister(currentPage + 1));
byId("manage-ledgers").addEventListener("click", () => openLedgers());
byId("inline-ledger").addEventListener("click", () => openLedgers("cash"));
byId("journal-ledger").addEventListener("click", () => openLedgers("journal"));
byId("print-voucher").addEventListener("click", () => window.print());
for (const button of typeButtons) {
  button.addEventListener("click", () => switchKind(button.dataset.voucherKind));
}
addLineButton.addEventListener("click", () => {
  addJournalLine("debit").focus();
  updatePreview();
});
journalLines.addEventListener("click", (event) => {
  const allocationButton = event.target.closest("[data-add-allocation]");
  if (allocationButton) {
    addAllocation(allocationButton.closest(".journal-line")).querySelector("input").focus();
    return;
  }
  const removeAllocation = event.target.closest("[data-remove-allocation]");
  if (removeAllocation) {
    removeAllocation.closest(".allocation-entry").remove();
    return;
  }
  const remove = event.target.closest('[data-journal="remove"]');
  if (!remove) return;
  const line = remove.closest(".journal-line");
  const next = line.nextElementSibling;
  line.remove();
  updateJournalControls();
  updatePreview();
  (next ? journalControl(next, "ledger") : addLineButton).focus();
});
compoundCheckbox.addEventListener("change", updateEntryMode);
byId("clear-voucher").addEventListener("click", () => {
  status("voucher-status");
  resetVoucher();
  dateField.focus();
});
voucherForm.addEventListener("input", () => {
  if (isLineEntry() && byId("voucher-status").classList.contains("error")) status("voucher-status");
  updatePreview();
});

for (const button of document.querySelectorAll("[data-close]")) {
  button.addEventListener("click", () => {
    if (!savingLedger || button.dataset.close !== "ledger-dialog") byId(button.dataset.close).close();
  });
}
ledgerDialog.addEventListener("cancel", (event) => {
  if (savingLedger) event.preventDefault();
});

function updateNavigation() {
  const section = location.hash.slice(1) || "entry";
  for (const link of document.querySelectorAll("[data-section]")) {
    const active = link.dataset.section === section;
    link.classList.toggle("active", active);
    if (active) link.setAttribute("aria-current", "location");
    else link.removeAttribute("aria-current");
  }
}

document.addEventListener("keydown", (event) => {
  if (ledgerDialog.open || detailDialog.open || byId("sign-in-dialog").open || savingVoucher || event.altKey) return;
  if (!event.ctrlKey && !event.metaKey && !event.shiftKey && Object.hasOwn(shortcutKinds, event.key)) {
    event.preventDefault();
    switchKind(shortcutKinds[event.key]);
    location.hash = "entry";
    (isLineEntry() ? journalControl(journalLines.firstElementChild, "ledger") : amountField).focus();
  } else if ((event.ctrlKey || event.metaKey) && event.key === "Enter"
      && voucherForm.contains(document.activeElement)) {
    event.preventDefault();
    if (!saveButton.disabled) voucherForm.requestSubmit();
  }
});

byId("sign-out").addEventListener("click", async () => {
  await fetch("/api/v1/session", { method: "DELETE" }).catch(() => undefined);
  location.reload();
});
fetch("/api/v1/session").then((response) => response.json()).then(showSession).catch(() => undefined);

window.addEventListener("hashchange", updateNavigation);
updateNavigation();
dateField.value = localDate();
resetJournalLines();
updatePreview();
loadLedgers().catch((error) => status("voucher-status", error.message, true));
void refreshRegister(1);
