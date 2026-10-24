//! Fills the screen with a strip's windows, a column at the least
//! (`auto_fill_width`): with fewer windows than columns they share the screen
//! (one window full, two halves), with as many or more each is a column wide
//! (half the screen with 2 columns, a third with 3). Opening or closing a
//! window resizes the rest to match. A window the user sized differently,
//! say to full width on purpose, is left alone.

use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::query::{Changed, Has, With};
use bevy::ecs::system::{Commands, Populated, Query, Res};
use bevy::prelude::Component;

use crate::config::Config;
use crate::ecs::layout::{Column, LayoutStrip};
use crate::ecs::params::Windows;
use crate::ecs::{DockPosition, SpawnCommandsExt};
use crate::manager::{Display, Size};
use crate::util::round_px;

/// A window that just joined a strip and may be sized by auto-fill.
#[derive(Component, Debug)]
pub struct AutoFillPending;

/// The width, in pixels, auto-fill last gave a window. A window whose width
/// no longer matches has been resized by the user and is left alone.
#[derive(Component, Debug)]
pub struct AutoFillWidth(pub i32);

/// Widths a window may drift from the one auto-fill gave it and still count
/// as untouched (rounding, padding).
const TOLERANCE: i32 = 2;

/// What auto-fill needs to know about one column of a strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FillColumn {
    pub entity: Entity,
    /// A stacked or tabbed column is never resized by auto-fill.
    pub single: bool,
    /// The window just joined the strip.
    pub pending: bool,
    /// The width auto-fill last gave it, if any.
    pub filled: Option<i32>,
    /// Its current (or pending) width.
    pub width: i32,
}

impl FillColumn {
    fn untouched(&self) -> bool {
        self.filled
            .is_some_and(|filled| (self.width - filled).abs() <= TOLERANCE)
    }
}

/// The width each window gets with `windows` of them in a strip laid out in
/// `columns` columns: an equal share of the screen, but never less than a
/// column. `edge` is the display's usable width the shares are of, `full`
/// the viewport a window may fill.
pub(crate) fn fill_width(windows: usize, columns: u8, edge: i32, full: i32) -> i32 {
    let share = windows.clamp(1, usize::from(columns.max(1)));
    #[allow(clippy::cast_precision_loss)]
    let width = round_px(f64::from(edge) / share as f64);
    if share == 1 { full } else { width.min(full) }
}

/// The widths auto-fill gives the columns of a strip: [`fill_width`] for each
/// window that just joined, or that is still the width auto-fill gave it.
pub(crate) fn plan_auto_fill(columns: &[FillColumn], target: i32) -> Vec<(Entity, i32)> {
    columns
        .iter()
        .filter(|column| column.single && (column.pending || column.untouched()))
        .filter(|column| column.pending || (column.width - target).abs() > TOLERANCE)
        .map(|column| (column.entity, target))
        .collect()
}

