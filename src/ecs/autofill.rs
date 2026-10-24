//! Fills the screen with a strip's windows, a column at the least
//! (`auto_fill_width`). Every tiled window carries a [`WidthIntent`]: windows
//! that fill share the screen among themselves - one alone is full width,
//! fewer than the columns split it evenly, more are a column wide each (half
//! the screen with 2 columns, a third with 3) - and are resized whenever
//! windows come or go or the column count changes, so there is never a gap.
//! A window made full width on purpose, or sized by hand, keeps its width.

use bevy::ecs::change_detection::DetectChanges;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::query::{Changed, Or};
use bevy::ecs::system::{Commands, Query, Res};
use bevy::ecs::world::Ref;
use bevy::prelude::Component;

use crate::config::Config;
use crate::ecs::columns::ColumnChoices;
use crate::ecs::layout::{Column, LayoutStrip};
use crate::ecs::params::Windows;
use crate::ecs::{DockPosition, SpawnCommandsExt};
use crate::manager::{Display, Size};
use crate::util::round_px;

/// How a tiled window's width is decided.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidthIntent {
    /// Shares the screen with the other filling windows, down to a column.
    /// A window that fills alone is full width only as a fallback.
    Fill,
    /// Full width on purpose: windows coming and going and column changes
    /// leave it be. Only resetting the widths makes it fill again.
    Full,
    /// Sized by hand or by a window rule, and left at that.
    Manual,
}

/// Widths a window may be off its target and still count as there (rounding,
/// padding).
const TOLERANCE: i32 = 2;

/// What auto-fill needs to know about one column of a strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FillColumn {
    pub entity: Entity,
    /// A stacked or tabbed column is never resized by auto-fill.
    pub single: bool,
    pub intent: WidthIntent,
    /// Its current (or pending) width.
    pub width: i32,
}

/// The width each of `windows` filling windows gets in a strip laid out in
/// `columns` columns, beside other windows `others` wide: a column, or more
/// when that would leave the screen part empty, the filling windows then
/// sharing what the others leave. `edge` is the display's usable width the
/// columns are of, `full` the viewport a window may fill.
pub(crate) fn fill_width(windows: usize, columns: u8, edge: i32, full: i32, others: i32) -> i32 {
    #[allow(clippy::cast_precision_loss)]
    let windows = windows.max(1) as f64;
    let column = f64::from(edge) / f64::from(columns.max(1));
    let share = f64::from((edge - others).max(0)) / windows;
    let width = round_px(column.max(share));
    // Within a pixel or two of the viewport is all of it.
    if width >= full - TOLERANCE {
        full
    } else {
        width
    }
}

/// The intent of a window that had none, such as one already open when fih
/// started: full width beside other windows reads as full on purpose,
/// anything else as filling.
pub(crate) fn infer_intent(width: i32, full: i32, columns: usize) -> WidthIntent {
    if columns > 1 && width >= full - TOLERANCE {
        WidthIntent::Full
    } else {
        WidthIntent::Fill
    }
}

/// The resizes auto-fill makes: every filling window not yet at `target`.
pub(crate) fn plan_auto_fill(columns: &[FillColumn], target: i32) -> Vec<(Entity, i32)> {
    columns
        .iter()
        .filter(|column| column.single && column.intent == WidthIntent::Fill)
        .filter(|column| (column.width - target).abs() > TOLERANCE)
        .map(|column| (column.entity, target))
        .collect()
}

/// Strips that changed, or moved to another display.
type MovedOrChangedStrips<'w, 's> = Query<'w, 's, (), Or<(Changed<LayoutStrip>, Changed<ChildOf>)>>;

/// Whether a strip changed or moved display, a window's width intent changed,
/// or the column choices or config did, since auto-fill last ran.
pub(crate) fn fill_inputs_changed(
    strips: MovedOrChangedStrips,
    intents: Query<(), Changed<WidthIntent>>,
    choices: Res<ColumnChoices>,
    config: Res<Config>,
) -> bool {
    !strips.is_empty() || !intents.is_empty() || choices.is_changed() || config.is_changed()
}

