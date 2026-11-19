import { App } from "@modelcontextprotocol/ext-apps";

const app = new App({ name: "OpenTally Trial Balance", version: "0.3.0" });
const byId = (id) => document.getElementById(id);
const formatter = new Intl.NumberFormat("en-IN");
let data;

function money(paise) {
  const value = BigInt(paise);
  const integer = value < 0n ? -value : value;
  return "₹" + formatter.format(integer / 100n) + "." + String(integer % 100n).padStart(2, "0");
}

function balance(paise) {
  if (paise === null) return "Unconfirmed";
  const value = BigInt(paise);
  return money(value) + (value === 0n ? "" : value < 0n ? " Cr" : " Dr");
}

function render(result) {
  const report = result?.structuredContent ?? result;
  if (!report?.period || !Array.isArray(report.rows)) return;
  data = report;
  const page = data.pagination.rows;
  const start = report.rows.length ? page.offset + 1 : 0;
  byId("period").textContent = report.period.from + " to " + report.period.to
    + " · " + start + "–" + (page.offset + report.rows.length) + " / " + page.total_rows + " ledgers";
  byId("debit").textContent = money(report.debit_paise);
  byId("credit").textContent = money(report.credit_paise);
  byId("closing-debit").textContent = report.closing_debit_paise === null ? "Unconfirmed" : money(report.closing_debit_paise);
  byId("closing-credit").textContent = report.closing_credit_paise === null ? "Unconfirmed" : money(report.closing_credit_paise);
  byId("opening").textContent = report.missing_openings ? "Missing openings: " + report.missing_openings : "";
  const rows = report.rows.map((row) => {
    const tr = document.createElement("tr");
    for (const value of [row.ledger, balance(row.opening_paise), money(row.debit_paise), money(row.credit_paise), balance(row.closing_paise)]) {
      const cell = tr.insertCell();
      cell.textContent = value;
    }
    return tr;
  });
  byId("rows").replaceChildren(...rows);
  byId("report").hidden = false;
  byId("previous").disabled = page.offset === 0;
  byId("next").disabled = page.next_offset === null;
}

async function goTo(offset) {
  const page = data.pagination.rows;
  byId("previous").disabled = byId("next").disabled = true;
  try {
    const result = await app.callServerTool({
      name: "trial_balance",
      arguments: { ...data.period, offset, limit: page.limit },
    });
    if (result.isError) throw new Error("Report request failed");
    render(result);
  } catch {
    byId("period").textContent = "Could not load that page. Try again.";
    byId("previous").disabled = page.offset === 0;
    byId("next").disabled = page.next_offset === null;
  }
}

byId("previous").addEventListener("click", () => goTo(Math.max(0, data.pagination.rows.offset - data.pagination.rows.limit)));
byId("next").addEventListener("click", () => goTo(data.pagination.rows.next_offset));
app.ontoolresult = render;
try {
  await app.connect();
} catch {
  byId("period").textContent = "Open this report from the OpenTally plugin.";
}
