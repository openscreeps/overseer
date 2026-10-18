//! Line icons drawn inline (24 × 24 view box, stroked with the current text colour), for icon
//! buttons and menu items. They are decorative: the button carries the accessible name
//! (`aria-label`) and the tooltip (`data-tip`, `style::TIP`).

use leptos::prelude::*;

use crate::ui::style;

/// The icons of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    /// Zoom in, add.
    Plus,
    /// Zoom out.
    Minus,
    /// Frame everything (the whole world).
    Fit,
    /// The right drawer.
    Drawer,
    /// Pop out into a window.
    PopOut,
    /// Close, dismiss.
    Close,
    /// Search.
    Search,
    /// Top-down (2D) view.
    TopDown,
    /// Perspective (3D) view.
    Perspective,
    /// A menu's caret; the next command of the console's history.
    ChevronDown,
    /// The previous command of the console's history.
    ChevronUp,
    /// Back.
    ArrowLeft,
    /// A server.
    Server,
    /// The account.
    User,
    /// Sign out.
    SignOut,
    /// Rate limits.
    Gauge,
    /// Remove.
    Trash,
    /// Done, success.
    Check,
    /// Warning, failure.
    Alert,
    /// Information.
    Info,
    /// Try again, reload.
    Refresh,
    /// The console.
    Terminal,
    /// A list of objects.
    List,
    /// The world.
    Globe,
    /// A room.
    Room,
    /// Pause.
    Pause,
    /// Play, resume.
    Play,
    /// Filter.
    Filter,
    /// Send, run.
    Send,
    /// A drag handle.
    Grip,
    /// Memory (braces).
    Memory,
    /// CPU (a chip).
    Cpu,
}

impl IconKind {
    /// The SVG path data (24 × 24, stroked).
    pub const fn path(self) -> &'static str {
        match self {
            Self::Plus => "M12 5v14M5 12h14",
            Self::Minus => "M5 12h14",
            Self::Fit => {
                "M8 3H5a2 2 0 0 0-2 2v3M21 8V5a2 2 0 0 0-2-2h-3M3 16v3a2 2 0 0 0 2 2h3M16 21h3a2 2 0 0 0 2-2v-3"
            }
            Self::Drawer => {
                "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM15 3v18"
            }
            Self::PopOut => {
                "M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"
            }
            Self::Close => "M18 6 6 18M6 6l12 12",
            Self::Search => "M11 4a7 7 0 1 0 0 14a7 7 0 1 0 0-14zM21 21l-4.35-4.35",
            Self::TopDown => "M4 4h16v16H4zM4 10h16M4 15h16M10 4v16M15 4v16",
            Self::Perspective => {
                "M12 2.5 3.5 7v10l8.5 4.5 8.5-4.5V7zM3.5 7l8.5 4.5L20.5 7M12 11.5v10"
            }
            Self::ChevronDown => "m6 9 6 6 6-6",
            Self::ChevronUp => "m18 15-6-6-6 6",
            Self::ArrowLeft => "M19 12H5M12 19l-7-7 7-7",
            Self::Server => "M4 4h16v6H4zM4 14h16v6H4zM8 7h.01M8 17h.01",
            Self::User => "M12 12a4 4 0 1 0 0-8a4 4 0 1 0 0 8zM4 21a8 8 0 0 1 16 0",
            Self::SignOut => "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9",
            Self::Gauge => "M12 14l4-4M3.34 19a10 10 0 1 1 17.32 0",
            Self::Trash => "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6",
            Self::Check => "M20 6 9 17l-5-5",
            Self::Alert => {
                "M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0zM12 9v4M12 17h.01"
            }
            Self::Info => "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20zM12 16v-4M12 8h.01",
            Self::Refresh => "M21 12a9 9 0 1 1-2.64-6.36L21 8M21 3v5h-5",
            Self::Terminal => "M4 17l6-6-6-6M12 19h8",
            Self::List => "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
            Self::Globe => {
                "M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20zM2 12h20M12 2a15 15 0 0 1 0 20M12 2a15 15 0 0 0 0 20"
            }
            Self::Room => "M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z",
            Self::Pause => "M7 4h3v16H7zM14 4h3v16h-3z",
            Self::Play => "M6 3.5 19 12 6 20.5z",
            Self::Filter => "M22 3H2l8 9.46V19l4 2v-8.54z",
            Self::Send => "M22 2 11 13M22 2l-7 20-4-9-9-4z",
            Self::Grip => "M9 5h.01M9 12h.01M9 19h.01M15 5h.01M15 12h.01M15 19h.01",
            Self::Memory => {
                "M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1"
            }
            Self::Cpu => {
                "M7 5h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2zM10 10h4v4h-4zM9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3"
            }
        }
    }
}

/// An icon, sized by the font (`style::ICON`, 1.15em) unless `class` says otherwise, hidden
/// from assistive technology.
#[component]
pub fn Icon(
    /// Which icon.
    kind: IconKind,
    /// Its classes: size, colour, motion (`style::ICON` by default).
    #[prop(default = style::ICON)]
    class: &'static str,
) -> impl IntoView {
    view! {
        <svg
            class=class
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            focusable="false"
        >
            <path d=kind.path() />
        </svg>
    }
}
