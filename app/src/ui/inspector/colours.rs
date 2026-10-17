//! The colour classes of resource swatches and creep body parts.

use crate::ui::style::concatcp;

/// What every resource swatch shares.
const SWATCH: &str = "flex-none size-[0.7rem] rounded-xs";

/// The swatch of a resource's colour: `energy`, `power`, the base mineral of a mineral or
/// compound (`U` for `UH2O`), else muted.
pub fn resource_class(resource: &str) -> &'static str {
    match resource {
        "energy" => concatcp!(SWATCH, " bg-res-energy"),
        "power" => concatcp!(SWATCH, " bg-res-power"),
        "ops" => concatcp!(SWATCH, " bg-res-ops"),
        _ => {
            let base = resource
                .chars()
                .find(|character| matches!(character, 'U' | 'L' | 'K' | 'Z' | 'G'))
                .or_else(|| {
                    resource
                        .chars()
                        .next()
                        .filter(|first| matches!(first, 'H' | 'O' | 'X'))
                });
            match base {
                Some('U') => concatcp!(SWATCH, " bg-res-u"),
                Some('L') => concatcp!(SWATCH, " bg-res-l"),
                Some('K') => concatcp!(SWATCH, " bg-res-k"),
                Some('Z') => concatcp!(SWATCH, " bg-res-z"),
                Some('G') => concatcp!(SWATCH, " bg-res-g"),
                Some('X') => concatcp!(SWATCH, " bg-res-x"),
                Some('H' | 'O') => concatcp!(SWATCH, " bg-res-ho"),
                _ => concatcp!(SWATCH, " bg-muted"),
            }
        }
    }
}

/// What every body part shares: dimmed once dead (`data-dead`), ringed when boosted
/// (`data-boosted`).
pub(super) const PART: &str = "size-[0.85rem] rounded-xs data-dead:opacity-25 \
    data-boosted:shadow-[0_0_0_2px_var(--color-sunken),0_0_0_3px_var(--color-fg)]";

/// The square of a body part of kind `kind` (`move`, `ranged_attack`, …) in its colour, muted
/// for an unknown kind.
pub(super) fn part_class(kind: &str) -> &'static str {
    match kind {
        "move" => concatcp!(PART, " bg-part-move"),
        "work" => concatcp!(PART, " bg-part-work"),
        "carry" => concatcp!(PART, " bg-part-carry"),
        "attack" => concatcp!(PART, " bg-part-attack"),
        "ranged_attack" => concatcp!(PART, " bg-part-ranged-attack"),
        "heal" => concatcp!(PART, " bg-part-heal"),
        "claim" => concatcp!(PART, " bg-part-claim"),
        "tough" => concatcp!(PART, " bg-part-tough"),
        _ => concatcp!(PART, " bg-muted"),
    }
}
