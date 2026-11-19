import { z } from "zod";

const id = z.number().int().positive();
const revision = id.max(2147483647);
const date = z.string().regex(/^\d{4}-\d{2}-\d{2}$/, "Use YYYY-MM-DD.");
const required = (length) => z.string().trim().min(1).max(length);
const memo = (length) => z.string().max(length).optional();
const amount = z.string().regex(/^\d{1,18}(?:\.\d{1,2})?$/);
const signedAmount = z.string().regex(/^[+-]?\d{1,18}(?:\.\d{1,2})?$/);
const ledgerKind = z.enum(["cash", "bank", "general"]);
const voucherKind = z.enum(["payment", "receipt", "journal", "contra", "other"]);
const voucherStatus = z.enum(["posted", "cancelled", "optional"]);
const period = { from: date, to: date };
const windowFields = {
  offset: z.number().int().min(0).optional().describe("Zero-based offset for the returned rows."),
  limit: z.number().int().min(1).max(100).optional().describe("Maximum rows per table. Defaults to 25."),
};
const operator = { actor: required(100).describe("Name supplied by the operator for the audit record.") };
const reason = { reason: required(500) };
const posting = z.strictObject({
  ledger_id: id,
  side: z.enum(["debit", "credit"]),
  amount: amount.describe("Positive amount in rupees, as a decimal string."),
  narration: memo(500),
  allocations: z.array(z.strictObject({
    category: required(100), centre: required(100), amount,
  })).max(100).optional(),
});
const voucherHeader = {
  request_id: z.uuid().describe("A UUID for this posting request. Reuse the same UUID on a retry."),
  date,
  reference: memo(100),
  narration: memo(500),
};
const simple = z.strictObject({
  ...voucherHeader,
  kind: z.enum(["payment", "receipt"]),
  cash_ledger_id: id,
  other_ledger_id: id,
  amount,
});
const compound = z.strictObject({
  ...voucherHeader,
  kind: z.enum(["payment", "receipt", "journal", "contra"]),
  postings: z.array(posting).min(2).max(50),
});
const voucher = z.union([simple, compound]);
const importFields = {
  format: z.enum(["xml", "json"]),
  content: z.string().min(1).max(2_000_000).describe("Full text of a Tally export or OpenTally JSON file."),
  company: required(150).optional(),
  opening_date: date.optional(),
};

function pathWithQuery(path, entries) {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(entries)) {
    if (value !== undefined) query.set(key, String(value));
  }
  return query.size ? path + "?" + query : path;
}

function definition(name, title, description, fields, execute, options = {}) {
  const schema = z.strictObject(fields);
  const { $schema, ...inputSchema } = z.toJSONSchema(schema);
  return { name, title, description, schema, inputSchema, execute, ...options };
}

function queryTool(name, title, description, fields, path, keys) {
  return definition(
    name, title, description, keys ? { ...fields, ...windowFields } : fields,
    async (args, api) => {
      const { offset, limit, ...params } = args;
      const route = typeof path === "function" ? path(params) : pathWithQuery(path, params);
      const value = await api(route);
      return keys ? windowResult(value, keys, offset, limit) : value;
    },
  );
}

function changeTool(name, title, description, fields, path, body, options = {}) {
  return definition(name, title, description, fields,
    async (args, api) => (await api(typeof path === "function" ? path(args) : path, body(args))) ?? { saved: true },
    { write: true, ...options },
  );
}

function windowResult(value, keys, offset = 0, limit = 25) {
  const original = Array.isArray(value) ? { rows: value } : value;
  const data = { ...original };
  const pagination = {};
  for (const key of keys) {
    const rows = original[key] ?? [];
    data[key] = rows.slice(offset, offset + limit);
    pagination[key] = { offset, limit, total_rows: rows.length, next_offset: rows.length > offset + limit ? offset + limit : null };
  }
  return { ...data, pagination };
}

const identity = (value) => value;
const byVoucher = ({ voucher_id }) => "/api/vouchers/" + voucher_id;
const voucherChanges = (params) => byVoucher(params) + "/changes";
const bankLedger = ({ ledger_id }) => "/api/bank/" + ledger_id;

