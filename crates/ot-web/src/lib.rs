//! OpenTally's browser interface, built with Leptos and compiled to WebAssembly.
//!
//! It runs at `/app/` and talks only to the `/api/v1` API. Screens move here from the
//! classic interface one at a time; the Day Book is the first.

mod accounts;
mod api;
mod day_book;
mod items;
pub mod keys;
mod money_masters;
mod number_series;
mod parties;
mod setup;
mod taxes;
mod views;

use leptos::{prelude::*, task::spawn_local};
use ot_api_types::{Setup, SignIn, User};

use crate::{
    accounts::ChartView,
    day_book::DayBookView,
    items::ItemsView,
    keys::{Action, Modifiers},
    money_masters::MoneyView,
    number_series::SeriesView,
    parties::PartiesView,
    setup::SetupWizard,
    taxes::TaxesView,
    views::{FieldSpec, FormView, Screen, Shell},
};

/// The latest keyboard command, with a counter so the same key can fire twice.
#[derive(Clone, Copy)]
pub struct Command(pub RwSignal<Option<(u64, Action)>>);

#[derive(Clone, PartialEq)]
enum Phase {
    Loading,
    Setup,
    SignIn,
    /// Signed in (or a book without users); the flag says whether the company is set up.
    Ready(Option<User>, bool),
    Failed(String),
}

/// Mounts the app on the page.
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

fn check_session(phase: RwSignal<Phase>) {
    spawn_local(async move {
        let next = match api::session().await {
            Ok(state) if state.signed_in => Phase::Ready(state.user, state.company_setup_complete),
            Ok(state) if state.setup_required => Phase::Setup,
            Ok(_) => Phase::SignIn,
            Err(error) => Phase::Failed(error.message),
        };
        // Setting the same phase again would redraw the screen and clear a form the
        // user is typing in, e.g. when a request that was in flight at sign-out fails.
        if phase.get_untracked() != next {
            phase.set(next);
        }
    });
}

