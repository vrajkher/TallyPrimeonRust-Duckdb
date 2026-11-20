//! Generic building blocks: a form, a list table and the app shell.

use std::sync::Arc;

use leptos::prelude::*;
use ot_api_types::{SOURCE_URL, User};

/// One input of a [`FormView`].
#[derive(Clone)]
pub struct FieldSpec {
    pub name: &'static str,
    pub label: &'static str,
    /// The input type, e.g. `text` or `password`.
    pub kind: &'static str,
    pub autocomplete: &'static str,
}

/// A titled form whose submit passes every field's value, by name.
#[component]
pub fn FormView(
    title: &'static str,
    intro: &'static str,
    fields: Vec<FieldSpec>,
    submit_label: &'static str,
    #[prop(into)] error: Signal<Option<String>>,
    #[prop(into)] busy: Signal<bool>,
    on_submit: Callback<Vec<(&'static str, String)>>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let values: Vec<(FieldSpec, RwSignal<String>)> = fields
        .into_iter()
        .map(|field| (field, RwSignal::new(String::new())))
        .collect();
    let submitted = values.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        on_submit.run(
            submitted
                .iter()
                .map(|(field, value)| (field.name, value.get_untracked()))
                .collect(),
        );
    };
    let inputs = values
        .into_iter()
        .enumerate()
        .map(|(index, (field, value))| {
            view! {
                <label class="field">
                    <span>{field.label}</span>
                    <input
                        type=field.kind
                        name=field.name
                        autocomplete=field.autocomplete
                        autofocus=index == 0
                        required
                        bind:value=value
                    />
                </label>
            }
        })
        .collect_view();
    view! {
        <form class="card form-view" on:submit=submit>
            <h1>{title}</h1>
            <p class="muted">{intro}</p>
            {inputs}
            <Show when=move || error.get().is_some()>
                <p class="status error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <button class="button primary" type="submit" disabled=move || { busy.get() }>
                {submit_label}
            </button>
            {children.map(|children| children())}
        </form>
    }
}

/// A column of a [`ListView`]: its heading and how to show a row's cell.
pub struct Column<T> {
    pub label: &'static str,
    pub numeric: bool,
    /// Hidden on narrow screens.
    pub secondary: bool,
    pub cell: Arc<dyn Fn(&T) -> String + Send + Sync>,
}

impl<T> Column<T> {
    pub fn text(label: &'static str, cell: impl Fn(&T) -> String + Send + Sync + 'static) -> Self {
        Self {
            label,
            numeric: false,
            secondary: false,
            cell: Arc::new(cell),
        }
    }

    pub fn number(
        label: &'static str,
        cell: impl Fn(&T) -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            label,
            numeric: true,
            secondary: false,
            cell: Arc::new(cell),
        }
    }

    /// Hides the column on narrow screens.
    pub fn secondary(mut self) -> Self {
        self.secondary = true;
        self
    }
}

/// A table of rows with fixed columns and an empty-state message.
#[component]
pub fn ListView<T>(
    #[prop(into)] rows: Signal<Vec<T>>,
    columns: Vec<Column<T>>,
    caption: &'static str,
    empty: &'static str,
) -> impl IntoView
where
    T: Clone + Send + Sync + 'static,
{
    let columns = Arc::new(columns);
    let head = columns
        .iter()
        .map(|column| view! { <th scope="col" class:num=column.numeric class:secondary=column.secondary>{column.label}</th> })
        .collect_view();
    let body_columns = Arc::clone(&columns);
    let body = move || {
        let rows = rows.get();
        if rows.is_empty() {
            return view! { <tr><td class="empty" colspan=body_columns.len()>{empty}</td></tr> }
                .into_any();
        }
        rows.into_iter()
            .map(|row| {
                let cells = body_columns
                    .iter()
                    .map(|column| view! { <td class:num=column.numeric class:secondary=column.secondary>{(column.cell)(&row)}</td> })
                    .collect_view();
                view! { <tr>{cells}</tr> }
            })
            .collect_view()
            .into_any()
    };
    view! {
        <div class="table-wrap">
            <table class="list-view">
                <caption class="visually-hidden">{caption}</caption>
                <thead><tr>{head}</tr></thead>
                <tbody>{body}</tbody>
            </table>
        </div>
    }
}

/// The screens of the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    DayBook,
    Chart,
}

/// The signed-in workspace: sidebar, top bar and content.
#[component]
pub fn Shell(
    user: Option<User>,
    screen: RwSignal<Screen>,
    #[prop(into)] notice: Signal<Option<String>>,
    on_sign_out: Callback<()>,
    children: Children,
) -> impl IntoView {
    let label = match &user {
        Some(user) => format!("Signed in as {}", user.full_name),
        None => "Local book, no users yet".to_owned(),
    };
    let signed_in = user.is_some();
    view! {
        <div class="layout">
            <aside class="sidebar">
                <a class="brand" href="/app/">
                    <span class="brand-mark" aria-hidden="true"><i></i><i></i><i></i></span>
                    <span>"OpenTally"</span>
                </a>
                <nav aria-label="Workspace">
                    <NavButton screen=screen target=Screen::DayBook label="Day Book" key="Alt+D"/>
                    <NavButton screen=screen target=Screen::Chart label="Chart of Accounts" key="Alt+A"/>
                    <a class="nav-link" href="/">"Classic screens"</a>
                </nav>
                <div class="sidebar-footer">
                    <span>{label}</span>
                    <Show when=move || signed_in>
                        <button class="link-button" type="button" on:click=move |_| on_sign_out.run(())>
                            "Sign out"
                        </button>
                    </Show>
                    <a href=SOURCE_URL rel="noopener">"Source code (AGPL-3.0)"</a>
                </div>
            </aside>
            <main class="workspace" id="main" tabindex="-1">
                <Show when=move || notice.get().is_some()>
                    <p class="status notice" role="status">{move || notice.get().unwrap_or_default()}</p>
                </Show>
                {children()}
            </main>
        </div>
    }
}

#[component]
fn NavButton(
    screen: RwSignal<Screen>,
    target: Screen,
    label: &'static str,
    key: &'static str,
) -> impl IntoView {
    let active = move || screen.get() == target;
    view! {
        <button
            type="button"
            class=move || if active() { "nav-link active" } else { "nav-link" }
            aria-current=move || active().then_some("page")
            on:click=move |_| screen.set(target)
        >
            {label} <kbd>{key}</kbd>
        </button>
    }
}
