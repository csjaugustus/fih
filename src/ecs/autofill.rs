//! Fills the screen with however few windows a strip has (`auto_fill_width`):
//! the first window on an empty workspace opens at full width, a second one
//! splits the screen with it 50/50, and a window left alone fills the screen
//! again. A window the user has resized is never touched.

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
    fn untouched_at(&self, width: i32) -> bool {
        self.filled
            .is_some_and(|filled| filled == width && (self.width - filled).abs() <= TOLERANCE)
    }

    fn untouched(&self) -> bool {
        self.filled
            .is_some_and(|filled| (self.width - filled).abs() <= TOLERANCE)
    }
}

/// The widths auto-fill gives the columns of a strip, given the full and
/// half widths of its display.
pub(crate) fn plan_auto_fill(columns: &[FillColumn], full: i32, half: i32) -> Vec<(Entity, i32)> {
    match columns {
        [only]
            if only.single && (only.pending || only.untouched() && only.filled != Some(full)) =>
        {
            vec![(only.entity, full)]
        }
        [first, second] if first.single && second.single => {
            let (new, old) = match (first.pending, second.pending) {
                (true, false) => (first, second),
                (false, true) => (second, first),
                _ => return Vec::new(),
            };
            if old.untouched_at(full) {
                vec![(new.entity, half), (old.entity, half)]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
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
        let full = viewport_width;
        let half = round_px(f64::from(edge_width) * 0.5).min(viewport_width);

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

        for (entity, width) in plan_auto_fill(&fill_columns, full, half) {
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

    use super::{FillColumn, plan_auto_fill};

    const FULL: i32 = 1000;
    const HALF: i32 = 500;

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
    fn first_window_fills_the_screen() {
        let only = column(1, true, None, 640);
        assert_eq!(plan_auto_fill(&[only], FULL, HALF), [(only.entity, FULL)]);
    }

    #[test]
    fn second_window_splits_with_an_untouched_first() {
        let old = column(1, false, Some(FULL), FULL);
        let new = column(2, true, None, 640);
        assert_eq!(
            plan_auto_fill(&[old, new], FULL, HALF),
            [(new.entity, HALF), (old.entity, HALF)]
        );
    }

    #[test]
    fn a_resized_first_window_is_left_alone() {
        let old = column(1, false, Some(FULL), 700);
        let new = column(2, true, None, 640);
        assert_eq!(plan_auto_fill(&[old, new], FULL, HALF), []);
    }

    #[test]
    fn a_lone_untouched_window_fills_the_screen_again() {
        let left = column(1, false, Some(HALF), HALF);
        assert_eq!(plan_auto_fill(&[left], FULL, HALF), [(left.entity, FULL)]);
        // Already full: nothing to do, so the strip settles.
        let full = column(1, false, Some(FULL), FULL);
        assert_eq!(plan_auto_fill(&[full], FULL, HALF), []);
    }

    #[test]
    fn a_lone_resized_window_keeps_its_width() {
        let resized = column(1, false, Some(HALF), 620);
        assert_eq!(plan_auto_fill(&[resized], FULL, HALF), []);
        let never_filled = column(1, false, None, 620);
        assert_eq!(plan_auto_fill(&[never_filled], FULL, HALF), []);
    }

    #[test]
    fn third_window_keeps_its_own_width() {
        let columns = [
            column(1, false, Some(HALF), HALF),
            column(2, false, Some(HALF), HALF),
            column(3, true, None, 640),
        ];
        assert_eq!(plan_auto_fill(&columns, FULL, HALF), []);
    }
}
