//! The company setup wizard (roadmap step 1.3), shown once to a System Manager.

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::CompanySetup;

use crate::api;

fn optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// Fills the country's suggestions into the form, as Frappe Books' wizard does.
fn suggest(
    country: String,
    currency: RwSignal<String>,
    chart: RwSignal<String>,
    start: RwSignal<String>,
    end: RwSignal<String>,
) {
    spawn_local(async move {
        if let Ok(defaults) = api::country_defaults(&country).await {
            currency.set(defaults.currency.unwrap_or_default());
            chart.set(defaults.chart);
            start.set(defaults.fiscal_year_start);
            end.set(defaults.fiscal_year_end);
        }
    });
}

#[component]
pub fn SetupWizard(on_done: Callback<()>) -> impl IntoView {
    let company = RwSignal::new(String::new());
    let country = RwSignal::new("India".to_owned());
    let currency = RwSignal::new(String::new());
    let chart = RwSignal::new(String::new());
    let start = RwSignal::new(String::new());
    let end = RwSignal::new(String::new());
    let bank = RwSignal::new(String::new());
    let gstin = RwSignal::new(String::new());
    let countries = RwSignal::new(Vec::<String>::new());
    let charts = RwSignal::new(Vec::<String>::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    spawn_local(async move {
        if let Ok(list) = api::countries().await {
            countries.set(list);
        }
        if let Ok(list) = api::chart_templates().await {
            charts.set(list.into_iter().map(|c| c.name).collect());
        }
    });
    suggest("India".into(), currency, chart, start, end);

    let on_country = move |event: leptos::ev::Event| {
        let value = event_target_value(&event);
        country.set(value.clone());
        suggest(value, currency, chart, start, end);
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let input = CompanySetup {
            company_name: company.get_untracked(),
            country: country.get_untracked(),
            bank_name: bank.get_untracked(),
            currency: optional(currency.get_untracked()),
            chart: optional(chart.get_untracked()),
            fiscal_year_start: optional(start.get_untracked()),
            fiscal_year_end: optional(end.get_untracked()),
            gstin: optional(gstin.get_untracked()),
        };
        busy.set(true);
        spawn_local(async move {
            match api::set_up_company(&input).await {
                Ok(_) => on_done.run(()),
                Err(failure) => error.set(Some(failure.message)),
            }
            busy.set(false);
        });
    };

    view! {
        <div class="centered">
            <form class="card form-view wide" on:submit=submit>
                <h1>"Set up your company"</h1>
                <p class="muted">
                    "This creates the chart of accounts, your bank ledger and the default accounts. You can do this once; the choices for the country are filled in for you."
                </p>
                <label class="field">
                    <span>"Company name"</span>
                    <input required autofocus bind:value=company autocomplete="organization"/>
                </label>
                <div class="field-row">
                    <label class="field">
                        <span>"Country"</span>
                        <select aria-label="Country" on:change=on_country prop:value=move || country.get()>
                            {move || {
                                countries
                                    .get()
                                    .into_iter()
                                    .map(|name| {
                                        let selected = name == country.get_untracked();
                                        view! { <option value=name.clone() selected=selected>{name.clone()}</option> }
                                    })
                                    .collect_view()
                            }}
                        </select>
                    </label>
                    <label class="field">
                        <span>"Currency"</span>
                        <input required maxlength="3" bind:value=currency/>
                    </label>
                </div>
                <label class="field">
                    <span>"Chart of accounts"</span>
                    <select aria-label="Chart of accounts" bind:value=chart>
                        {move || {
                            charts
                                .get()
                                .into_iter()
                                .map(|name| {
                                    // Options may arrive after the suggestion, so mark it here too.
                                    let key = name.clone();
                                    let selected = move || chart.get() == key;
                                    view! { <option value=name.clone() selected=selected>{name.clone()}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
                <div class="field-row">
                    <label class="field">
                        <span>"Fiscal year starts"</span>
                        <input type="date" required bind:value=start/>
                    </label>
                    <label class="field">
                        <span>"Fiscal year ends"</span>
                        <input type="date" required bind:value=end/>
                    </label>
                </div>
                <div class="field-row">
                    <label class="field">
                        <span>"Bank name"</span>
                        <input required bind:value=bank placeholder="e.g. HDFC Bank"/>
                    </label>
                    <label class="field">
                        <span>"GSTIN (optional)"</span>
                        <input maxlength="15" bind:value=gstin/>
                    </label>
                </div>
                <Show when=move || error.get().is_some()>
                    <p class="status error" role="alert">{move || error.get().unwrap_or_default()}</p>
                </Show>
                <button class="button primary" type="submit" disabled=move || { busy.get() }>
                    "Set up company"
                </button>
            </form>
        </div>
    }
}
