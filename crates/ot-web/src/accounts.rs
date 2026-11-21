//! The Chart of Accounts: groups and ledgers as an indented tree (roadmap step 1.3).

use std::collections::HashSet;

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{ChartOfAccounts, GroupAccount, LedgerAccount};

use crate::api::{self, ApiError};

/// One line of the tree, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub name: String,
    pub is_group: bool,
    pub root_type: Option<String>,
    pub account_type: Option<String>,
}

fn group_row(group: &GroupAccount, depth: usize) -> TreeRow {
    TreeRow {
        depth,
        name: group.name.clone(),
        is_group: true,
        root_type: group.root_type.clone(),
        account_type: group.account_type.clone(),
    }
}

fn ledger_row(ledger: &LedgerAccount, depth: usize) -> TreeRow {
    TreeRow {
        depth,
        name: ledger.name.clone(),
        is_group: false,
        root_type: ledger.root_type.clone(),
        account_type: ledger.effective_type.clone(),
    }
}

/// Flattens the chart depth-first: each group is followed by its subgroups, then its
/// ledgers. Ledgers outside any group come last. Names sort alphabetically.
pub fn flatten(chart: &ChartOfAccounts) -> Vec<TreeRow> {
    let mut groups: Vec<&GroupAccount> = chart.groups.iter().collect();
    groups.sort_by_key(|g| g.name.to_lowercase());
    let mut ledgers: Vec<&LedgerAccount> = chart.ledgers.iter().collect();
    ledgers.sort_by_key(|l| l.name.to_lowercase());
    let ids: HashSet<i64> = groups.iter().map(|g| g.id).collect();
    let known = |id: Option<i64>| id.is_some_and(|id| ids.contains(&id));

    let mut walk = Walk {
        groups: &groups,
        ledgers: &ledgers,
        visited: HashSet::new(),
        rows: Vec::with_capacity(groups.len() + ledgers.len()),
    };
    for group in groups.iter().filter(|g| !known(g.parent_id)) {
        walk.group(group, 0);
    }
    // Groups in a parent cycle have no root; show them at the top level anyway.
    for group in &groups {
        walk.group(group, 0);
    }
    walk.rows.extend(
        ledgers
            .iter()
            .filter(|l| !known(l.group_id))
            .map(|l| ledger_row(l, 0)),
    );
    walk.rows
}

struct Walk<'a> {
    groups: &'a [&'a GroupAccount],
    ledgers: &'a [&'a LedgerAccount],
    visited: HashSet<i64>,
    rows: Vec<TreeRow>,
}

impl Walk<'_> {
    fn group(&mut self, group: &GroupAccount, depth: usize) {
        // A group is shown once, so a parent cycle cannot recurse forever.
        if !self.visited.insert(group.id) {
            return;
        }
        self.rows.push(group_row(group, depth));
        let groups = self.groups;
        for child in groups.iter().filter(|g| g.parent_id == Some(group.id)) {
            self.group(child, depth + 1);
        }
        let ledgers = self.ledgers;
        self.rows.extend(
            ledgers
                .iter()
                .filter(|l| l.group_id == Some(group.id))
                .map(|l| ledger_row(l, depth + 1)),
        );
    }
}

#[component]
pub fn ChartView(on_signed_out: Callback<()>) -> impl IntoView {
    let chart = RwSignal::new(None::<Result<ChartOfAccounts, ApiError>>);
    spawn_local(async move {
        let result = api::chart_of_accounts().await;
        if let Err(error) = &result
            && error.signed_out
        {
            on_signed_out.run(());
            return;
        }
        chart.set(Some(result));
    });

    view! {
        <header class="page-heading">
            <h1>"Chart of Accounts"</h1>
            <p class="muted">"Groups and ledgers with their root type and account type."</p>
        </header>
        {move || match chart.get() {
            None => view! { <p class="muted">"Loading the chart…"</p> }.into_any(),
            Some(Err(error)) => view! { <p class="status error" role="alert">{error.message}</p> }.into_any(),
            Some(Ok(chart)) => {
                let rows = flatten(&chart);
                let unclassified = chart.unclassified_ledgers;
                view! {
                    <p class="totals">
                        {format!("{} groups, {} ledgers", chart.groups.len(), chart.ledgers.len())}
                        {(unclassified > 0).then(|| format!(" · {unclassified} ledgers have no root type yet"))}
                    </p>
                    <div class="table-wrap">
                        <table class="list-view chart-tree">
                            <thead><tr><th>"Account"</th><th>"Root type"</th><th>"Account type"</th></tr></thead>
                            <tbody>
                                {rows.into_iter().map(|row| {
                                    let indent = format!("padding-left: {}px", 14 + row.depth * 20);
                                    view! {
                                        <tr class=if row.is_group { "group" } else { "ledger" }>
                                            <td style=indent>{row.name}</td>
                                            <td class="secondary">{row.root_type.unwrap_or_default()}</td>
                                            <td class="secondary">{row.account_type.unwrap_or_default()}</td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_any()
            }
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: i64, name: &str, parent_id: Option<i64>) -> GroupAccount {
        GroupAccount {
            id,
            name: name.into(),
            parent_id,
            root_type: Some("Asset".into()),
            root_source: None,
            account_type: None,
        }
    }

    fn ledger(id: i64, name: &str, group_id: Option<i64>) -> LedgerAccount {
        LedgerAccount {
            id,
            name: name.into(),
            kind: "ledger".into(),
            group_id,
            root_type: None,
            account_type: None,
            effective_type: Some("Cash".into()),
            gst_head: None,
        }
    }

    #[test]
    fn subgroups_come_before_ledgers_and_orphans_last() {
        let chart = ChartOfAccounts {
            groups: vec![
                group(1, "Current Assets", None),
                group(2, "Bank Accounts", Some(1)),
                group(3, "Capital", None),
            ],
            ledgers: vec![
                ledger(10, "Cash", Some(1)),
                ledger(11, "HDFC", Some(2)),
                ledger(12, "Loose", None),
            ],
            unclassified_ledgers: 0,
        };
        let rows: Vec<(usize, String)> = flatten(&chart)
            .into_iter()
            .map(|r| (r.depth, r.name))
            .collect();
        assert_eq!(
            rows,
            vec![
                (0, "Capital".into()),
                (0, "Current Assets".into()),
                (1, "Bank Accounts".into()),
                (2, "HDFC".into()),
                (1, "Cash".into()),
                (0, "Loose".into()),
            ]
        );
    }

    #[test]
    fn a_parent_cycle_does_not_hang() {
        let chart = ChartOfAccounts {
            groups: vec![group(1, "A", Some(2)), group(2, "B", Some(1))],
            ledgers: vec![],
            unclassified_ledgers: 0,
        };
        let rows: Vec<(usize, String)> = flatten(&chart)
            .into_iter()
            .map(|r| (r.depth, r.name))
            .collect();
        assert_eq!(rows, vec![(0, "A".into()), (1, "B".into())]);
    }
}
