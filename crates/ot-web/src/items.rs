//! Items, item groups and units of measure (roadmap step 1.6).

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{Conversion, Item, ItemGroup, ItemInput, LedgerAccount, Tax, Unit};

use crate::api::{self, ApiError};

fn optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// The form's fields, one signal each. Ids are kept as text, as selects give them.
#[derive(Clone, Copy)]
struct Form {
    id: RwSignal<Option<i64>>,
    revision: RwSignal<Option<i32>>,
    name: RwSignal<String>,
    item_code: RwSignal<String>,
    group_id: RwSignal<String>,
    item_type: RwSignal<String>,
    usage: RwSignal<String>,
    unit: RwSignal<String>,
    rate: RwSignal<String>,
    hsn_code: RwSignal<String>,
    barcode: RwSignal<String>,
    description: RwSignal<String>,
    income: RwSignal<String>,
    expense: RwSignal<String>,
    tax: RwSignal<String>,
    conversions: RwSignal<Vec<(RwSignal<String>, RwSignal<String>)>>,
}

impl Form {
    fn new() -> Self {
        let text = || RwSignal::new(String::new());
        Self {
            id: RwSignal::new(None),
            revision: RwSignal::new(None),
            name: text(),
            item_code: text(),
            group_id: text(),
            item_type: RwSignal::new("product".into()),
            usage: RwSignal::new("both".into()),
            unit: RwSignal::new("Unit".into()),
            rate: text(),
            hsn_code: text(),
            barcode: text(),
            description: text(),
            income: text(),
            expense: text(),
            tax: text(),
            conversions: RwSignal::new(Vec::new()),
        }
    }