export const tools = [
  queryTool("get_company", "Get company and period settings",
    "Read the current OpenTally company, source company and closing date.", {}, "/api/settings"),
  queryTool("list_groups", "List account groups",
    "Find group IDs, names and parent IDs.", {}, "/api/groups", ["rows"]),
  queryTool("list_ledgers", "List ledgers",
    "Find ledger IDs, names, ledger types and account groups.", {}, "/api/ledgers", ["rows"]),
  queryTool("list_vouchers", "Search Day Book",
    "Search Day Book by voucher type, status, reference, ledger, narration or inclusive date range. Results contain up to 25 vouchers per page.",
    { kind: voucherKind.optional(), from: date.optional(), to: date.optional(), q: memo(100),
      status: voucherStatus.optional(), page: id.max(4294967295).optional() }, "/api/vouchers"),
  queryTool("get_voucher", "Get voucher and its postings",
    "Read a voucher by ID, including ordered debit and credit lines, narration and cost allocations.",
    { voucher_id: id }, byVoucher, ["postings"]),
  queryTool("list_voucher_revisions", "List voucher revisions",
    "Read the voucher audit trail, including the stated operator, action, reason, and date.",
    { voucher_id: id }, voucherChanges, ["rows"]),
  queryTool("get_voucher_revision", "Get a voucher revision",
    "Read the ordered posting lines for one historical revision.",
    { voucher_id: id, revision }, (args) => voucherChanges(args) + "/" + args.revision, ["postings"]),
  definition("trial_balance", "Run Trial Balance",
    "Show opening, debit and credit movements, and closing balances for an inclusive period. Missing openings remain unconfirmed. Totals cover all rows.",
    { ...period, ...windowFields }, async ({ from, to, offset, limit }, api) => {
      const report = await api(pathWithQuery("/api/reports/trial-balance", { from, to }));
      return { period: { from, to }, ...windowResult(report, ["rows"], offset, limit) };
    }, { widget: true }),
  queryTool("ledger_statement", "Get ledger statement",
    "Read a ledger's opening, closing, and individual posted entries for an inclusive period.",
    { ledger_id: id, ...period },
    ({ ledger_id, ...params }) => pathWithQuery("/api/reports/ledgers/" + ledger_id, params), ["rows"]),
  queryTool("cost_center_totals", "Get cost-centre totals",
    "Read allocated debit and credit totals by category and cost centre. Categories may overlap.",
    period, "/api/reports/cost-centres", ["rows"]),
  queryTool("cost_center_postings", "Get cost-centre postings",
    "Trace a cost category and centre back to individual voucher posting lines.",
    { ...period, category: required(100), centre: required(100) }, "/api/reports/cost-centres/postings", ["rows"]),
  queryTool("bank_reconciliation", "Review bank reconciliation",
    "List statement transactions and ledger postings with their active match IDs.",
    { ledger_id: id, ...period }, ({ ledger_id, ...params }) => pathWithQuery(bankLedger({ ledger_id }), params),
    ["statement", "book"]),
  definition("preview_import", "Preview a Tally or OpenTally import",
    "Validate accounting data and show totals, duplicates and warnings. The preview rolls back its database transaction.",
    importFields, (args, api) => api("/api/import", { ...args, commit: false })),
  changeTool("create_group", "Create account group",
    "Add an account group under an optional existing parent.",
    { name: required(100), parent_id: id.optional() }, "/api/groups", identity),
  changeTool("create_ledger", "Create ledger",
    "Add a cash, bank or general ledger under an optional account group.",
    { name: required(100), kind: ledgerKind, group_id: id.optional() }, "/api/ledgers", identity),
  changeTool("post_voucher", "Post voucher",
    "Post an INR Payment, Receipt, Journal or Contra voucher. The open period and balanced accounting lines are checked.",
    { voucher }, "/api/vouchers", ({ voucher }) => voucher, { idempotent: true }),
  changeTool("edit_voucher", "Edit posted voucher",
    "Replace an open-period local voucher’s posting data and retain the prior revision. Use the current revision and keep the same kind.",
    { voucher_id: id, expected_revision: revision, ...operator, ...reason, voucher }, voucherChanges,
    ({ voucher_id, ...body }) => body, { destructive: true }),
  changeTool("cancel_voucher", "Cancel posted voucher",
    "Cancel a local voucher in an open period. The voucher remains in Day Book and its accounting effect is removed.",
    { voucher_id: id, expected_revision: revision, ...operator, ...reason }, voucherChanges,
    ({ voucher_id, ...body }) => body, { destructive: true }),
  changeTool("commit_import", "Import accounting data",
    "Write a Tally XML or OpenTally JSON batch. A higher source revision can update an existing voucher.",
    importFields, "/api/import", (args) => ({ ...args, commit: true }), { destructive: true }),
  changeTool("record_opening", "Record ledger opening balance",
    "Set a dated opening balance with a source reference. Use positive rupees for debit, negative for credit and 0.00 for a confirmed zero.",
    { ledger_id: id, date, amount: signedAmount, source: required(200) },
    ({ ledger_id }) => "/api/ledgers/" + ledger_id + "/openings", ({ ledger_id, ...body }) => body, { idempotent: true }),
  changeTool("set_company_name", "Change company name",
    "Change the display name of the local book.",
    { name: required(150) }, "/api/settings", identity, { destructive: true, idempotent: true }),
  changeTool("close_period", "Close accounting period",
    "Lock posting and corrections through the supplied date.",
    { through: date, ...operator, ...reason }, "/api/settings/close-period", identity, { destructive: true }),
  changeTool("import_bank_statement", "Import bank statement CSV",
    "Import bank lines with stable transaction IDs, YYYY-MM-DD dates and signed rupee amounts. Deposits are positive.",
    { ledger_id: id, content: z.string().min(1).max(750_000) }, bankLedger,
    ({ ledger_id, ...body }) => body, { idempotent: true }),
  changeTool("match_bank_rows", "Match bank and ledger rows",
    "Record a reviewed match using statement IDs and posting identifiers. Signed totals must agree.",
    { ledger_id: id, statement_ids: z.array(id).min(1).max(100),
      postings: z.array(z.strictObject({ voucher_id: id, revision, line_no: revision })).min(1).max(100),
      ...operator, note: memo(500) },
    (args) => bankLedger(args) + "/matches", ({ ledger_id, ...body }) => body),
  changeTool("undo_bank_match", "Undo bank reconciliation match",
    "Undo a bank match and retain the operator’s reason. Its rows return to the unmatched list.",
    { match_id: id, ...operator, ...reason }, ({ match_id }) => "/api/bank/matches/" + match_id + "/undo",
    ({ match_id, ...body }) => body, { destructive: true }),
];
