"use strict";

const byId = (id) => document.getElementById(id);
const voucherForm = byId("voucher-form");
const voucherFields = byId("voucher-fields");
const cashFields = byId("cash-fields");
const journalFields = byId("journal-fields");
const journalLines = byId("journal-lines");
const addLineButton = byId("add-journal-line");
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
const kindNames = { payment: "Payment", receipt: "Receipt", journal: "Journal" };
const kindDescriptions = {
  payment: "Record money paid from cash or bank.",
  receipt: "Record money received into cash or bank.",
  journal: "Record an adjustment with balanced debit and credit lines.",
};
const shortcutKinds = { F5: "payment", F6: "receipt", F7: "journal" };
const maxJournalLines = 50;
const ledgerKindNames = { cash: "Cash", bank: "Bank", general: "Party / income / expense" };

let kind = "payment";
let ledgers = [];
let requestId;
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

async function api(path, payload) {
  const options = payload === undefined ? {} : {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  };
  let response;
  try {
    response = await fetch(path, options);
  } catch {
    throw new Error("Could not reach the local app. Check that it is running and try again.");
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
  setLedgerChoices(ledger, ledgers, "", "Select a ledger");
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
  if (kind === "journal") {
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

function switchKind(nextKind) {
  if (savingVoucher || kind === nextKind) return;
  kind = nextKind;
  const receipt = kind === "receipt";
  const journal = kind === "journal";
  for (const button of typeButtons) {
    button.setAttribute("aria-pressed", String(button.dataset.voucherKind === kind));
  }
  voucherForm.dataset.kind = kind;
  cashFields.hidden = cashFields.disabled = journal;
  journalFields.hidden = journalFields.disabled = !journal;
  byId("cash-preview").hidden = journal;
  byId("journal-preview").hidden = !journal;
  byId("mode-description").textContent = kindDescriptions[kind];
  byId("cash-label").textContent = receipt ? "Received into" : "Paid from";
  byId("party-label").textContent = receipt ? "Received from / income" : "Paid to / expense";
  voucherForm.elements.reference.placeholder = journal ? "Document or adjustment reference" : "Cheque, UTR, or document number";
  saveButton.textContent = "Save " + kind;
  status("voucher-status");
  updateSaveAvailability();
  updatePreview();
}

function updateSaveAvailability() {
  const ready = kind === "journal" ? ledgers.length > 0 : ledgers.some((ledger) => ledger.kind === "general");
  saveButton.disabled = savingVoucher || !ready;
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
  for (const line of journalLines.children) {
    const select = journalControl(line, "ledger");
    setLedgerChoices(select, ledgers, select.value, "Select a ledger");
  }

  const list = byId("ledger-list");
  list.replaceChildren();
  for (const ledger of ledgers) {
    const item = element("li");
    item.append(element("span", ledger.name), element("span", ledgerKindNames[ledger.kind]));
    list.append(item);
  }
  updateSaveAvailability();
  updatePreview();
}

async function loadLedgers() {
  const saved = await api("/api/ledgers");
  ledgers = [...new Map([...ledgers, ...saved].map((ledger) => [ledger.id, ledger])).values()];
  ledgers.sort((a, b) => a.name.localeCompare(b.name, "en-IN"));
  showLedgers();
}

function resetVoucher({ preserveCash = false } = {}) {
  const cash = cashSelect.value;
  voucherForm.reset();
  resetJournalLines();
  if (preserveCash) cashSelect.value = cash;
  dateField.value = localDate();
  requestId = undefined;
  showLedgers();
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
        if (emptyLedger) {
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
  const values = Object.fromEntries(new FormData(voucherForm));
  requestId ??= crypto.randomUUID();
  const payload = {
    ...values,
    kind,
    request_id: requestId,
  };
  if (kind === "journal") {
    const postings = readJournalLines();
    const invalid = postings.findIndex((posting) => !draftPaise(posting.amount));
    if (invalid !== -1) {
      status("voucher-status", "Enter a valid positive amount on line " + (invalid + 1) + ".", true);
      journalControl(journalLines.children[invalid], "amount").focus();
      return;
    }
    const { debit, credit } = journalTotals(postings);
    if (debit !== credit) {
      status("voucher-status", "Journal debits and credits must match.", true);
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
    const voucher = await api("/api/vouchers", payload);
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
  byId("detail-title").textContent = kindNames[voucher.kind] + " " + voucher.code;
  byId("detail-subtitle").textContent = formatDate(voucher.date);
  const fields = byId("detail-fields");
  fields.replaceChildren();
  const details = [
    ["Cash / bank", voucher.cash_account],
    ["Amount", money(voucher.amount_paise)],
    ["Reference", voucher.reference],
    ["Narration", voucher.narration],
  ];
  for (const [label, value] of details) {
    if (!value) continue;
    const detail = element("div", undefined, label === "Narration" ? "wide" : "");
    detail.append(element("dt", label), element("dd", value));
    fields.append(detail);
  }
  const postingRows = byId("detail-postings");
  postingRows.replaceChildren();
  const postings = voucher.postings ?? [
    { ledger: voucher.debit, side: "debit", amount_paise: voucher.amount_paise },
    { ledger: voucher.credit, side: "credit", amount_paise: voucher.amount_paise },
  ];
  for (const posting of postings) {
    const row = element("tr");
    const amount = money(posting.amount_paise);
    row.append(
      element("td", posting.ledger),
      element("td", posting.side === "debit" ? amount : "—"),
      element("td", posting.side === "credit" ? amount : "—"),
    );
    postingRows.append(row);
  }
  if (!detailDialog.open) detailDialog.showModal();
}

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
    row.insertCell().append(element("span", kindNames[voucher.kind], "kind-tag " + voucher.kind));
    const particulars = row.insertCell();
    if (voucher.kind === "journal") {
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
  const remove = event.target.closest('[data-journal="remove"]');
  if (!remove) return;
  const line = remove.closest(".journal-line");
  const next = line.nextElementSibling;
  line.remove();
  updateJournalControls();
  updatePreview();
  (next ? journalControl(next, "ledger") : addLineButton).focus();
});
byId("clear-voucher").addEventListener("click", () => {
  status("voucher-status");
  resetVoucher();
  dateField.focus();
});
voucherForm.addEventListener("input", () => {
  if (kind === "journal" && byId("voucher-status").classList.contains("error")) status("voucher-status");
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
  const section = location.hash === "#register" ? "register" : "entry";
  for (const link of document.querySelectorAll("[data-section]")) {
    const active = link.dataset.section === section;
    link.classList.toggle("active", active);
    if (active) link.setAttribute("aria-current", "location");
    else link.removeAttribute("aria-current");
  }
}

document.addEventListener("keydown", (event) => {
  if (ledgerDialog.open || detailDialog.open || savingVoucher || event.altKey) return;
  if (!event.ctrlKey && !event.metaKey && !event.shiftKey && Object.hasOwn(shortcutKinds, event.key)) {
    event.preventDefault();
    switchKind(shortcutKinds[event.key]);
    location.hash = "entry";
    (kind === "journal" ? journalControl(journalLines.firstElementChild, "ledger") : amountField).focus();
  } else if ((event.ctrlKey || event.metaKey) && event.key === "Enter"
      && voucherForm.contains(document.activeElement)) {
    event.preventDefault();
    if (!saveButton.disabled) voucherForm.requestSubmit();
  }
});

window.addEventListener("hashchange", updateNavigation);
updateNavigation();
dateField.value = localDate();
resetJournalLines();
updatePreview();
loadLedgers().catch((error) => status("voucher-status", error.message, true));
void refreshRegister(1);
