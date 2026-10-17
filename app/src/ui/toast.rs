//! Toasts: short confirmations and failures shown over the bottom right corner of the window,
//! announced to screen readers, dismissed after a while or by their close button. A toast with
//! an action stays until dismissed or acted on.

use std::time::Duration;

use leptos::prelude::*;

use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};

/// How long a success or information toast stays.
const SHORT: Duration = Duration::from_secs(5);
/// How long an error toast stays.
const LONG: Duration = Duration::from_secs(10);

/// A success toast: a box with a green left edge, green text.
const TOAST_SUCCESS: &str = "flex items-center gap-2 max-w-[min(26rem,calc(100vw-2rem))] py-2 pr-2 \
    pl-3 text-ok bg-panel-3 border border-l-3 border-edge-strong border-l-ok rounded-md shadow-lg \
    pointer-events-auto animate-toast-in";
/// An information toast: a box with a blue left edge.
const TOAST_INFO: &str = "flex items-center gap-2 max-w-[min(26rem,calc(100vw-2rem))] py-2 pr-2 \
    pl-3 bg-panel-3 border border-l-3 border-edge-strong border-l-info rounded-md shadow-lg \
    pointer-events-auto animate-toast-in";
/// An error toast: a box with a red left edge, red text.
const TOAST_ERROR: &str = "flex items-center gap-2 max-w-[min(26rem,calc(100vw-2rem))] py-2 pr-2 \
    pl-3 text-error bg-panel-3 border border-l-3 border-edge-strong border-l-error rounded-md \
    shadow-lg pointer-events-auto animate-toast-in";
/// A toast's action.
const TOAST_ACTION: &str = concatcp!(style::LINK_BUTTON, " whitespace-nowrap");
/// The icon of a success toast.
const ICON_SUCCESS: &str = concatcp!(style::ICON, " text-ok");
/// The icon of an information toast.
const ICON_INFO: &str = concatcp!(style::ICON, " text-info");
/// The icon of an error toast.
const ICON_ERROR: &str = concatcp!(style::ICON, " text-error");

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
        <div class="fixed right-[calc(1rem+var(--spacing-safe-r))] \
            bottom-[calc(var(--spacing-statusbar-safe)+1rem)] z-60 flex flex-col items-end gap-2 \
            pointer-events-none">
            <For each=move || toasts.list.get() key=|toast| toast.id let:toast>
                {
                    let (class, icon, icon_class) = match toast.kind {
                        ToastKind::Success => (TOAST_SUCCESS, IconKind::Check, ICON_SUCCESS),
                        ToastKind::Info => (TOAST_INFO, IconKind::Info, ICON_INFO),
                        ToastKind::Error => (TOAST_ERROR, IconKind::Alert, ICON_ERROR),
                    };
                    let role = if toast.kind == ToastKind::Error { "alert" } else { "status" };
                    let id = toast.id;
                    let action = toast.action.map(|(label, action)| {
                        view! {
                            <button
                                type="button"
                                class=TOAST_ACTION
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
                            <Icon kind=icon class=icon_class />
                            <span class="flex-1 min-w-0 wrap-break-word">{toast.text}</span>
                            {action}
                            <button
                                type="button"
                                class=style::ICON_BUTTON_GHOST_SM
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
