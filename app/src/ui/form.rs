//! Form fields with labels, help, errors and live status, wired for screen readers.

use leptos::prelude::*;

use crate::ui::style;

/// A labelled text input bound to `value`. `error` is announced and marks the input invalid;
/// `status` (live checks) is announced politely.
#[component]
pub fn Field(
    /// Element id of the input.
    id: &'static str,
    /// Visible label.
    label: &'static str,
    /// `type` of the input.
    #[prop(default = "text")]
    kind: &'static str,
    /// `autocomplete` of the input.
    #[prop(default = "off")]
    autocomplete: &'static str,
    /// The bound value.
    value: RwSignal<String>,
    /// The field's error, if shown.
    #[prop(into, optional)]
    error: Signal<Option<String>>,
    /// A live status line (availability checks).
    #[prop(into, optional)]
    status: Signal<Option<String>>,
    /// Help text under the input.
    #[prop(optional)]
    help: Option<&'static str>,
    /// Called when the input loses focus.
    #[prop(optional)]
    on_blur: Option<Callback<()>>,
) -> impl IntoView {
    let error_id = format!("{id}-error");
    let status_id = format!("{id}-status");
    let help_id = format!("{id}-help");
    let described_by = {
        let mut ids = vec![error_id.clone(), status_id.clone()];
        if help.is_some() {
            ids.push(help_id.clone());
        }
        ids.join(" ")
    };
    view! {
        <div class=style::FIELD>
            <label for=id class=style::LABEL>{label}</label>
            <input
                class=style::INPUT
                id=id
                type=kind
                autocomplete=autocomplete
                spellcheck="false"
                aria-invalid=move || error.with(Option::is_some).to_string()
                aria-describedby=described_by
                bind:value=value
                on:blur=move |_| {
                    if let Some(on_blur) = on_blur {
                        on_blur.run(());
                    }
                }
            />
            {help.map(|help| view! { <p id=help_id class=style::HELP>{help}</p> })}
            <p id=status_id class=style::CHECK aria-live="polite">
                {move || status.get()}
            </p>
            <p id=error_id class=style::FIELD_ERROR role="alert">
                {move || error.get()}
            </p>
        </div>
    }
}

/// A form's error, announced when it appears.
#[component]
pub fn FormError(
    /// The error.
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <p class=style::ERROR role="alert">
            {move || error.get()}
        </p>
    }
}

/// A form's success message, announced when it appears.
#[component]
pub fn FormStatus(
    /// The message.
    message: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <p class=style::SUCCESS role="status">
            {move || message.get()}
        </p>
    }
}
