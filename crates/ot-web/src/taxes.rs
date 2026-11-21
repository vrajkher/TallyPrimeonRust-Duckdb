//! Tax templates (roadmap step 1.7), e.g. GST-18 as CGST 9% + SGST 9%.

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{LedgerAccount, Tax, TaxDetail, TaxInput};

use crate::api::{self, ApiError};

/// One editable row: ledger id, payment ledger id (both as select text) and rate.
type Row = (RwSignal<String>, RwSignal<String>, RwSignal<String>);

fn new_row(ledger: String, payment: String, rate: String) -> Row {
    (
        RwSignal::new(ledger),
        RwSignal::new(payment),
        RwSignal::new(rate),
    )
}

#[component]
pub fn TaxesView(on_signed_out: Callback<()>) -> impl IntoView {
    let rows = RwSignal::new(None::<Result<Vec<Tax>, ApiError>>);
    let ledgers = RwSignal::new(Vec::<LedgerAccount>::new());
    let message = RwSignal::new(None::<(bool, String)>);
    let editing = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let id = RwSignal::new(None::<i64>);
    let revision = RwSignal::new(None::<i32>);
    let name = RwSignal::new(String::new());
    let details = RwSignal::new(Vec::<Row>::new());

    let reload = move || {
        spawn_local(async move {
            let result = api::taxes().await;
            if let Err(error) = &result
                && error.signed_out
            {
                on_signed_out.run(());
                return;
            }
            rows.set(Some(result));
        });
    };
    reload();
    spawn_local(async move {
        if let Ok(chart) = api::chart_of_accounts().await {
            ledgers.set(chart.ledgers);
        }
    });

    let report = move |result: Result<String, ApiError>| match result {
        Ok(text) => {
            message.set(Some((false, text)));
            editing.set(false);
            reload();
        }
        Err(error) if error.signed_out => on_signed_out.run(()),
        Err(error) => message.set(Some((true, error.message))),
    };

    let open = move |tax: Option<Tax>| {
        id.set(tax.as_ref().map(|t| t.id));
        revision.set(tax.as_ref().map(|t| t.revision));
        name.set(tax.as_ref().map(|t| t.name.clone()).unwrap_or_default());
        details.set(match &tax {
            Some(tax) => tax
                .details
                .iter()
                .map(|d| {
                    new_row(
                        d.ledger_id.to_string(),
                        d.payment_ledger_id
                            .map(|p| p.to_string())
                            .unwrap_or_default(),
                        d.rate.clone(),
                    )
                })
                .collect(),
            None => vec![new_row(String::new(), String::new(), String::new())],
        });
        message.set(None);
        editing.set(true);
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let input = TaxInput {
            name: name.get_untracked().trim().to_owned(),
            details: details
                .get_untracked()
                .into_iter()
                .map(|(ledger, payment, rate)| TaxDetail {
                    ledger_id: ledger.get_untracked().parse().unwrap_or_default(),
                    payment_ledger_id: payment.get_untracked().parse().ok(),
                    rate: rate.get_untracked().trim().to_owned(),
                    ..Default::default()
                })
                .collect(),
            revision: revision.get_untracked(),
        };
        let target = id.get_untracked();
        busy.set(true);
        spawn_local(async move {
            let result = api::save_tax(target, &input).await;
            report(result.map(|tax| format!("Saved {} ({}%).", tax.name, tax.total_rate)));
            busy.set(false);
        });
    };
    let delete = move |_| {
        let Some(target) = id.get_untracked() else {
            return;
        };
        let label = name.get_untracked();
        spawn_local(async move {
            let result = api::delete_tax(target).await;
            report(result.map(|()| format!("Deleted {label}.")));
        });
    };
    let add_gst = move |_| {
        spawn_local(async move {
            let before = match rows.get_untracked() {
                Some(Ok(list)) => list.len(),
                _ => 0,
            };
            let result = api::add_gst_taxes().await;
            report(
                result
                    .map(|list| format!("Added {} GST taxes.", list.len().saturating_sub(before))),
            );
        });
    };

    let ledger_options = move |value: RwSignal<String>, blank: &'static str| {
        let mut list = ledgers.get();
        // Tax ledgers first, as a tax row usually posts to one.
        list.sort_by_key(|l| {
            (
                l.effective_type.as_deref() != Some("Tax"),
                l.name.to_lowercase(),
            )
        });
        let options = list
            .into_iter()
            .map(|l| {
                let key = l.id.to_string();
                let selected = {
                    let key = key.clone();
                    move || value.get() == key
                };
                view! { <option value=key selected=selected>{l.name}</option> }
            })
            .collect_view();
        view! {
            <option value="">{blank}</option>
            {options}
        }
    };

    view! {
        <header class="page-heading with-action">
            <div>
                <h1>"Taxes"</h1>
                <p class="muted">"Tax templates for items and invoices. Each row posts a percentage of the amount to a ledger; GST-18 is CGST 9% and SGST 9%."</p>
            </div>
            <div class="form-actions">
                <button class="button" type="button" on:click=add_gst>"Add GST taxes"</button>
                <button class="button primary" type="button" on:click=move |_| open(None)>"New tax"</button>
            </div>
        </header>
        <Show when=move || message.get().is_some()>
            {move || {
                let (error, text) = message.get().unwrap_or_default();
                view! {
                    <p class=if error { "status error" } else { "status notice" } role=if error { "alert" } else { "status" }>
                        {text}
                    </p>
                }
            }}
        </Show>
        <Show when=move || editing.get()>
            <form class="card party-form" on:submit=submit>
                <h2>{move || if id.get().is_some() { "Edit tax" } else { "New tax" }}</h2>
                <label class="field">
                    <span>"Name"</span>
                    <input required autofocus bind:value=name placeholder="e.g. GST-18"/>
                </label>
                <fieldset class="address">
                    <legend>"Rows"</legend>
                    {move || {
                        details
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(index, (ledger, payment, rate))| view! {
                                <div class="conversion tax-row">
                                    <select aria-label="Tax account" required bind:value=ledger>
                                        {move || ledger_options(ledger, "Choose an account")}
                                    </select>
                                    <input aria-label="Rate" required inputmode="decimal" bind:value=rate placeholder="Rate"/>
                                    <span>"%"</span>
                                    <select aria-label="Payment account" bind:value=payment>
                                        {move || ledger_options(payment, "Paid from the same account")}
                                    </select>
                                    <button class="link-button danger" type="button" on:click=move |_| {
                                        details.update(|rows| { rows.remove(index); });
                                    }>"Remove"</button>
                                </div>
                            })
                            .collect_view()
                    }}
                    <button class="link-button" type="button" on:click=move |_| {
                        details.update(|rows| rows.push(new_row(String::new(), String::new(), String::new())));
                    }>"Add a row"</button>
                </fieldset>
                <div class="form-actions">
                    <button class="button primary" type="submit" disabled=move || busy.get()>"Save tax"</button>
                    <button class="button" type="button" on:click=move |_| editing.set(false)>"Cancel"</button>
                    <Show when=move || id.get().is_some()>
                        <button class="link-button danger" type="button" on:click=delete>"Delete"</button>
                    </Show>
                </div>
            </form>
        </Show>
        {move || match rows.get() {
            None => view! { <p class="muted">"Loading taxes…"</p> }.into_any(),
            Some(Err(error)) => view! { <p class="status error" role="alert">{error.message}</p> }.into_any(),
            Some(Ok(list)) if list.is_empty() => view! {
                <p class="muted">"No taxes yet. For an Indian company, Add GST taxes adds GST, IGST and exempt taxes at each rate."</p>
            }.into_any(),
            Some(Ok(list)) => view! {
                <div class="table-wrap">
                    <table class="list-view party-list">
                        <thead>
                            <tr>
                                <th>"Name"</th>
                                <th>"Rows"</th>
                                <th class="num">"Total"</th>
                                <th class="num secondary">"Used by"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {list.into_iter().map(|tax| {
                                let summary = tax
                                    .details
                                    .iter()
                                    .map(|d| format!("{} {}%", d.ledger_name, d.rate))
                                    .collect::<Vec<_>>()
                                    .join(" + ");
                                let row = tax.clone();
                                view! {
                                    <tr>
                                        <td>
                                            <button class="link-button" type="button" on:click=move |_| open(Some(row.clone()))>
                                                {tax.name.clone()}
                                            </button>
                                        </td>
                                        <td>{summary}</td>
                                        <td class="num">{format!("{}%", tax.total_rate)}</td>
                                        <td class="num secondary">{tax.used_by}</td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                </div>
            }.into_any(),
        }}
    }
}