fn value(values: &[(&'static str, String)], name: &str) -> String {
    values
        .iter()
        .find(|(field, _)| *field == name)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

#[component]
fn App() -> impl IntoView {
    let phase = RwSignal::new(Phase::Loading);
    let command = Command(RwSignal::new(None));
    let notice = RwSignal::new(None::<String>);
    let screen = RwSignal::new(Screen::DayBook);
    provide_context(command);
    check_session(phase);

    let counter = StoredValue::new(0_u64);
    let handle = window_event_listener(leptos::ev::keydown, move |event| {
        if !matches!(phase.get_untracked(), Phase::Ready(..)) {
            return;
        }
        let mods = Modifiers {
            ctrl: event.ctrl_key(),
            alt: event.alt_key(),
            shift: event.shift_key(),
            meta: event.meta_key(),
        };
        let Some(action) = keys::action(&event.key(), mods, typing(&event)) else {
            return;
        };
        event.prevent_default();
        if let Some(kind) = keys::voucher_kind(action) {
            // Voucher entry is still on the classic screens.
            let _ = window()
                .location()
                .assign(&format!("/?voucher={kind}#entry"));
            return;
        }
        match action {
            Action::Sales | Action::Purchase => notice.set(Some(
                "Sales and purchase invoices arrive in a later release.".into(),
            )),
            Action::Save | Action::Create => notice.set(Some(
                "There is nothing to save or create on this screen.".into(),
            )),
            Action::Back => notice.set(None),
            Action::DayBook => screen.set(Screen::DayBook),
            Action::Accounts => screen.set(Screen::Chart),
            Action::Series => screen.set(Screen::Series),
            Action::Parties => screen.set(Screen::Parties),
            Action::Items => screen.set(Screen::Items),
            Action::Taxes => screen.set(Screen::Taxes),
            Action::Money => screen.set(Screen::Money),
            _ => {}
        }
        counter.update_value(|n| *n += 1);
        command.0.set(Some((counter.get_value(), action)));
    });
    on_cleanup(move || handle.remove());

    let on_signed_out = Callback::new(move |_| check_session(phase));
    let on_sign_out = Callback::new(move |_| {
        spawn_local(async move {
            api::sign_out().await;
            check_session(phase);
        });
    });

    move || {
        match phase.get() {
        Phase::Loading => view! { <p class="centered muted">"Opening OpenTally…"</p> }.into_any(),
        Phase::Failed(message) => view! {
            <div class="centered card">
                <p class="status error" role="alert">{message}</p>
                <button class="button" on:click=move |_| check_session(phase)>"Try again"</button>
            </div>
        }
        .into_any(),
        Phase::SignIn => view! { <SignInForm phase=phase/> }.into_any(),
        Phase::Setup => view! { <SetupForm phase=phase/> }.into_any(),
        Phase::Ready(user, false) if is_admin(user.as_ref()) => {
            let on_done = Callback::new(move |_| {
                screen.set(Screen::Chart);
                phase.set(Phase::Ready(user.clone(), true));
            });
            view! { <SetupWizard on_done=on_done/> }.into_any()
        }
        Phase::Ready(user, company_ready) => view! {
            <Shell user=user screen=screen notice=notice on_sign_out=on_sign_out>
                <Show when=move || !company_ready>
                    <p class="status notice" role="status">
                        "A System Manager still needs to finish setting up the company."
                    </p>
                </Show>
                {move || match screen.get() {
                    Screen::DayBook => view! { <DayBookView on_signed_out=on_signed_out/> }.into_any(),
                    Screen::Chart => view! { <ChartView on_signed_out=on_signed_out/> }.into_any(),
                    Screen::Series => view! { <SeriesView on_signed_out=on_signed_out/> }.into_any(),
                    Screen::Parties => view! { <PartiesView on_signed_out=on_signed_out/> }.into_any(),
                    Screen::Items => view! { <ItemsView on_signed_out=on_signed_out/> }.into_any(),
                    Screen::Taxes => view! { <TaxesView on_signed_out=on_signed_out/> }.into_any(),
                    Screen::Money => view! { <MoneyView on_signed_out=on_signed_out/> }.into_any(),
                }}
            </Shell>
        }
        .into_any(),
    }
    }
}

/// A System Manager, or anyone in a book without users, may run the setup wizard.
fn is_admin(user: Option<&User>) -> bool {
    user.is_none_or(|user| user.roles.iter().any(|role| role == "system_manager"))
}

/// Whether focus is in a field that takes text.
fn typing(event: &web_sys::KeyboardEvent) -> bool {
    use leptos::wasm_bindgen::JsCast;
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
        .is_some_and(|element| {
            matches!(element.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                || element.is_content_editable()
        })
}

#[component]
fn SignInForm(phase: RwSignal<Phase>) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = Callback::new(move |values: Vec<(&'static str, String)>| {
        let input = SignIn {
            username: value(&values, "username"),
            password: value(&values, "password"),
        };
        busy.set(true);
        spawn_local(async move {
            match api::sign_in(&input).await {
                Ok(state) => phase.set(Phase::Ready(state.user, state.company_setup_complete)),
                Err(failure) => error.set(Some(failure.message)),
            }
            busy.set(false);
        });
    });
    view! {
        <div class="centered">
            <FormView
                title="Sign in to OpenTally"
                intro="Use the user name and password a System Manager gave you."
                fields=vec![
                    FieldSpec { name: "username", label: "User name", kind: "text", autocomplete: "username" },
                    FieldSpec { name: "password", label: "Password", kind: "password", autocomplete: "current-password" },
                ]
                submit_label="Sign in"
                error=error
                busy=busy
                on_submit=submit
            />
        </div>
    }
}

#[component]
fn SetupForm(phase: RwSignal<Phase>) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = Callback::new(move |values: Vec<(&'static str, String)>| {
        let input = Setup {
            username: value(&values, "username"),
            full_name: value(&values, "full_name"),
            password: value(&values, "password"),
        };
        busy.set(true);
        spawn_local(async move {
            match api::setup(&input).await {
                Ok(state) => phase.set(Phase::Ready(state.user, state.company_setup_complete)),
                Err(failure) => error.set(Some(failure.message)),
            }
            busy.set(false);
        });
    });
    view! {
        <div class="centered">
            <FormView
                title="Create the first user"
                intro="This book has no users yet, so only this computer can open it. The first user becomes a System Manager. Use a password of at least 10 characters."
                fields=vec![
                    FieldSpec { name: "username", label: "User name", kind: "text", autocomplete: "username" },
                    FieldSpec { name: "full_name", label: "Full name", kind: "text", autocomplete: "name" },
                    FieldSpec { name: "password", label: "Password", kind: "password", autocomplete: "new-password" },
                ]
                submit_label="Create user and sign in"
                error=error
                busy=busy
                on_submit=submit
            >
                <button class="link-button" type="button" on:click=move |_| {
                    // The session reply says whether the company is set up yet.
                    spawn_local(async move {
                        let ready = api::session().await.is_ok_and(|s| s.company_setup_complete);
                        phase.set(Phase::Ready(None, ready));
                    });
                }>
                    "Continue without users on this computer"
                </button>
            </FormView>
        </div>
    }
}