    fn load(&self, item: Option<&Item>) {
        let text = |v: Option<&String>| v.cloned().unwrap_or_default();
        let id_text = |v: Option<i64>| v.map(|id| id.to_string()).unwrap_or_default();
        self.id.set(item.map(|i| i.id));
        self.revision.set(item.map(|i| i.revision));
        self.name
            .set(item.map(|i| i.name.clone()).unwrap_or_default());
        self.item_code
            .set(text(item.and_then(|i| i.item_code.as_ref())));
        self.group_id.set(id_text(item.and_then(|i| i.group_id)));
        self.item_type
            .set(item.map_or_else(|| "product".into(), |i| i.item_type.clone()));
        self.usage
            .set(item.map_or_else(|| "both".into(), |i| i.usage.clone()));
        self.unit
            .set(item.map_or_else(|| "Unit".into(), |i| i.unit.clone()));
        self.rate
            .set(item.map(|i| i.rate.clone()).unwrap_or_default());
        self.hsn_code
            .set(text(item.and_then(|i| i.hsn_code.as_ref())));
        self.barcode
            .set(text(item.and_then(|i| i.barcode.as_ref())));
        self.description
            .set(text(item.and_then(|i| i.description.as_ref())));
        self.income.set(id_text(item.map(|i| i.income_ledger_id)));
        self.expense.set(id_text(item.map(|i| i.expense_ledger_id)));
        self.tax.set(id_text(item.and_then(|i| i.tax_id)));
        self.conversions.set(
            item.map(|i| {
                i.conversions
                    .iter()
                    .map(|c| {
                        (
                            RwSignal::new(c.unit.clone()),
                            RwSignal::new(c.factor.clone()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        );
    }

    fn input(&self) -> ItemInput {
        let get = |s: RwSignal<String>| s.get_untracked();
        let id = |s: RwSignal<String>| get(s).parse::<i64>().ok();
        ItemInput {
            name: get(self.name).trim().to_owned(),
            item_code: optional(get(self.item_code)),
            group_id: id(self.group_id),
            usage: get(self.usage),
            item_type: get(self.item_type),
            unit: optional(get(self.unit)),
            rate: optional(get(self.rate)),
            description: optional(get(self.description)),
            income_ledger_id: id(self.income),
            expense_ledger_id: id(self.expense),
            hsn_code: optional(get(self.hsn_code)),
            barcode: optional(get(self.barcode)),
            track_item: false,
            tax_id: id(self.tax),
            conversions: self
                .conversions
                .get_untracked()
                .into_iter()
                .filter(|(unit, _)| !unit.get_untracked().is_empty())
                .map(|(unit, factor)| Conversion {
                    unit: unit.get_untracked(),
                    factor: factor.get_untracked().trim().to_owned(),
                })
                .collect(),
            revision: self.revision.get_untracked(),
        }
    }
}

/// A select of ledgers; the empty choice lets the server pick the company default.
#[component]
fn LedgerSelect(
    label: &'static str,
    value: RwSignal<String>,
    ledgers: Signal<Vec<LedgerAccount>>,
) -> impl IntoView {
    view! {
        <label class="field">
            <span>{label}</span>
            <select aria-label=label bind:value=value>
                <option value="">"Company default"</option>
                {move || {
                    ledgers
                        .get()
                        .into_iter()
                        .map(|l| {
                            let key = l.id.to_string();
                            let selected = {
                                let key = key.clone();
                                move || value.get() == key
                            };
                            view! { <option value=key selected=selected>{l.name}</option> }
                        })
                        .collect_view()
                }}
            </select>
        </label>
    }
}

#[component]
pub fn ItemsView(on_signed_out: Callback<()>) -> impl IntoView {
    let rows = RwSignal::new(None::<Result<Vec<Item>, ApiError>>);
    let search = RwSignal::new(String::new());
    let message = RwSignal::new(None::<(bool, String)>);
    let editing = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let units = RwSignal::new(Vec::<Unit>::new());
    let groups = RwSignal::new(Vec::<ItemGroup>::new());
    let taxes = RwSignal::new(Vec::<Tax>::new());
    // Ledgers by root type; empty when the user may not read the chart of accounts.
    let ledgers = RwSignal::new(Vec::<LedgerAccount>::new());
    let form = Form::new();

    let reload = move || {
        let q = search.get_untracked();
        spawn_local(async move {
            let result = api::items(&q).await;
            if let Err(error) = &result
                && error.signed_out
            {
                on_signed_out.run(());
                return;
            }
            rows.set(Some(result));
        });
    };
    let reload_masters = move || {
        spawn_local(async move {
            if let Ok(list) = api::units().await {
                units.set(list);
            }
            if let Ok(list) = api::item_groups().await {
                groups.set(list);
            }
            if let Ok(list) = api::taxes().await {
                taxes.set(list);
            }
        });
    };
    reload();
    reload_masters();
    spawn_local(async move {
        if let Ok(chart) = api::chart_of_accounts().await {
            ledgers.set(chart.ledgers);
        }
    });
    let of_root = move |root: &'static str| {
        Signal::derive(move || {
            ledgers
                .get()
                .into_iter()
                .filter(|l| l.root_type.as_deref() == Some(root))
                .collect::<Vec<_>>()
        })
    };
    let income_ledgers = of_root("Income");
    let expense_ledgers = of_root("Expense");

    let report = move |result: Result<String, ApiError>, close: bool| match result {
        Ok(text) => {
            message.set(Some((false, text)));
            if close {
                editing.set(false);
                form.load(None);
            }
            reload();
            reload_masters();
        }
        Err(error) if error.signed_out => on_signed_out.run(()),
        Err(error) => message.set(Some((true, error.message))),
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let (id, input) = (form.id.get_untracked(), form.input());
        busy.set(true);
        spawn_local(async move {
            let result = api::save_item(id, &input).await;
            report(result.map(|item| format!("Saved {}.", item.name)), true);
            busy.set(false);
        });
    };
    let delete = move |_| {
        let Some(id) = form.id.get_untracked() else {
            return;
        };
        let name = form.name.get_untracked();
        spawn_local(async move {
            let result = api::delete_item(id).await;
            report(result.map(|()| format!("Deleted {name}.")), true);
        });
    };
    let open = move |item: Item| {
        form.load(Some(&item));
        message.set(None);
        editing.set(true);
    };
    let find = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        reload();
    };

    // The units and groups panel.
    let new_unit = RwSignal::new(String::new());
    let new_unit_whole = RwSignal::new(false);
    let new_group = RwSignal::new(String::new());
    let new_group_hsn = RwSignal::new(String::new());
    let add_unit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let (name, whole) = (new_unit.get_untracked(), new_unit_whole.get_untracked());
        spawn_local(async move {
            let result = api::add_unit(name.trim(), whole).await;
            if result.is_ok() {
                new_unit.set(String::new());
                new_unit_whole.set(false);
            }
            report(result.map(|u| format!("Added the unit {}.", u.name)), false);
        });
    };
    let add_group = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let (name, hsn) = (
            new_group.get_untracked(),
            optional(new_group_hsn.get_untracked()),
        );
        spawn_local(async move {
            let result = api::add_item_group(name.trim(), hsn).await;
            if result.is_ok() {
                new_group.set(String::new());
                new_group_hsn.set(String::new());
            }
            report(
                result.map(|g| format!("Added the item group {}.", g.name)),
                false,
            );
        });
    };

    let unit_options = move |value: RwSignal<String>| {
        units
            .get()
            .into_iter()
            .map(|u| {
                let key = u.name.clone();
                let selected = move || value.get() == key;
                view! { <option value=u.name.clone() selected=selected>{u.name.clone()}</option> }
            })
            .collect_view()
    };

    view! {
        <header class="page-heading with-action">
            <div>
                <h1>"Items"</h1>
                <p class="muted">"Goods and services you buy and sell. Sales and purchase accounts default to the company's Sales and Purchase ledgers."</p>
            </div>
            <button class="button primary" type="button" on:click=move |_| {
                form.load(None);
                message.set(None);
                editing.set(true);
            }>"New item"</button>
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
                <h2>{move || if form.id.get().is_some() { "Edit item" } else { "New item" }}</h2>
                <div class="field-row">
                    <label class="field">
                        <span>"Name"</span>
                        <input required autofocus bind:value=form.name/>
                    </label>
                    <label class="field">
                        <span>"Item code"</span>
                        <input bind:value=form.item_code/>
                    </label>
                </div>
                <div class="field-row three">
                    <label class="field">
                        <span>"Type"</span>
                        <select aria-label="Type" bind:value=form.item_type>
                            <option value="product">"Product"</option>
                            <option value="service">"Service"</option>
                        </select>
                    </label>
                    <label class="field">
                        <span>"Used for"</span>
                        <select aria-label="Used for" bind:value=form.usage>
                            <option value="both">"Sales and purchases"</option>
                            <option value="sales">"Sales"</option>
                            <option value="purchases">"Purchases"</option>
                        </select>
                    </label>
                    <label class="field">
                        <span>"Item group"</span>
                        <select aria-label="Item group" bind:value=form.group_id>
                            <option value="">"None"</option>
                            {move || {
                                groups
                                    .get()
                                    .into_iter()
                                    .map(|g| {
                                        let key = g.id.to_string();
                                        let selected = {
                                            let key = key.clone();
                                            move || form.group_id.get() == key
                                        };
                                        view! { <option value=key selected=selected>{g.name}</option> }
                                    })
                                    .collect_view()
                            }}
                        </select>
                    </label>
                </div>
                <div class="field-row three">
                    <label class="field">
                        <span>"Unit"</span>
                        <select aria-label="Unit" bind:value=form.unit>
                            {move || unit_options(form.unit)}
                        </select>
                    </label>
                    <label class="field">
                        <span>"Rate"</span>
                        <input inputmode="decimal" bind:value=form.rate placeholder="0.00"/>
                    </label>
                    <label class="field">
                        <span>"HSN/SAC code"</span>
                        <input inputmode="numeric" maxlength="8" bind:value=form.hsn_code placeholder="From the group if empty"/>
                    </label>
                </div>
                <label class="field">
                    <span>"Tax"</span>
                    <select aria-label="Tax" bind:value=form.tax>
                        <option value="">"None (use the item group's)"</option>
                        {move || {
                            taxes
                                .get()
                                .into_iter()
                                .map(|t| {
                                    let key = t.id.to_string();
                                    let selected = {
                                        let key = key.clone();
                                        move || form.tax.get() == key
                                    };
                                    view! { <option value=key selected=selected>{format!("{} ({}%)", t.name, t.total_rate)}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
                <Show when=move || !ledgers.get().is_empty()>
                    <div class="field-row">
                        <LedgerSelect label="Sales account" value=form.income ledgers=income_ledgers/>
                        <LedgerSelect label="Purchase account" value=form.expense ledgers=expense_ledgers/>
                    </div>
                </Show>
                <div class="field-row">
                    <label class="field">
                        <span>"Barcode"</span>
                        <input inputmode="numeric" maxlength="12" bind:value=form.barcode placeholder="12 digits"/>
                    </label>
                    <label class="field">
                        <span>"Description"</span>
                        <input bind:value=form.description/>
                    </label>
                </div>
                <fieldset class="address">
                    <legend>"Other units"</legend>
                    {move || {
                        form.conversions
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(index, (unit, factor))| view! {
                                <div class="conversion">
                                    <span>"1"</span>
                                    <select aria-label="Other unit" bind:value=unit>
                                        <option value="">"Choose a unit"</option>
                                        {move || unit_options(unit)}
                                    </select>
                                    <span>"="</span>
                                    <input aria-label="Conversion factor" inputmode="decimal" bind:value=factor/>
                                    <span>{move || form.unit.get()}</span>
                                    <button class="link-button danger" type="button" on:click=move |_| {
                                        form.conversions.update(|rows| { rows.remove(index); });
                                    }>"Remove"</button>
                                </div>
                            })
                            .collect_view()
                    }}
                    <button class="link-button" type="button" on:click=move |_| {
                        form.conversions.update(|rows| rows.push((RwSignal::new(String::new()), RwSignal::new("1".into()))));
                    }>"Add a unit"</button>
                </fieldset>
                <div class="form-actions">
                    <button class="button primary" type="submit" disabled=move || busy.get()>"Save item"</button>
                    <button class="button" type="button" on:click=move |_| editing.set(false)>"Cancel"</button>
                    <Show when=move || form.id.get().is_some()>
                        <button class="link-button danger" type="button" on:click=delete>"Delete"</button>
                    </Show>
                </div>
            </form>
        </Show>
        <div class="items-layout">
            <section class="items-main">
                <form class="filters party-filters items-filter" on:submit=find>
                    <label class="field grow">
                        <span>"Search"</span>
                        <input type="search" bind:value=search placeholder="Name, code, HSN or barcode"/>
                    </label>
                    <button class="button" type="submit">"Search"</button>
                </form>
                {move || match rows.get() {
                    None => view! { <p class="muted">"Loading items…"</p> }.into_any(),
                    Some(Err(error)) => view! { <p class="status error" role="alert">{error.message}</p> }.into_any(),
                    Some(Ok(list)) if list.is_empty() => {
                        let text = if search.get_untracked().trim().is_empty() {
                            "No items yet. Add what you buy and sell with New item."
                        } else {
                            "No items match this search."
                        };
                        view! { <p class="muted">{text}</p> }.into_any()
                    }
                    Some(Ok(list)) => view! {
                        <div class="table-wrap">
                            <table class="list-view party-list">
                                <thead>
                                    <tr>
                                        <th>"Name"</th>
                                        <th class="secondary">"Group"</th>
                                        <th class="num">"Rate"</th>
                                        <th>"Unit"</th>
                                        <th class="secondary">"HSN/SAC"</th>
                                        <th class="secondary">"Tax"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {list.into_iter().map(|item| {
                                        let row = item.clone();
                                        view! {
                                            <tr>
                                                <td>
                                                    <button class="link-button" type="button" on:click=move |_| open(row.clone())>
                                                        {item.name.clone()}
                                                    </button>
                                                </td>
                                                <td class="secondary">{item.group_name.clone().unwrap_or_default()}</td>
                                                <td class="num">{item.rate.clone()}</td>
                                                <td>{item.unit.clone()}</td>
                                                <td class="secondary">{item.hsn_code.clone().unwrap_or_default()}</td>
                                                <td class="secondary">{item.tax_name.clone().unwrap_or_default()}</td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any(),
                }}
            </section>
            <aside class="card masters">
                <h2>"Units"</h2>
                <p class="muted">{move || units.get().into_iter().map(|u| u.name).collect::<Vec<_>>().join(", ")}</p>
                <form class="mini-form" on:submit=add_unit>
                    <input aria-label="New unit" required bind:value=new_unit placeholder="New unit, e.g. Box"/>
                    <label class="check">
                        <input type="checkbox" bind:checked=new_unit_whole/>
                        <span>"Whole numbers only"</span>
                    </label>
                    <button class="button" type="submit">"Add unit"</button>
                </form>
                <h2>"Item groups"</h2>
                <ul class="group-list">
                    {move || groups.get().into_iter().map(|g| view! {
                        <li>{g.name}<span class="muted">{g.hsn_code.map(|h| format!(" HSN {h}")).unwrap_or_default()}</span></li>
                    }).collect_view()}
                </ul>
                <form class="mini-form" on:submit=add_group>
                    <input aria-label="New item group" required bind:value=new_group placeholder="New group, e.g. Grains"/>
                    <input aria-label="Group HSN code" inputmode="numeric" bind:value=new_group_hsn placeholder="HSN code (optional)"/>
                    <button class="button" type="submit">"Add group"</button>
                </form>
            </aside>
        </div>
    }
}
