//! Toasts: short confirmations and failures shown over the bottom right corner of the window,
//! announced to screen readers, dismissed after a while or by their close button. A toast with
//! an action stays until dismissed or acted on.

use std::time::Duration;

use leptos::prelude::*;

use crate::ui::icon::{Icon, IconKind};

/// How long a success or information toast stays.
const SHORT: Duration = Duration::from_secs(5);
/// How long an error toast stays.
const LONG: Duration = Duration::from_secs(10);

/// What a toast reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// Something worked.
    Success,
    /// Something to know.
    Info,
    /// Something failed.
    Error,
}

/// One toast.
#[derive(Clone)]
struct Toast {
    id: u64,
    kind: ToastKind,
    text: String,
    /// A button's label and what it does; the toast closes with it.
    action: Option<(&'static str, Callback<()>)>,
}

/// The toasts of the window; `Copy`, provided as context by [`crate::ui::App`].
#[derive(Clone, Copy)]
pub struct Toasts {
    list: RwSignal<Vec<Toast>>,
    next: StoredValue<u64>,
}

impl Toasts {
    /// No toasts.
    pub fn new() -> Self {
        Self {
            list: RwSignal::new(Vec::new()),
            next: StoredValue::new(0),
        }
    }

    /// The toasts provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// Shows `text` for a while.
    pub fn show(self, kind: ToastKind, text: impl Into<String>) {
        let id = self.push(kind, text.into(), None);
        let stay = if kind == ToastKind::Error {
            LONG
        } else {
            SHORT
        };
        set_timeout(move || self.dismiss(id), stay);
    }

    /// Shows `text` with a button `label` running `action` until either is used or
    /// [`Self::dismiss`] removes it; returns its id.
    pub fn show_action(
        self,
        kind: ToastKind,
        text: impl Into<String>,
        label: &'static str,
        action: Callback<()>,
    ) -> u64 {
        self.push(kind, text.into(), Some((label, action)))
    }

    /// Adds a toast; returns its id.
    fn push(
        self,
        kind: ToastKind,
        text: String,
        action: Option<(&'static str, Callback<()>)>,
    ) -> u64 {
        let id = self.next.get_value();
        self.next.set_value(id + 1);
        self.list.update(|list| {
            list.push(Toast {
                id,
                kind,
                text,
                action,
            });
        });
        id
    }

    /// Removes the toast `id`, if still shown.
    pub fn dismiss(self, id: u64) {
        self.list
            .try_update(|list| list.retain(|toast| toast.id != id));
    }
}

/// Where the toasts show.
#[component]
pub fn ToastRegion() -> impl IntoView {
    let toasts = Toasts::expect();
    view! {
        <div class="toasts">
            <For each=move || toasts.list.get() key=|toast| toast.id let:toast>
                {
                    let (class, icon) = match toast.kind {
                        ToastKind::Success => ("toast success", IconKind::Check),
                        ToastKind::Info => ("toast info", IconKind::Info),
                        ToastKind::Error => ("toast error", IconKind::Alert),
                    };
                    let role = if toast.kind == ToastKind::Error { "alert" } else { "status" };
                    let id = toast.id;
                    let action = toast.action.map(|(label, action)| {
                        view! {
                            <button
                                type="button"
                                class="link toast-action"
                                on:click=move |_| {
                                    toasts.dismiss(id);
                                    action.run(());
                                }
                            >
                                {label}
                            </button>
                        }
                    });
                    view! {
                        <div class=class role=role>
                            <Icon kind=icon />
                            <span class="toast-text">{toast.text}</span>
                            {action}
                            <button
                                type="button"
                                class="icon-button sm ghost"
                                aria-label="Dismiss"
                                on:click=move |_| toasts.dismiss(id)
                            >
                                <Icon kind=IconKind::Close />
                            </button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}
