//! Payment methods and exchange rates (roadmap step 1.8).

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{ExchangeRate, LedgerAccount, PaymentMethod, PaymentMethodInput};

use crate::api::{self, ApiError};

fn type_label(key: &str) -> &'static str {
    match key {
        "bank" => "Bank",
        "transfer" => "Transfer",
        _ => "Cash",
    }
}

/// Today's date as YYYY-MM-DD, from the browser's clock.
fn today() -> String {
    let now = js_sys::Date::new_0();
    format!(
        "{:04}-{:02}-{:02}",
        now.get_full_year(),
        now.get_month() + 1,
        now.get_date()
    )
}

#[component]
pub fn MoneyView(on_signed_out: Callback<()>) -> impl IntoView {
    let methods = RwSignal::new(None::<Result<Vec<PaymentMethod>, ApiError>>);
    let rates = RwSignal::new(None::<Result<Vec<ExchangeRate>, ApiError>>);
    let ledgers = RwSignal::new(Vec::<LedgerAccount>::new());
    let currencies = RwSignal::new(Vec::<String>::new());
    let message = RwSignal::new(None::<(bool, String)>);

    // Fetches both lists, then shows them together with `done`, so a message never
    // describes a change the lists do not show yet.
    let reload = move |done: Option<String>| {
        spawn_local(async move {
            let result = api::payment_methods().await;
            if let Err(error) = &result
                && error.signed_out
            {
                on_signed_out.run(());
                return;
            }
            let fetched = api::exchange_rates().await;
            methods.set(Some(result));
            rates.set(Some(fetched));
            if let Some(text) = done {
                message.set(Some((false, text)));
            }
        });
    };
    reload(None);

    // The method form.
    let name = RwSignal::new(String::new());
    let kind = RwSignal::new("bank".to_owned());
    let ledger = RwSignal::new(String::new());
    let clearance = RwSignal::new(false);
    // The rate form: one `from` is `rate` of `to` on `date`.
    let from = RwSignal::new("USD".to_owned());
    let to = RwSignal::new(String::new());
    let date = RwSignal::new(today());
    let rate = RwSignal::new(String::new());

    spawn_local(async move {
        if let Ok(chart) = api::chart_of_accounts().await {
            ledgers.set(chart.ledgers);
        }
        if let Ok(list) = api::currencies().await {
            currencies.set(list);
        }
        let company = api::accounting_settings()
            .await
            .ok()
            .and_then(|s| s["currency"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "INR".into());
        to.set(company);
    });

    let report = move |result: Result<String, ApiError>| match result {
        Ok(text) => reload(Some(text)),
        Err(error) if error.signed_out => on_signed_out.run(()),
        Err(error) => message.set(Some((true, error.message))),
    };

    let add_method = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let input = PaymentMethodInput {
            name: name.get_untracked().trim().to_owned(),
            method_type: kind.get_untracked(),
            ledger_id: ledger.get_untracked().parse().ok(),
            requires_clearance_date: clearance.get_untracked(),
            revision: None,
        };
        spawn_local(async move {
            let result = api::add_payment_method(&input).await;
            if result.is_ok() {
                name.set(String::new());
                ledger.set(String::new());
                clearance.set(false);
            }
            report(result.map(|m| format!("Added the payment method {}.", m.name)));
        });
    };
    let add_rate = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let input = ExchangeRate {
            from_currency: from.get_untracked(),
            to_currency: to.get_untracked(),
            date: date.get_untracked(),
            rate: rate.get_untracked(),
        };
        spawn_local(async move {
            let result = api::set_exchange_rate(&input).await;
            if result.is_ok() {
                rate.set(String::new());
            }
            report(result.map(|r| {
                format!(
                    "Recorded 1 {} = {} {} on {}.",
                    r.from_currency, r.rate, r.to_currency, r.date
                )
            }));
        });
    };

    // Ledgers a method of the chosen type may use: Cash for cash, Cash or Bank otherwise.
    let method_ledgers = move || {
        let cash_only = kind.get() == "cash";
        ledgers
            .get()
            .into_iter()
            .filter(|l| match l.effective_type.as_deref() {
                Some("Cash") => true,
                Some("Bank") => !cash_only,
                _ => false,
            })
            .map(|l| view! { <option value=l.id.to_string()>{l.name}</option> })
            .collect_view()
    };

    view! {
        <header class="page-heading">
            <h1>"Payment methods and exchange rates"</h1>
            <p class="muted">"How payments are made, and the rates used for documents in another currency."</p>
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
        <section class="card masters-section">
            <h2>"Payment methods"</h2>
            {move || match methods.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(error)) => view! { <p class="status error">{error.message}</p> }.into_any(),
                Some(Ok(list)) => view! {
                    <div class="table-wrap">
                        <table class="list-view party-list">
                            <thead><tr><th>"Name"</th><th>"Type"</th><th>"Account"</th><th class="secondary">"Clearance date"</th><th><span class="visually-hidden">"Actions"</span></th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|m| {
                                    let (id, label) = (m.id, m.name.clone());
                                    view! {
                                        <tr>
                                            <td>{m.name}</td>
                                            <td>{type_label(&m.method_type)}</td>
                                            <td>{m.ledger_name.unwrap_or_else(|| "Chosen on the payment".into())}</td>
                                            <td class="secondary">{if m.requires_clearance_date { "Required" } else { "" }}</td>
                                            <td class="num">
                                                <button class="link-button danger" type="button" on:click=move |_| {
                                                    let label = label.clone();
                                                    spawn_local(async move {
                                                        let result = api::delete_payment_method(id).await;
                                                        report(result.map(|()| format!("Deleted {label}.")));
                                                    });
                                                }>"Delete"</button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_any(),
            }}
            <form class="inline-form" on:submit=add_method>
                <label class="field">
                    <span>"Name"</span>
                    <input required bind:value=name placeholder="e.g. UPI"/>
                </label>
                <label class="field">
                    <span>"Type"</span>
                    <select aria-label="Type" bind:value=kind>
                        <option value="cash">"Cash"</option>
                        <option value="bank">"Bank"</option>
                        <option value="transfer">"Transfer"</option>
                    </select>
                </label>
                <label class="field">
                    <span>"Account"</span>
                    <select aria-label="Account" bind:value=ledger>
                        <option value="">"Chosen on the payment"</option>
                        {method_ledgers}
                    </select>
                </label>
                <label class="check">
                    <input type="checkbox" bind:checked=clearance/>
                    <span>"Needs a clearance date"</span>
                </label>
                <button class="button primary" type="submit">"Add method"</button>
            </form>
        </section>
        <section class="card masters-section">
            <h2>"Exchange rates"</h2>
            <p class="muted">"A document in another currency uses the latest rate on or before its date. Enter one pair and the opposite is worked out."</p>
            <form class="inline-form rate-form" on:submit=add_rate>
                <span>"1"</span>
                <label class="field">
                    <span>"From"</span>
                    <input aria-label="From currency" required maxlength="3" list="currency-codes" bind:value=from/>
                </label>
                <span>"="</span>
                <label class="field">
                    <span>"Rate"</span>
                    <input required inputmode="decimal" bind:value=rate placeholder="e.g. 83.25"/>
                </label>
                <label class="field">
                    <span>"To"</span>
                    <input aria-label="To currency" required maxlength="3" list="currency-codes" bind:value=to/>
                </label>
                <label class="field">
                    <span>"On"</span>
                    <input type="date" required bind:value=date/>
                </label>
                <datalist id="currency-codes">
                    {move || currencies.get().into_iter().map(|c| view! { <option value=c/> }).collect_view()}
                </datalist>
                <button class="button primary" type="submit">"Save rate"</button>
            </form>
            {move || match rates.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(error)) => view! { <p class="status error">{error.message}</p> }.into_any(),
                Some(Ok(list)) if list.is_empty() => view! { <p class="muted">"No rates yet."</p> }.into_any(),
                Some(Ok(list)) => view! {
                    <div class="table-wrap">
                        <table class="list-view party-list">
                            <thead><tr><th>"Date"</th><th>"Rate"</th><th><span class="visually-hidden">"Actions"</span></th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|r| {
                                    let text = format!("1 {} = {} {}", r.from_currency, r.rate, r.to_currency);
                                    let row = r.clone();
                                    view! {
                                        <tr>
                                            <td>{r.date}</td>
                                            <td>{text}</td>
                                            <td class="num">
                                                <button class="link-button danger" type="button" on:click=move |_| {
                                                    let row = row.clone();
                                                    spawn_local(async move {
                                                        let result = api::delete_exchange_rate(&row).await;
                                                        report(result.map(|()| format!("Deleted the {} rate of {}.", row.from_currency, row.date)));
                                                    });
                                                }>"Delete"</button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_any(),
            }}
        </section>
    }
}
