//! Class sets shared by the components: the app's controls (buttons, inputs, tabs), text
//! styles, tooltips and small pieces, as Tailwind utility classes (`app/tailwind.css` holds the
//! design tokens they use).
//!
//! Every set is complete for what it styles: Tailwind orders utilities by its own rules, not by
//! their order in `class`, so a component never adds a utility that one of these already sets
//! (a second background, padding or size); it picks the set that fits, or composes one from
//! the parts ([`BTN`] with a `BTN_*` colour and a size) with [`concatcp!`]. Classes stay literal
//! strings: Tailwind finds them by scanning the sources.

pub use const_format::concatcp;

/// An icon (`ui::icon::Icon`), sized by the font.
pub const ICON: &str = "inline flex-none size-[1.15em]";
/// An [`ICON`] in the muted colour (beside a menu item's text, a panel's title).
pub const ICON_MUTED: &str = concatcp!(ICON, " text-muted");

/// What every button shares: layout, line height, shape and motion, without weight, colours,
/// padding or size (the `BTN_*` colour sets bring the weight).
pub const BTN: &str = "inline-flex items-center justify-center gap-2 leading-tight \
    whitespace-nowrap border rounded-md cursor-pointer transition-colors \
    duration-(--duration-fast) ease-standard disabled:opacity-55 disabled:cursor-default";
/// A button's default colours.
pub const BTN_DEFAULT: &str = "font-medium text-fg bg-panel-2 border-edge \
    enabled:hover:bg-panel-3 enabled:hover:border-edge-strong";
/// The colours of the main action of a form or page.
pub const BTN_PRIMARY: &str = "font-semibold text-accent-fg bg-accent border-accent \
    enabled:hover:bg-accent-hover enabled:hover:border-accent-hover";
/// The colours of a destructive action.
pub const BTN_DANGER: &str = "font-medium text-error bg-transparent border-error/45 \
    enabled:hover:bg-error-soft enabled:hover:border-error";
/// The colours of a quiet button: no fill or border until hovered.
pub const BTN_GHOST: &str =
    "font-medium text-fg bg-transparent border-transparent enabled:hover:bg-hover";
/// A text button's size: control height, horizontal padding.
pub const BTN_MD: &str = "min-h-control px-3";
/// A small text button's size: small control height, horizontal padding, small text.
pub const BTN_SM: &str = "min-h-control-sm px-3 text-sm";
/// A square icon button's size.
pub const BTN_ICON: &str = "flex-none size-control min-h-control p-0";
/// A small square icon button's size.
pub const BTN_ICON_SM: &str = "flex-none size-control-sm min-h-control-sm p-0";
/// An icon button that opens a menu or panel: highlighted while it is open.
pub const BTN_EXPANDED: &str =
    "aria-expanded:text-accent aria-expanded:border-accent/45 aria-expanded:bg-accent-soft";

/// A button.
pub const BUTTON: &str = concatcp!(BTN, " ", BTN_DEFAULT, " ", BTN_MD);
/// The main action of a form or page.
pub const BUTTON_PRIMARY: &str = concatcp!(BTN, " ", BTN_PRIMARY, " ", BTN_MD);
/// A destructive action.
pub const BUTTON_DANGER: &str = concatcp!(BTN, " ", BTN_DANGER, " ", BTN_MD);
/// A small button (in a bar or a panel of small controls).
pub const BUTTON_SM: &str = concatcp!(BTN, " ", BTN_DEFAULT, " ", BTN_SM);
/// A small main action.
pub const BUTTON_PRIMARY_SM: &str = concatcp!(BTN, " ", BTN_PRIMARY, " ", BTN_SM);
/// A small destructive action.
pub const BUTTON_DANGER_SM: &str = concatcp!(BTN, " ", BTN_DANGER, " ", BTN_SM);
/// A square icon button (the accessible name in `aria-label`, a tooltip in `data-tip`).
pub const ICON_BUTTON: &str = concatcp!(BTN, " ", BTN_DEFAULT, " ", BTN_ICON, " ", BTN_EXPANDED);
/// A small square icon button.
pub const ICON_BUTTON_SM: &str =
    concatcp!(BTN, " ", BTN_DEFAULT, " ", BTN_ICON_SM, " ", BTN_EXPANDED);
/// A quiet square icon button (in bars and heads).
pub const ICON_BUTTON_GHOST: &str =
    concatcp!(BTN, " ", BTN_GHOST, " ", BTN_ICON, " ", BTN_EXPANDED);
