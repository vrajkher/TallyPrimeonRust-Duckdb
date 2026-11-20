//! Number Series: the prefixes that name documents (roadmap step 1.4).

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{NewNumberSeries, NumberSeries, SeriesType};

use crate::api::{self, ApiError};

#[component]
pub fn SeriesView(on_signed_out: Callback<()>) -> impl IntoView {
    let rows = RwSignal::new(None::<Result<Vec<NumberSeries>, ApiError>>);
    let types = RwSignal::new(Vec::<SeriesType>::new());
    let message = RwSignal::new(None::<(bool, String)>);

    let reload = move || {
        spawn_local(async move {
            let result = api::number_series().await;
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
        if let Ok(list) = api::series_types().await {
            types.set(list);
        }
    });

    // The add form.
    let prefix = RwSignal::new(String::new());
    let kind = RwSignal::new("sales_invoice".to_owned());
    let start = RwSignal::new(String::new());
    let pad = RwSignal::new(String::new());
    let make_default = RwSignal::new(false);
    let busy = RwSignal::new(false);

    let report = move |result: Result<String, ApiError>| match result {
        Ok(text) => {
            message.set(Some((false, text)));
            reload();
        }
        Err(error) if error.signed_out => on_signed_out.run(()),
        Err(error) => message.set(Some((true, error.message))),
    };

    let submit =
        move |event: leptos::ev::SubmitEvent| {
            event.prevent_default();
            let input = NewNumberSeries {
                prefix: prefix.get_untracked().trim().to_owned(),
                reference_type: kind.get_untracked(),
                start: start.get_untracked().trim().parse().ok(),
                pad_zeros: pad.get_untracked().trim().parse().ok(),
                make_default: make_default.get_untracked(),
            };
            busy.set(true);
            spawn_local(async move {
                let result = api::add_number_series(&input).await;
                if result.is_ok() {
                    prefix.set(String::new());
                    start.set(String::new());
                    pad.set(String::new());
                    make_default.set(false);
                }
                report(result.map(|row| {
                    format!("Added {}; the next name is {}.", row.prefix, row.next_name)
                }));
                busy.set(false);
            });
        };

    let set_default = move |prefix: String| {
        spawn_local(async move {
            let result = api::make_default_series(&prefix).await;
            report(result.map(|row| {
                format!(
                    "{} is now the default {} series.",
                    row.prefix, row.reference_label
                )
            }));
        });
    };
    let delete = move |prefix: String| {
        spawn_local(async move {
            let result = api::delete_number_series(&prefix).await;
            report(result.map(|()| format!("Deleted {prefix}.")));
        });
    };

    view! {
        <header class="page-heading">
            <h1>"Number Series"</h1>
            <p class="muted">
                "Each document type takes its names from its default series, e.g. SINV-1001. Numbers already given out are never reused."
            </p>
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
        <form class="card series-form" on:submit=submit>
            <label class="field">
                <span>"Prefix"</span>
                <input required bind:value=prefix placeholder="e.g. GST-26-"/>
            </label>
            <label class="field">
                <span>"Document type"</span>
                <select aria-label="Document type" bind:value=kind>
                    {move || {
                        types
                            .get()
                            .into_iter()
                            .map(|t| {
                                let selected = t.key == kind.get_untracked();
                                view! { <option value=t.key.clone() selected=selected>{t.label.clone()}</option> }
                            })
                            .collect_view()
                    }}
                </select>
            </label>
            <label class="field">
                <span>"Start"</span>
                <input type="number" min="1" bind:value=start placeholder="1001"/>
            </label>
            <label class="field">
                <span>"Digits"</span>
                <input type="number" min="1" max="12" bind:value=pad placeholder="4"/>
            </label>
            <label class="check">
                <input type="checkbox" bind:checked=make_default/>
                <span>"Make default"</span>
            </label>
            <button class="button primary" type="submit" disabled=move || busy.get()>"Add series"</button>
        </form>
        {move || match rows.get() {
            None => view! { <p class="muted">"Loading number series…"</p> }.into_any(),
            Some(Err(error)) => view! { <p class="status error" role="alert">{error.message}</p> }.into_any(),
            Some(Ok(list)) => view! {
                <div class="table-wrap">
                    <table class="list-view series-list">
                        <thead>
                            <tr>
                                <th>"Document type"</th>
                                <th>"Prefix"</th>
                                <th>"Next name"</th>
                                <th>"Default"</th>
                                <th><span class="visually-hidden">"Actions"</span></th>
                            </tr>
                        </thead>
                        <tbody>
                            {list.into_iter().map(|row| {
                                let for_default = row.prefix.clone();
                                let for_delete = row.prefix.clone();
                                let deletable = !row.used && !row.is_default;
                                let is_default = row.is_default;
                                view! {
                                    <tr>
                                        <td>{row.reference_label}</td>
                                        <td><code>{row.prefix}</code></td>
                                        <td>{row.next_name}</td>
                                        <td>{if is_default { "Yes" } else { "" }}</td>
                                        <td><div class="actions">
                                            <Show when=move || !is_default>
                                                {
                                                    let p = for_default.clone();
                                                    view! { <button class="link-button" type="button" on:click=move |_| set_default(p.clone())>"Make default"</button> }
                                                }
                                            </Show>
                                            <Show when=move || deletable>
                                                {
                                                    let p = for_delete.clone();
                                                    view! { <button class="link-button danger" type="button" on:click=move |_| delete(p.clone())>"Delete"</button> }
                                                }
                                            </Show>
                                            </div>
                                        </td>
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
