//! Toasts: short confirmations and failures shown over the bottom right corner of the window,
//! announced to screen readers, dismissed after a while or by their close button.

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
#[derive(Clone, Debug, PartialEq, Eq)]
struct Toast {
    id: u64,
    kind: ToastKind,
    text: String,
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
        let id = self.next.get_value();
        self.next.set_value(id + 1);
        self.list.update(|list| {
            list.push(Toast {
                id,
                kind,
                text: text.into(),
            });
        });
        let stay = if kind == ToastKind::Error {
            LONG
        } else {
            SHORT
        };
        set_timeout(move || self.dismiss(id), stay);
    }

    /// Removes the toast `id`.
    fn dismiss(self, id: u64) {
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
                    view! {
                        <div class=class role=role>
                            <Icon kind=icon />
                            <span class="toast-text">{toast.text}</span>
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