/// A quiet small square icon button.
pub const ICON_BUTTON_GHOST_SM: &str =
    concatcp!(BTN, " ", BTN_GHOST, " ", BTN_ICON_SM, " ", BTN_EXPANDED);
/// A button that reads as a link.
pub const LINK_BUTTON: &str = "inline-flex items-center gap-2 p-0 text-focus underline \
    underline-offset-2 cursor-pointer enabled:hover:text-link-hover disabled:opacity-55 \
    disabled:cursor-default";
/// A "back" link button under a card.
pub const BACK_BUTTON: &str = "inline-flex items-center gap-1 mt-5 p-0 text-muted cursor-pointer \
    enabled:hover:text-fg";

/// A segmented control: buttons side by side (each [`SEGMENT`]), one of them pressed.
pub const SEGMENTED: &str = "inline-flex";
/// Tabs made of a segmented control, as wide as their form (each [`TAB_SEGMENT`]).
pub const SEGMENTED_TABS: &str = "flex mb-4";
/// A button of a [`SEGMENTED`] control (`aria-pressed` marks the chosen one).
pub const SEGMENT: &str = concatcp!(BTN, " ", BTN_DEFAULT, " ", BTN_MD, " ", SEGMENT_SHAPE);
/// A button of [`SEGMENTED_TABS`].
pub const TAB_SEGMENT: &str = concatcp!(SEGMENT, " flex-1");
/// The shape and pressed colours of a segment, for segments of another size (with [`BTN`]: its
/// `rounded-md` stays on the outer corners only). The pressed one keeps its colours on hover.
pub const SEGMENT_SHAPE: &str = "not-first:rounded-l-none not-last:rounded-r-none \
    not-first:-ml-px aria-pressed:relative aria-pressed:text-accent-fg aria-pressed:bg-accent \
    aria-pressed:border-accent aria-pressed:enabled:hover:bg-accent \
    aria-pressed:enabled:hover:border-accent";

/// What every text input and `<select>` shares, without its size.
const INPUT_FIELD: &str = "px-3 text-fg bg-sunken border border-edge rounded-md \
    transition-[border-color] duration-(--duration-fast) ease-standard hover:border-edge-strong \
    focus-visible:outline-offset-0 focus-visible:border-focus placeholder:text-faint \
    aria-invalid:border-error";
/// A text input or `<select>`.
pub const INPUT: &str = concatcp!("min-h-control ", INPUT_FIELD);
/// A small text input or `<select>` (in a bar of small controls).
pub const INPUT_SM: &str = concatcp!("min-h-control-sm text-sm ", INPUT_FIELD);
/// A checkbox.
pub const CHECKBOX: &str = "size-[1.1rem] m-0 accent-accent";

/// A form: fields stacked; its buttons keep their width.
pub const FORM: &str = "flex flex-col gap-3 [&>button]:self-start";
/// A labelled field: label, input and the lines under it.
pub const FIELD: &str = "flex flex-col gap-1";
/// A field's label.
pub const LABEL: &str = "font-medium";
/// A checkbox and its label on one line.
pub const CHECK_FIELD: &str = "flex items-center gap-2";
/// Help under a field.
pub const HELP: &str = "text-sm text-muted";
/// A live check's result under a field (hidden while empty).
pub const CHECK: &str = "text-sm text-ok empty:hidden";
/// An error (hidden while empty).
pub const ERROR: &str = "text-error empty:hidden";
/// A field's error (hidden while empty).
pub const FIELD_ERROR: &str = "text-sm text-error empty:hidden";
/// A success message (hidden while empty).
pub const SUCCESS: &str = "text-ok empty:hidden";
/// A warning note over a form.
pub const NOTICE: &str = "mb-4 px-3 py-2 bg-warn-soft border-l-3 border-warn rounded-r-sm";
/// A row of buttons under a form or panel.
pub const ACTIONS: &str = "flex flex-wrap gap-2 mt-4";

/// A section's title.
pub const H2: &str = "mb-3 text-lg font-semibold leading-tight";
/// A keyboard key.
pub const KBD: &str = "inline-flex items-center justify-center min-w-[1.4em] px-[0.3em] \
    font-mono text-xs text-muted border border-b-2 border-edge rounded-sm";

/// A page of the main area (servers, profile): centred, at most 60rem wide.
pub const PAGE: &str = "flex flex-col gap-4 w-full max-w-240 mx-auto";
/// A panel.
pub const PANEL: &str = "p-5 bg-panel border border-edge rounded-lg";
/// A narrow panel, centred (sign-in, register, set username).
pub const PANEL_NARROW: &str =
    "p-5 bg-panel border border-edge rounded-lg w-full max-w-md self-center";
