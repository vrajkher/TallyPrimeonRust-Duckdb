const dateArguments = [
  { name: "from", description: "First date, YYYY-MM-DD", required: true },
  { name: "to", description: "Last date, YYYY-MM-DD", required: true },
];

export const prompts = [
  {
    name: "review_period",
    title: "Review a period before closing",
    description: "Review Day Book, Trial Balance, openings and unresolved differences for a period.",
    arguments: dateArguments,
    instructions: "Check company settings and Trial Balance for the supplied dates. Review missing openings, any imbalance, and Day Book exceptions. Link exceptions to their voucher or ledger IDs. Present a review summary.",
  },
  {
    name: "review_bank",
    title: "Review a bank reconciliation",
    description: "Compare bank statement and ledger rows, including possible grouped matches.",
    arguments: [...dateArguments, { name: "ledger_id", description: "Bank ledger ID", required: true }],
    instructions: "Review the bank statement and book postings for the supplied ledger and dates. Compare dates, references and signed totals. Identify possible one-to-many matches and items that need an operator’s decision.",
  },
  {
    name: "review_import",
    title: "Review an accounting import",
    description: "Review the controls and evidence needed to preview a Tally or OpenTally import.",
    instructions: "Use preview_import when the user supplies the original file text and company. Summarize posted debit and credit totals, rejected data, duplicate and revised vouchers, and any evidence still needed.",
  },
];

export const guides = new Map([
  ["opentally://guide/accounting", {
    name: "OpenTally accounting and report guide",
    description: "Report date scope, amount formats, journal sides, and voucher identities.",
    mimeType: "text/markdown",
    text: [
      "# OpenTally accounting guide",
      "OpenTally stores one INR company per DuckDB file. Write amounts are decimal rupee strings. Report fields with a _paise suffix are exact integer strings. Signed balances use debit positive and credit negative. A null opening or closing indicates unconfirmed data.",
      "Day Book accepts inclusive dates, status and voucher kind. Report row windows use offset and limit. Headline report totals cover the full result. Find ledger and voucher identifiers before opening detail tools.",
      "The application validates balanced ordered postings, closed periods, bank matches and voucher revisions. Use a stable request UUID for voucher creation and retry. Edit and cancellation retain the previous revision.",
    ].join("\n\n"),
  }],
  ["opentally://guide/import-reconciliation", {
    name: "OpenTally import and bank guide",
    description: "Import preview controls and bank match rules.",
    mimeType: "text/markdown",
    text: [
      "# OpenTally import and reconciliation guide",
      "Preview the original Tally XML or OpenTally JSON text before choosing whether to persist an import. Supply its source company and a confirmed opening date when available. Check voucher and posting counts, debit and credit totals, duplicate and revised entries, and import warnings.",
      "Bank CSV columns are transaction_id, date, amount, reference and description. Use an immutable source transaction ID and a YYYY-MM-DD date. Deposits are positive and withdrawals negative.",
      "A reviewed match can span multiple statement lines and postings. Both signed totals must agree. Match IDs identify the reconciliation group. Undo records the operator and reason and returns the underlying rows to review.",
    ].join("\n\n"),
  }],
]);

export const widgetUri = "ui://opentally/trial-balance-v1.html";
