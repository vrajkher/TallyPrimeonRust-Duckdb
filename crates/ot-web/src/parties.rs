//! Parties: customers and suppliers with their address (roadmap step 1.5).

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{Address, Party, PartyInput};

use crate::api::{self, ApiError};

fn optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn role_label(role: &str) -> &'static str {
    match role {
        "customer" => "Customer",
        "supplier" => "Supplier",
        _ => "Customer and supplier",
    }
}

/// The form's fields, one signal each.
#[derive(Clone, Copy)]
struct Form {
    id: RwSignal<Option<i64>>,
    revision: RwSignal<Option<i32>>,
    name: RwSignal<String>,
    role: RwSignal<String>,
    email: RwSignal<String>,
    phone: RwSignal<String>,
    gst_type: RwSignal<String>,
    gstin: RwSignal<String>,
    line1: RwSignal<String>,
    line2: RwSignal<String>,
    city: RwSignal<String>,
    state: RwSignal<String>,
    country: RwSignal<String>,
    postal_code: RwSignal<String>,
}

impl Form {
    fn new() -> Self {
        let text = || RwSignal::new(String::new());
        Self {
            id: RwSignal::new(None),
            revision: RwSignal::new(None),
            name: text(),
            role: RwSignal::new("customer".into()),
            email: text(),
            phone: text(),
            gst_type: RwSignal::new("unregistered".into()),
            gstin: text(),
            line1: text(),
            line2: text(),
            city: text(),
            state: text(),
            country: RwSignal::new("India".into()),
            postal_code: text(),
        }
    }

    fn clear(&self) {
        self.load(None);
    }

    fn load(&self, party: Option<&Party>) {
        let text = |v: Option<&String>| v.cloned().unwrap_or_default();
        let address = party.and_then(|p| p.address.as_ref());
        self.id.set(party.map(|p| p.id));
        self.revision.set(party.map(|p| p.revision));
        self.name
            .set(party.map(|p| p.name.clone()).unwrap_or_default());
        self.role
            .set(party.map_or_else(|| "customer".into(), |p| p.role.clone()));
        self.email.set(text(party.and_then(|p| p.email.as_ref())));
        self.phone.set(text(party.and_then(|p| p.phone.as_ref())));
        self.gst_type
            .set(party.map_or_else(|| "unregistered".into(), |p| p.gst_type.clone()));
        self.gstin.set(text(party.and_then(|p| p.gstin.as_ref())));
        self.line1
            .set(address.map(|a| a.address_line1.clone()).unwrap_or_default());
        self.line2
            .set(text(address.and_then(|a| a.address_line2.as_ref())));
        self.city
            .set(address.map(|a| a.city.clone()).unwrap_or_default());
        self.state.set(text(address.and_then(|a| a.state.as_ref())));
        self.country
            .set(address.map_or_else(|| "India".into(), |a| a.country.clone()));
        self.postal_code
            .set(text(address.and_then(|a| a.postal_code.as_ref())));
    }

    /// The input to send. An address is sent only once its first line is filled in.
    fn input(&self) -> PartyInput {
        let get = |s: RwSignal<String>| s.get_untracked();
        let address = optional(get(self.line1)).map(|line1| Address {
            address_line1: line1,
            address_line2: optional(get(self.line2)),
            city: get(self.city).trim().to_owned(),
            state: optional(get(self.state)),
            country: get(self.country),
            postal_code: optional(get(self.postal_code)),
            ..Default::default()
        });
        PartyInput {
            name: get(self.name).trim().to_owned(),
            role: get(self.role),
            email: optional(get(self.email)),
            phone: optional(get(self.phone)),
            address,
            gst_type: get(self.gst_type),
            gstin: optional(get(self.gstin)),
            revision: self.revision.get_untracked(),
            ..Default::default()
        }
    }
}