/// A [`PANEL_NARROW`] raised as a card.
pub const CARD: &str = "mt-6 p-6 bg-panel border border-edge rounded-lg shadow-card w-full max-w-md \
    self-center";
/// A card's head: icon, title and lead (the text shrinks; a long one wraps).
pub const CARD_HEAD: &str = "flex items-center gap-3 mb-5 [&>div]:min-w-0";
/// A card's title.
pub const CARD_TITLE: &str = "text-lg font-semibold leading-tight";

/// A whole-page state (loading, failure, missing): an icon over a title and its message.
pub const STATE_CARD: &str = "flex flex-col items-center gap-2 text-center";
/// The title of a [`STATE_CARD`].
pub const STATE_TITLE: &str = "text-lg font-semibold leading-tight";
/// The message of a [`STATE_CARD`].
pub const STATE_TEXT: &str = "max-w-md text-muted";
/// The round icon of a [`STATE_CARD`].
pub const STATE_ICON: &str = "inline-flex items-center justify-center size-11 mb-1 text-muted \
    bg-panel-2 rounded-full";
/// The round icon of a [`STATE_CARD`] that warns.
pub const STATE_ICON_WARN: &str = "inline-flex items-center justify-center size-11 mb-1 text-warn \
    bg-warn-soft rounded-full";
/// The round icon of a [`STATE_CARD`] that reports a failure.
pub const STATE_ICON_ERROR: &str = "inline-flex items-center justify-center size-11 mb-1 \
    text-error bg-error-soft rounded-full";
/// The icon inside a `STATE_ICON*`.
pub const STATE_ICON_SVG: &str = "inline flex-none size-[1.4rem]";

/// An empty list or panel: a short, centred, muted message.
pub const EMPTY_STATE: &str =
    "flex flex-col items-center gap-2 px-4 py-6 text-sm text-muted text-center";
/// The icon of an [`EMPTY_STATE`].
pub const EMPTY_ICON: &str = "inline flex-none size-[1.6rem] text-faint";

/// A spinner.
pub const SPINNER: &str = "inline-block flex-none size-[1.4rem] border-3 border-edge \
    border-t-accent rounded-full animate-spin motion-reduce:animate-spin-slow";
/// A small spinner (in text).
pub const SPINNER_SMALL: &str = "inline-block flex-none size-[0.8rem] border-2 border-edge \
    border-t-accent rounded-full animate-spin motion-reduce:animate-spin-slow";
/// A placeholder line while text loads.
pub const SKELETON_LINE: &str = "inline-block w-24 h-[0.9em] align-middle rounded-sm \
    bg-linear-90 from-panel-2 from-25% via-panel-3 via-50% to-panel-2 to-75% bg-size-[200%_100%] \
    animate-shimmer motion-reduce:animate-none";
/// A wide placeholder line.
pub const SKELETON_LINE_WIDE: &str = "inline-block w-48 h-[0.9em] align-middle rounded-sm \
    bg-linear-90 from-panel-2 from-25% via-panel-3 via-50% to-panel-2 to-75% bg-size-[200%_100%] \
    animate-shimmer motion-reduce:animate-none";

/// What every count pill shares. Use [`COUNT`] or [`COUNT_NARROW`].
const COUNT_BASE: &str = "inline-flex items-center justify-center text-xs font-semibold \
    tabular-nums leading-[1.4] text-muted bg-panel-3 rounded-full";
/// A count pill.
pub const COUNT: &str = concatcp!(COUNT_BASE, " min-w-6 px-2");
/// A narrower count pill, for a row short of space (the drawer's tabs).
pub const COUNT_NARROW: &str = concatcp!(COUNT_BASE, " min-w-[1.3rem] px-1.5");
/// A status dot: unknown, off.
pub const DOT: &str = "inline-block flex-none size-[0.55rem] rounded-full bg-faint \
    transition-colors duration-(--duration) ease-standard";
/// A status dot: up.
pub const DOT_OK: &str = "inline-block flex-none size-[0.55rem] rounded-full bg-ok \
    shadow-[0_0_0_3px_var(--color-ok-soft)] transition-colors duration-(--duration) ease-standard";
/// A status dot: busy.
pub const DOT_BUSY: &str = "inline-block flex-none size-[0.55rem] rounded-full bg-accent \
    transition-colors duration-(--duration) ease-standard";