/// Resizes the columns of every changed strip as [`plan_auto_fill`] says.
pub(crate) fn auto_fill_widths(
    strips: Populated<(&LayoutStrip, &ChildOf), Changed<LayoutStrip>>,
    displays: Query<(&Display, Option<&DockPosition>)>,
    columns: Query<(Has<AutoFillPending>, Option<&AutoFillWidth>)>,
    pending: Query<Entity, With<AutoFillPending>>,
    windows: Windows,
    config: Res<Config>,
    mut commands: Commands,
) {
    for (strip, child_of) in &strips {
        let Ok((display, dock)) = displays.get(child_of.parent()) else {
            continue;
        };
        // Ratios are of the display's usable width, as the resize commands
        // take them; a pinned window caps how wide a column can get.
        let edge_width = display.edge_bounds(dock, &config).width();
        let viewport_width = display.actual_display_bounds(dock, &config).width();
        let target = fill_width(
            strip.columns().count(),
            strip.column_count(),
            edge_width,
            viewport_width,
        );

        let fill_columns = strip
            .columns()
            .filter_map(|column| {
                let entity = column.top()?;
                let (is_pending, filled) = columns.get(entity).ok()?;
                Some(FillColumn {
                    entity,
                    single: matches!(column, Column::Single(_)),
                    pending: is_pending,
                    filled: filled.map(|filled| filled.0),
                    width: windows.moving_frame(entity)?.width(),
                })
            })
            .collect::<Vec<_>>();

        for (entity, width) in plan_auto_fill(&fill_columns, target) {
            let Some(frame) = windows.frame(entity) else {
                continue;
            };
            commands.resize_entity(entity, Size::new(width, frame.height()));
            commands.reshuffle_around(entity);
            if let Ok(mut entity_commands) = commands.get_entity(entity) {
                entity_commands.try_insert(AutoFillWidth(width));
            }
        }

        // A window gets one chance, when it joins; after that only a window
        // auto-fill sized is ever resized again.
        for column in &fill_columns {
            if column.pending
                && let Ok(mut entity_commands) = commands.get_entity(column.entity)
            {
                entity_commands.try_remove::<AutoFillPending>();
            }
        }
    }

    // Pending windows that ended up in no strip (floated, closed) drop out.
    for entity in &pending {
        if !strips.iter().any(|(strip, _)| strip.contains(entity))
            && let Ok(mut entity_commands) = commands.get_entity(entity)
        {
            entity_commands.try_remove::<AutoFillPending>();
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::entity::Entity;

    use super::{FillColumn, fill_width, plan_auto_fill};

    const FULL: i32 = 1200;
    const HALF: i32 = 600;
    const THIRD: i32 = 400;

    fn column(index: u32, pending: bool, filled: Option<i32>, width: i32) -> FillColumn {
        FillColumn {
            entity: Entity::from_raw_u32(index).unwrap(),
            single: true,
            pending,
            filled,
            width,
        }
    }

    #[test]
    fn windows_share_the_screen_down_to_a_column() {
        // Two columns: full, then halves however many there are.
        assert_eq!(fill_width(1, 2, FULL, FULL), FULL);
        assert_eq!(fill_width(2, 2, FULL, FULL), HALF);
        assert_eq!(fill_width(5, 2, FULL, FULL), HALF);
        // Three columns: full, halves, then thirds.
        assert_eq!(fill_width(1, 3, FULL, FULL), FULL);
        assert_eq!(fill_width(2, 3, FULL, FULL), HALF);
        assert_eq!(fill_width(3, 3, FULL, FULL), THIRD);
        assert_eq!(fill_width(4, 3, FULL, FULL), THIRD);
    }

    #[test]
    fn first_window_fills_the_screen() {
        let only = column(1, true, None, 640);
        assert_eq!(plan_auto_fill(&[only], FULL), [(only.entity, FULL)]);
    }

    #[test]
    fn a_new_window_narrows_the_untouched_ones() {
        let old = column(1, false, Some(FULL), FULL);
        let new = column(2, true, None, 640);
        assert_eq!(
            plan_auto_fill(&[old, new], HALF),
            [(old.entity, HALF), (new.entity, HALF)]
        );
    }

    #[test]
    fn closing_one_of_three_thirds_widens_the_rest_to_halves() {
        let left = [
            column(1, false, Some(THIRD), THIRD),
            column(2, false, Some(THIRD), THIRD),
        ];
        assert_eq!(
            plan_auto_fill(&left, HALF),
            [(left[0].entity, HALF), (left[1].entity, HALF)]
        );
    }

    #[test]
    fn a_window_sized_on_purpose_is_left_alone() {
        // Made full width by hand: auto-fill last gave it a half.
        let full = column(1, false, Some(HALF), FULL);
        let new = column(2, true, None, 640);
        assert_eq!(plan_auto_fill(&[full, new], HALF), [(new.entity, HALF)]);
        let never_filled = column(1, false, None, 620);
        assert_eq!(plan_auto_fill(&[never_filled], FULL), []);
    }

    #[test]
    fn windows_already_at_their_width_stay() {
        let settled = [
            column(1, false, Some(HALF), HALF),
            column(2, false, Some(HALF), HALF),
        ];
        assert_eq!(plan_auto_fill(&settled, HALF), []);
    }
}
