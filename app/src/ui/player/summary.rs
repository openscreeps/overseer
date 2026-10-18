//! The summary of a player, the same on the own profile and on the page of another player:
//! the head ([`SummaryHead`]: badge, name and server), the level rows ([`LevelRows`]), and
//! the way back to the map ([`open_map`]).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use leptos::prelude::*;
use overseer::model::users::LevelProgress;

use crate::state::AppState;
use crate::ui::spawn_scoped;
use crate::ui::style;

/// The head: badge, name and server.
const HEAD: &str = "flex items-center gap-4 mb-5";
/// The badge.
const BADGE: &str = "flex-none size-[64px] rounded-full bg-panel-2";
/// The badge's placeholder while it loads.
const BADGE_SKELETON: &str = "flex-none size-[64px] rounded-full bg-linear-90 from-panel-2 \
    from-25% via-panel-3 via-50% to-panel-2 to-75% bg-size-[200%_100%] animate-shimmer \
    motion-reduce:animate-none";
/// The figures: names beside their values, stacked on phones.
pub const STATS: &str = "grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 max-xs:grid-cols-1 \
    max-xs:gap-x-4 max-xs:gap-y-0";
/// The name of a figure of [`STATS`].
pub const STAT_NAME: &str = "text-muted";
/// The value of a figure of [`STATS`].
pub const STAT_VALUE: &str = "flex flex-wrap items-center gap-x-3 gap-y-1 tabular-nums max-xs:mb-2";

/// The image source of a badge ([`load_badge`]): `None` while it loads, `Some(None)` when the
/// server sends no badge.
pub type BadgeSource = ReadSignal<Option<Option<String>>>;

/// Loads the badge SVG document that `svg` fetches, as the source of an image (a `data:` URL).
/// The load stops when the owner of the calling scope goes away.
pub fn load_badge(
    svg: impl Future<Output = Result<String, overseer::Error>> + 'static,
) -> BadgeSource {
    let source = RwSignal::new(None);
    spawn_scoped(async move {
        let url = svg
            .await
            .ok()
            .map(|svg| format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg)));
        source.set(Some(url));
    });
    source.read_only()
}

/// The head of a summary: the badge, the player's name as the page's heading (element id
/// `title_id`, which names the summary's region), and the server's name.
#[component]
pub fn SummaryHead(
    /// The badge.
    badge: BadgeSource,
    /// The player's name.
    name: String,
    /// The element id of the heading.
    title_id: &'static str,
    /// The server's name.
    server: Option<String>,
) -> impl IntoView {
    let alt = format!("Badge of {name}");
    let badge = move || match badge.get() {
        None => view! { <span class=BADGE_SKELETON aria-hidden="true"></span> }.into_any(),
        Some(None) => view! { <span class=BADGE aria-hidden="true"></span> }.into_any(),
        Some(Some(src)) => {
            view! { <img class=BADGE src=src alt=alt.clone() width="64" height="64" /> }.into_any()
        }
    };
    view! {
        <div class=HEAD>
            {badge}
            <div class="min-w-0">
                <h1 id=title_id class="text-xl font-semibold leading-tight wrap-anywhere">
                    {name}
                </h1>
                {server.map(|server| view! { <p class="mt-1 text-muted">{server}</p> })}
            </div>
        </div>
    }
}

/// The placeholder of a [`SummaryHead`] while the player loads; `label` tells screen readers
/// what loads.
#[component]
pub fn HeadSkeleton(label: &'static str) -> impl IntoView {
    view! {
        <div class=HEAD role="status">
            <span class=BADGE_SKELETON aria-hidden="true"></span>
            <span class=style::SKELETON_LINE_WIDE aria-hidden="true"></span>
            <span class="sr-only">{label}</span>
        </div>
    }
}

/// The rows of the Global Control Level and the power level of a [`STATS`] list, from the
/// lifetime points `gcl` and `power` (0 when not reported).
#[component]
pub fn LevelRows(gcl: Option<f64>, power: Option<f64>) -> impl IntoView {
    let gcl = LevelProgress::gcl(gcl.unwrap_or(0.0));
    let power = LevelProgress::power(power.unwrap_or(0.0));
    view! {
        <dt class=STAT_NAME>"Global Control Level"</dt>
        <dd class=STAT_VALUE>
            <LevelBar label="GCL" level=gcl />
        </dd>
        <dt class=STAT_NAME>"Power level"</dt>
        <dd class=STAT_VALUE>
            <LevelBar label="Power level" level=power />
        </dd>
    }
}

/// Opens the world of the signed-in server.
pub fn open_map(state: AppState) {
    if let Some(server) = state.session_server() {
        state.open_server(&server.url);
    }
}

/// A level with a progress bar towards the next one.
#[component]
fn LevelBar(label: &'static str, level: LevelProgress) -> impl IntoView {
    let percent = level.fraction() * 100.0;
    let text = format!(
        "{} / {}",
        format_points(level.progress),
        format_points(level.total)
    );
    view! {
        <span class="min-w-[4.5em]">{format!("Level {}", level.level)}</span>
        <progress
            class=style::PROGRESS
            max="100"
            value=format!("{percent:.1}")
            aria-label=format!("{label} progress to level {}", level.level + 1)
        ></progress>
        <span class="text-muted">{text}</span>
    }
}

/// Whole points with thousands separators.
fn format_points(points: f64) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "display of non-negative point counts far below u64::MAX"
    )]
    let whole = points.max(0.0).round() as u64;
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}