#[component]
pub fn PartiesView(on_signed_out: Callback<()>) -> impl IntoView {
    let rows = RwSignal::new(None::<Result<Vec<Party>, ApiError>>);
    let role_filter = RwSignal::new(String::new());
    let search = RwSignal::new(String::new());
    let message = RwSignal::new(None::<(bool, String)>);
    let editing = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let countries = RwSignal::new(Vec::<String>::new());
    let states = RwSignal::new(Vec::<String>::new());
    let form = Form::new();

    let reload = move || {
        let (role, q) = (role_filter.get_untracked(), search.get_untracked());
        spawn_local(async move {
            let result = api::parties(&role, &q).await;
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
        if let Ok(list) = api::countries().await {
            countries.set(list);
        }
        if let Ok(list) = api::indian_states().await {
            states.set(list);
        }
    });

    let report = move |result: Result<String, ApiError>| match result {
        Ok(text) => {
            message.set(Some((false, text)));
            editing.set(false);
            form.clear();
            reload();
        }
        Err(error) if error.signed_out => on_signed_out.run(()),
        Err(error) => message.set(Some((true, error.message))),
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let (id, input) = (form.id.get_untracked(), form.input());
        busy.set(true);
        spawn_local(async move {
            let result = api::save_party(id, &input).await;
            report(result.map(|party| format!("Saved {}.", party.name)));
            busy.set(false);
        });
    };
    let delete = move |_| {
        let Some(id) = form.id.get_untracked() else {
            return;
        };
        let name = form.name.get_untracked();
        spawn_local(async move {
            let result = api::delete_party(id).await;
            report(result.map(|()| format!("Deleted {name}.")));
        });
    };
    let filter = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        reload();
    };
    let open = move |party: Party| {
        form.load(Some(&party));
        message.set(None);
        editing.set(true);
    };

    let registered = move || form.gst_type.get() == "registered_regular";
    let in_india = move || form.country.get() == "India";

    view! {
        <header class="page-heading with-action">
            <div>
                <h1>"Parties"</h1>
                <p class="muted">"Customers and suppliers. Each takes its default account and currency from the company settings unless you choose others."</p>
            </div>
            <button class="button primary" type="button" on:click=move |_| {
                form.clear();
                message.set(None);
                editing.set(true);
            }>"New party"</button>
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
                <h2>{move || if form.id.get().is_some() { "Edit party" } else { "New party" }}</h2>
                <div class="field-row">
                    <label class="field">
                        <span>"Name"</span>
                        <input required autofocus bind:value=form.name/>
                    </label>
                    <label class="field">
                        <span>"Role"</span>
                        <select aria-label="Role" bind:value=form.role>
                            <option value="customer">"Customer"</option>
                            <option value="supplier">"Supplier"</option>
                            <option value="both">"Customer and supplier"</option>
                        </select>
                    </label>
                </div>
                <div class="field-row">
                    <label class="field">
                        <span>"Email"</span>
                        <input type="email" bind:value=form.email/>
                    </label>
                    <label class="field">
                        <span>"Phone"</span>
                        <input type="tel" bind:value=form.phone/>
                    </label>
                </div>
                <div class="field-row">
                    <label class="field">
                        <span>"GST registration"</span>
                        <select aria-label="GST registration" bind:value=form.gst_type>
                            <option value="unregistered">"Unregistered"</option>
                            <option value="registered_regular">"Registered regular"</option>
                            <option value="consumer">"Consumer"</option>
                        </select>
                    </label>
                    <Show when=registered>
                        <label class="field">
                            <span>"GSTIN"</span>
                            <input required maxlength="15" bind:value=form.gstin placeholder="e.g. 24AAPFU0939F1ZV"/>
                        </label>
                    </Show>
                </div>
                <fieldset class="address">
                    <legend>"Address (optional)"</legend>
                    <label class="field">
                        <span>"Address line 1"</span>
                        <input bind:value=form.line1/>
                    </label>
                    <label class="field">
                        <span>"Address line 2"</span>
                        <input bind:value=form.line2/>
                    </label>
                    <div class="field-row">
                        <label class="field">
                            <span>"City"</span>
                            <input bind:value=form.city required=move || !form.line1.get().trim().is_empty()/>
                        </label>
                        <label class="field">
                            <span>"Postal code"</span>
                            <input bind:value=form.postal_code/>
                        </label>
                    </div>
                    <div class="field-row">
                        <label class="field">
                            <span>"State"</span>
                            <input bind:value=form.state list=move || in_india().then_some("indian-states")/>
                            <datalist id="indian-states">
                                {move || states.get().into_iter().map(|s| view! { <option value=s/> }).collect_view()}
                            </datalist>
                        </label>
                        <label class="field">
                            <span>"Country"</span>
                            <select aria-label="Country" bind:value=form.country>
                                {move || {
                                    countries
                                        .get()
                                        .into_iter()
                                        .map(|name| {
                                            let key = name.clone();
                                            let selected = move || form.country.get() == key;
                                            view! { <option value=name.clone() selected=selected>{name.clone()}</option> }
                                        })
                                        .collect_view()
                                }}
                            </select>
                        </label>
                    </div>
                </fieldset>
                <div class="form-actions">
                    <button class="button primary" type="submit" disabled=move || busy.get()>"Save party"</button>
                    <button class="button" type="button" on:click=move |_| editing.set(false)>"Cancel"</button>
                    <Show when=move || form.id.get().is_some()>
                        <button class="link-button danger" type="button" on:click=delete>"Delete"</button>
                    </Show>
                </div>
            </form>
        </Show>
        <form class="filters party-filters" on:submit=filter>
            <label class="field">
                <span>"Role"</span>
                <select aria-label="Show parties by role" bind:value=role_filter on:change=move |_| reload()>
                    <option value="">"All parties"</option>
                    <option value="customer">"Customers"</option>
                    <option value="supplier">"Suppliers"</option>
                </select>
            </label>
            <label class="field grow">
                <span>"Search"</span>
                <input type="search" bind:value=search placeholder="Name, email, phone or GSTIN"/>
            </label>
            <button class="button" type="submit">"Search"</button>
        </form>
        {move || match rows.get() {
            None => view! { <p class="muted">"Loading parties…"</p> }.into_any(),
            Some(Err(error)) => view! { <p class="status error" role="alert">{error.message}</p> }.into_any(),
            Some(Ok(list)) if list.is_empty() => {
                let filtered = !role_filter.get_untracked().is_empty()
                    || !search.get_untracked().trim().is_empty();
                let text = if filtered {
                    "No parties match this filter."
                } else {
                    "No parties yet. Add your customers and suppliers with New party."
                };
                view! { <p class="muted">{text}</p> }.into_any()
            }
            Some(Ok(list)) => view! {
                <div class="table-wrap">
                    <table class="list-view party-list">
                        <thead>
                            <tr>
                                <th>"Name"</th>
                                <th>"Role"</th>
                                <th class="secondary">"GSTIN"</th>
                                <th class="secondary">"Place of supply"</th>
                                <th class="secondary">"Default account"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {list.into_iter().map(|party| {
                                let place = party
                                    .address
                                    .as_ref()
                                    .and_then(|a| a.place_of_supply.clone())
                                    .unwrap_or_default();
                                let row = party.clone();
                                view! {
                                    <tr>
                                        <td>
                                            <button class="link-button" type="button" on:click=move |_| open(row.clone())>
                                                {party.name.clone()}
                                            </button>
                                        </td>
                                        <td>{role_label(&party.role)}</td>
                                        <td class="secondary">{party.gstin.clone().unwrap_or_default()}</td>
                                        <td class="secondary">{place}</td>
                                        <td class="secondary">{party.default_ledger_name.clone().unwrap_or_default()}</td>
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