/// Resizes the filling windows of every strip that changed, or holds a window
/// whose intent changed, as [`plan_auto_fill`] says.
#[allow(clippy::too_many_arguments)]
pub(crate) fn auto_fill_widths(
    strips: Query<(Ref<LayoutStrip>, Ref<ChildOf>)>,
    choices: Res<ColumnChoices>,
    changed_intents: Query<Entity, Changed<WidthIntent>>,
    intents: Query<&WidthIntent>,
    displays: Query<(&Display, Option<&DockPosition>)>,
    windows: Windows,
    config: Res<Config>,
    mut commands: Commands,
) {
    // A new column choice or config can change any strip's columns.
    let everything = choices.is_changed() || config.is_changed();
    for (strip, child_of) in &strips {
        // Moving to another display can change its columns too.
        if !everything
            && !strip.is_changed()
            && !child_of.is_changed()
            && !changed_intents.iter().any(|entity| strip.contains(entity))
        {
            continue;
        }
        let Ok((display, dock)) = displays.get(child_of.parent()) else {
            continue;
        };
        // Ratios are of the display's usable width, as the resize commands
        // take them; a pinned window caps how wide a column can get.
        let edge_width = display.edge_bounds(dock, &config).width();
        let viewport_width = display.actual_display_bounds(dock, &config).width();

        let column_count = strip.columns().count();
        let fill_columns = strip
            .columns()
            .filter_map(|column| {
                let entity = column.top()?;
                let width = windows.moving_frame(entity)?.width();
                let intent = intents.get(entity).copied().unwrap_or_else(|_| {
                    let intent = infer_intent(width, viewport_width, column_count);
                    if let Ok(mut entity_commands) = commands.get_entity(entity) {
                        entity_commands.try_insert(intent);
                    }
                    intent
                });
                Some(FillColumn {
                    entity,
                    single: matches!(column, Column::Single(_)),
                    intent,
                    width,
                })
            })
            .collect::<Vec<_>>();

        let (filling, others): (Vec<&FillColumn>, Vec<&FillColumn>) = fill_columns
            .iter()
            .partition(|column| column.single && column.intent == WidthIntent::Fill);
        let target = fill_width(
            filling.len(),
            choices.count(&strip, &display.screen(), &config),
            edge_width,
            viewport_width,
            others.iter().map(|column| column.width).sum(),
        );
        for (entity, width) in plan_auto_fill(&fill_columns, target) {
            let Some(frame) = windows.frame(entity) else {
                continue;
            };
            commands.resize_entity(entity, Size::new(width, frame.height()));
            commands.reshuffle_around(entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::entity::Entity;

    use super::{FillColumn, WidthIntent, fill_width, infer_intent, plan_auto_fill};

    const FULL: i32 = 1200;
    const HALF: i32 = 600;
    const THIRD: i32 = 400;

    fn column(index: u32, intent: WidthIntent, width: i32) -> FillColumn {
        FillColumn {
            entity: Entity::from_raw_u32(index).unwrap(),
            single: true,
            intent,
            width,
        }
    }

    #[test]
    fn windows_share_the_screen_down_to_a_column() {
        // Two columns: full, then halves however many there are.
        assert_eq!(fill_width(1, 2, FULL, FULL, 0), FULL);
        assert_eq!(fill_width(2, 2, FULL, FULL, 0), HALF);
        assert_eq!(fill_width(5, 2, FULL, FULL, 0), HALF);
        // Three columns: full, halves, then thirds.
        assert_eq!(fill_width(1, 3, FULL, FULL, 0), FULL);
        assert_eq!(fill_width(2, 3, FULL, FULL, 0), HALF);
        assert_eq!(fill_width(3, 3, FULL, FULL, 0), THIRD);
        assert_eq!(fill_width(4, 3, FULL, FULL, 0), THIRD);
    }

    #[test]
    fn beside_other_windows_filling_ones_take_only_the_empty_space() {
        // A full-width window leaves no empty space: filling ones are a column.
        assert_eq!(fill_width(2, 3, FULL, FULL, FULL), THIRD);
        assert_eq!(fill_width(1, 2, FULL, FULL, FULL), HALF);
        // A narrower window leaves room the filling one takes.
        assert_eq!(fill_width(1, 3, FULL, FULL, THIRD), 2 * THIRD);
    }

    #[test]
    fn filling_windows_are_resized_to_their_share() {
        let columns = [
            column(1, WidthIntent::Fill, FULL),
            column(2, WidthIntent::Fill, 640),
        ];
        assert_eq!(
            plan_auto_fill(&columns, HALF),
            [(columns[0].entity, HALF), (columns[1].entity, HALF)]
        );
    }

    #[test]
    fn full_and_manual_windows_keep_their_width() {
        let columns = [
            column(1, WidthIntent::Full, FULL),
            column(2, WidthIntent::Manual, 700),
            column(3, WidthIntent::Fill, THIRD),
        ];
        assert_eq!(plan_auto_fill(&columns, HALF), [(columns[2].entity, HALF)]);
    }

    #[test]
    fn windows_already_at_their_share_stay() {
        let columns = [
            column(1, WidthIntent::Fill, HALF),
            column(2, WidthIntent::Fill, HALF + 1),
        ];
        assert_eq!(plan_auto_fill(&columns, HALF), []);
    }

    #[test]
    fn full_width_beside_other_windows_reads_as_on_purpose() {
        assert_eq!(infer_intent(FULL, FULL, 2), WidthIntent::Full);
        // Alone, full width is just filling.
        assert_eq!(infer_intent(FULL, FULL, 1), WidthIntent::Fill);
        assert_eq!(infer_intent(HALF, FULL, 2), WidthIntent::Fill);
    }
}