/// A progress bar (`<progress>`), accent.
pub const PROGRESS: &str = "w-40 max-xs:w-28 h-2 appearance-none border-none rounded-full \
    overflow-hidden bg-panel-3 [&::-webkit-progress-bar]:bg-panel-3 \
    [&::-webkit-progress-value]:bg-accent [&::-webkit-progress-value]:rounded-full \
    [&::-moz-progress-bar]:bg-accent";

/// A layer over a canvas (loading, failure): centred message on a scrim.
pub const OVERLAY: &str = "absolute inset-0 z-10 flex flex-col items-center justify-center gap-3 \
    p-4 text-center bg-scrim animate-fade-in";

/// A meter: a bar (one of the `METER_BAR_*`) and its numbers ([`METER_TEXT`]).
pub const METER: &str = "grid grid-cols-[minmax(3rem,1fr)_auto] items-center gap-2 w-full";
/// The numbers of a [`METER`].
pub const METER_TEXT: &str = "text-xs text-muted tabular-nums whitespace-nowrap";
/// What every meter bar shares.
const METER_BAR: &str = "w-full h-2 appearance-none border-none rounded-full overflow-hidden \
    bg-panel-3 [&::-webkit-progress-bar]:bg-panel-3 [&::-webkit-progress-value]:rounded-full";
/// The bar of a hit points [`METER`].
pub const METER_BAR_HITS: &str = concatcp!(
    METER_BAR,
    " [&::-webkit-progress-value]:bg-ok [&::-moz-progress-bar]:bg-ok"
);
/// The bar of an energy [`METER`].
pub const METER_BAR_ENERGY: &str = concatcp!(
    METER_BAR,
    " [&::-webkit-progress-value]:bg-res-energy [&::-moz-progress-bar]:bg-res-energy"
);
/// The bar of a progress [`METER`] (controller, construction, spawning).
pub const METER_BAR_PROGRESS: &str = concatcp!(
    METER_BAR,
    " [&::-webkit-progress-value]:bg-info [&::-moz-progress-bar]:bg-info"
);

/// What every tooltip shares: `data-tip` holds the text (the accessible name stays in
/// `aria-label`). It shows after a short pause on hover, at once on keyboard focus, never while
/// the element's menu is open. Use a placed set: [`TIP`], [`TIP_END`], [`TIP_LEFT`],
/// [`TIP_TOP_END`]. Put an element at the right edge of a panel on an `_END` set: a centred
/// tooltip sticks out of the panel and makes it scroll sideways, also while hidden.
/// The element must be positioned: add `relative` unless it is `absolute` or `fixed` already.
const TIP_BASE: &str = "after:content-[attr(data-tip)] after:absolute after:z-20 \
    after:w-max after:max-w-72 after:px-2 after:py-1 after:text-xs after:font-medium \
    after:leading-tight after:whitespace-normal after:text-fg after:text-left \
    after:pointer-events-none after:bg-panel-3 after:border after:border-edge-strong \
    after:rounded-sm after:shadow-sm after:opacity-0 after:invisible \
    after:transition-[opacity,translate,visibility] after:duration-(--duration-fast) \
    after:ease-standard hover:after:opacity-100 hover:after:visible hover:after:delay-450 \
    focus-visible:after:opacity-100 focus-visible:after:visible focus-visible:after:delay-0 \
    aria-expanded:after:hidden active:after:hidden";
/// A tooltip below the element, centred.
pub const TIP: &str = concatcp!(
    TIP_BASE,
    " after:top-[calc(100%+0.5rem)] after:left-1/2 after:-translate-x-1/2 \
    after:-translate-y-[2px] hover:after:translate-y-0 focus-visible:after:translate-y-0"
);
/// A tooltip below the element, aligned to its right edge.
pub const TIP_END: &str = concatcp!(
    TIP_BASE,
    " after:top-[calc(100%+0.5rem)] after:right-0 after:-translate-y-[2px] \
    hover:after:translate-y-0 focus-visible:after:translate-y-0"
);
/// A tooltip left of the element.
pub const TIP_LEFT: &str = concatcp!(
    TIP_BASE,
    " after:top-1/2 after:right-[calc(100%+0.5rem)] after:translate-x-[2px] \
    after:-translate-y-1/2 hover:after:translate-x-0 focus-visible:after:translate-x-0"
);
/// A tooltip above the element, aligned to its right edge.
pub const TIP_TOP_END: &str = concatcp!(
    TIP_BASE,
    " after:bottom-[calc(100%+0.5rem)] after:right-0 after:translate-y-[2px] \
    hover:after:translate-y-0 focus-visible:after:translate-y-0"
);
