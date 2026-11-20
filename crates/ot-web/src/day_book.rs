//! The Day Book: every voucher, filtered by date, type and text, a page at a time.

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{DayBook, DayBookFilter, VoucherRow, format_paise};

use crate::{
    Command,
    api::{self, ApiError},
    keys::Action,
    views::{Column, ListView},
};

pub const SEARCH_ID: &str = "day-book-search";

fn optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn columns() -> Vec<Column<VoucherRow>> {
    vec![
        Column::text("Date", |row: &VoucherRow| row.date.clone()),
        Column::text("Voucher no.", |row: &VoucherRow| row.code.clone()),
        Column::text("Type", |row: &VoucherRow| row.type_name.clone()).secondary(),
        Column::text("Particulars", |row: &VoucherRow| {
            let mut text = if row.other_account.is_empty() {
                "Journal".to_owned()
            } else {
                row.other_account.clone()
            };
            if !row.narration.is_empty() {
                text = format!("{text} — {}", row.narration);
            }
            text
        }),
        Column::number("Amount (₹)", |row: &VoucherRow| {
            format_paise(&row.amount_paise)
        }),
        Column::text("Status", |row: &VoucherRow| row.status.clone()).secondary(),
    ]
}

#[component]
pub fn DayBookView(on_signed_out: Callback<()>) -> impl IntoView {
    let from = RwSignal::new(String::new());
    let to = RwSignal::new(String::new());
    let kind = RwSignal::new(String::new());
    let q = RwSignal::new(String::new());
    let page = RwSignal::new(1_u32);
    let book = RwSignal::new(None::<DayBook>);
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(false);

    let load = move || {
        let filter = DayBookFilter {
            kind: optional(kind.get_untracked()),
            from: optional(from.get_untracked()),
            to: optional(to.get_untracked()),
            q: optional(q.get_untracked()),
            status: None,
            page: Some(page.get_untracked()),
        };
        loading.set(true);
        spawn_local(async move {
            match api::day_book(&filter).await {
                Ok(result) => {
                    book.set(Some(result));
                    error.set(None);
                }
                Err(ApiError {
                    signed_out: true, ..
                }) => on_signed_out.run(()),
                Err(failure) => error.set(Some(failure.message)),
            }
            loading.set(false);
        });
    };
    load();

    let pages = move || book.with(|b| b.as_ref().map_or(1, DayBook::pages));
    let go = move |next: u32| {
        if next >= 1 && next <= pages() && next != page.get_untracked() {
            page.set(next);
            load();
        }
    };

    // Shortcuts from the app's keyboard layer.
    let command = use_context::<Command>().expect("the app provides commands");
    Effect::new(move |_| {
        if let Some((_, action)) = command.0.get() {
            match action {
                Action::NextPage => go(page.get_untracked() + 1),
                Action::PrevPage => go(page.get_untracked().saturating_sub(1)),
                Action::Search => {
                    if let Some(field) = document().get_element_by_id(SEARCH_ID) {
                        use leptos::wasm_bindgen::JsCast;
                        if let Ok(field) = field.dyn_into::<web_sys::HtmlElement>() {
                            let _ = field.focus();
                        }
                    }
                }
                _ => {}
            }
        }
    });

    let filter = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        page.set(1);
        load();
    };
    let rows = Signal::derive(move || {
        book.with(|b| b.as_ref().map(|b| b.rows.clone()).unwrap_or_default())
    });
    let totals = move || {
        book.with(|b| {
            b.as_ref().map(|b| {
                format!(
                    "{} voucher{} · Receipts ₹{} · Payments ₹{}",
                    b.count,
                    if b.count == 1 { "" } else { "s" },
                    format_paise(&b.receipt_paise),
                    format_paise(&b.payment_paise)
                )
            })
        })
    };

    view! {
        <header class="page-heading">
            <h1>"Day Book"</h1>
            <p class="muted">"Every voucher in the book. Press / to search, Page Up and Page Down to move between pages."</p>
        </header>
        <form class="card filters" on:submit=filter aria-label="Filter the Day Book">
            <label class="field"><span>"From"</span><input type="date" bind:value=from/></label>
            <label class="field"><span>"To"</span><input type="date" bind:value=to/></label>
            <label class="field">
                <span>"Type"</span>
                <select aria-label="Type" bind:value=kind>
                    <option value="">"All types"</option>
                    <option value="payment">"Payment"</option>
                    <option value="receipt">"Receipt"</option>
                    <option value="journal">"Journal"</option>
                    <option value="contra">"Contra"</option>
                </select>
            </label>
            <label class="field grow">
                <span>"Search"</span>
                <input id=SEARCH_ID type="search" placeholder="Voucher no., ledger, reference or narration" bind:value=q/>
            </label>
            <button class="button primary" type="submit">"Show"</button>
        </form>
        <Show when=move || error.get().is_some()>
            <p class="status error" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>
        <p class="totals" aria-live="polite">
            {move || totals().or_else(|| loading.get().then(|| "Loading…".to_owned()))}
        </p>
        <ListView
            rows=rows
            columns=columns()
            caption="Day Book vouchers"
            empty="No vouchers match these filters."
        />
        <nav class="pager" aria-label="Day Book pages">
            <button class="button" type="button" disabled=move || { page.get() <= 1 }
                on:click=move |_| go(page.get_untracked() - 1)>"Previous"</button>
            <span>{move || format!("Page {} of {}", page.get(), pages())}</span>
            <button class="button" type="button" disabled=move || { page.get() >= pages() }
                on:click=move |_| go(page.get_untracked() + 1)>"Next"</button>
        </nav>
    }
}
