//! One numbering for the virtual workspaces of every display, so the
//! keyboard reaches all of them: displays in their arrangement, left to right
//! (then top to bottom), and each display's workspaces in order, on the macOS
//! Space it is showing. Adding a workspace on one display shifts the numbers
//! of those after it.

use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::Has;
use bevy::ecs::system::{Commands, Query, Res, SystemParam};

use crate::commands::{Direction, Operation, filter_window_operations};
use crate::ecs::focus::FocusHistory;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::params::ActiveDisplay;
use crate::ecs::{ActiveWorkspaceMarker, SelectedVirtualMarker, SpawnCommandsExt};
use crate::events::Event;
use crate::manager::{Display, WindowManager};

/// One workspace in the combined numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NumberedWorkspace {
    pub strip: Entity,
    pub display: Entity,
    pub occupied: bool,
    pub active: bool,
}

/// A workspace as [`order_workspaces`] sorts it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkspaceEntry {
    /// The top-left of its display, in the display arrangement.
    pub display_origin: (i32, i32),
    pub virtual_index: u32,
    pub workspace: NumberedWorkspace,
}

/// Puts workspaces in their combined order: by display, left to right then
/// top to bottom, and by virtual index on each.
pub(crate) fn order_workspaces(mut entries: Vec<WorkspaceEntry>) -> Vec<NumberedWorkspace> {
    entries.sort_by_key(|entry| (entry.display_origin, entry.virtual_index));
    entries.into_iter().map(|entry| entry.workspace).collect()
}

/// The next workspace with windows from `current`, south (down) or north
/// (up), wrapping around past either end.
pub(crate) fn next_occupied(
    workspaces: &[NumberedWorkspace],
    current: usize,
    south: bool,
) -> Option<usize> {
    let occupied = workspaces
        .iter()
        .enumerate()
        .filter(|(index, workspace)| *index != current && workspace.occupied)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if south {
        occupied
            .iter()
            .find(|index| **index > current)
            .or(occupied.first())
            .copied()
    } else {
        occupied
            .iter()
            .rev()
            .find(|index| **index < current)
            .or(occupied.last())
            .copied()
    }
}

/// Every strip, its display, and whether it is the active or the selected
/// one on its Space.
type NumberedStrips<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static LayoutStrip,
        &'static ChildOf,
        Has<ActiveWorkspaceMarker>,
        Has<SelectedVirtualMarker>,
    ),
>;

/// The workspaces of every display in one numbering.
#[derive(SystemParam)]
pub(crate) struct CombinedWorkspaces<'w, 's> {
    strips: NumberedStrips<'w, 's>,
    displays: Query<'w, 's, (Entity, &'static Display)>,
    window_manager: Res<'w, WindowManager>,
}

impl CombinedWorkspaces<'_, '_> {
    /// Every display's workspaces, on the Space it is showing, in order.
    pub fn list(&self) -> Vec<NumberedWorkspace> {
        let entries = self
            .displays
            .iter()
            .flat_map(|(display_entity, display)| {
                // The Space the display shows: macOS's word for it, or else
                // the one holding the display's active or selected strip.
                let space = self
                    .window_manager
                    .active_display_space(display.id())
                    .ok()
                    .or_else(|| {
                        self.strips
                            .iter()
                            .filter(|(_, _, child_of, _, _)| child_of.parent() == display_entity)
                            .max_by_key(|(_, _, _, active, selected)| (*active, *selected))
                            .map(|(_, strip, _, _, _)| strip.id())
                    });
                let origin = display.bounds().min;
                self.strips
                    .iter()
                    .filter(move |(_, strip, child_of, _, _)| {
                        child_of.parent() == display_entity && Some(strip.id()) == space
                    })
                    .map(move |(strip_entity, strip, _, active, _)| WorkspaceEntry {
                        display_origin: (origin.x, origin.y),
                        virtual_index: strip.virtual_index,
                        workspace: NumberedWorkspace {
                            strip: strip_entity,
                            display: display_entity,
                            occupied: !strip.all_windows().is_empty(),
                            active,
                        },
                    })
            })
            .collect();
        order_workspaces(entries)
    }

    pub fn strip(&self, entity: Entity) -> Option<&LayoutStrip> {
        self.strips
            .get(entity)
            .ok()
            .map(|(_, strip, _, _, _)| strip)
    }

    pub fn display(&self, entity: Entity) -> Option<&Display> {
        self.displays.get(entity).ok().map(|(_, display)| display)
    }
}

/// Switches to a workspace by its combined number (`alt + N`), or to the
/// next one with windows up or down, on whichever display it is. Focus moves
/// to that display with it. A number past the last opens a new workspace on
/// the display in front.
pub(crate) fn switch_combined_workspace(
    mut messages: MessageReader<Event>,
    workspaces: CombinedWorkspaces,
    active_display: ActiveDisplay,
    focus_history: Res<FocusHistory>,
    mut commands: Commands,
) {
    let Some(operation) = filter_window_operations(&mut messages, |op| {
        matches!(
            op,
            Operation::VirtualNumber(_)
                | Operation::VirtualOccupied(Direction::North | Direction::South)
        )
    })
    .next() else {
        return;
    };

    let list = workspaces.list();
    let Some(current) = list.iter().position(|workspace| workspace.active) else {
        return;
    };
    let target = match operation {
        Operation::VirtualNumber(number) => {
            let number = usize::try_from(*number).unwrap_or(usize::MAX);
            if number >= list.len() {
                open_workspace(&workspaces, &active_display, &mut commands);
                return;
            }
            number
        }
        Operation::VirtualOccupied(direction) => {
            let Some(target) = next_occupied(&list, current, *direction == Direction::South) else {
                return;
            };
            target
        }
        _ => return,
    };
    if target == current {
        return;
    }

    let target = list[target];
    if let Ok(mut entity_commands) = commands.get_entity(target.strip) {
        entity_commands.try_insert(ActiveWorkspaceMarker);
    }
    if target.display == list[current].display {
        return;
    }
    // Across displays, focus has to follow: onto the workspace's last-focused
    // window, or with nothing to focus, the pointer onto the display.
    let Some(strip) = workspaces.strip(target.strip) else {
        return;
    };
    let window = focus_history
        .last_managed(strip.id())
        .filter(|entity| strip.contains(*entity))
        .or_else(|| strip.all_columns().into_iter().next());
    if let Some(window) = window {
        commands.focus_entity(window, true);
        commands.reshuffle_around(window);
    } else if let Some(display) = workspaces.display(target.display) {
        workspaces
            .window_manager
            .warp_mouse(display.bounds().center());
    }
}

/// Opens a new, empty workspace after the last one on the display in front,
/// and switches to it.
fn open_workspace(
    workspaces: &CombinedWorkspaces,
    active_display: &ActiveDisplay,
    commands: &mut Commands,
) {
    let space = active_display.active_strip().id();
    let next_index = workspaces
        .strips
        .iter()
        .filter(|(_, strip, _, _, _)| strip.id() == space)
        .map(|(_, strip, _, _, _)| strip.virtual_index)
        .max()
        .map_or(0, |index| index + 1);
    commands.spawn_layout_strip(
        LayoutStrip::new(space, next_index),
        active_display.bounds().min,
        active_display.entity(),
        true,
    );
}

#[cfg(test)]
mod tests {
    use bevy::ecs::entity::Entity;

    use super::{NumberedWorkspace, WorkspaceEntry, next_occupied, order_workspaces};

    fn entry(display: u32, origin: (i32, i32), index: u32, occupied: bool) -> WorkspaceEntry {
        WorkspaceEntry {
            display_origin: origin,
            virtual_index: index,
            workspace: NumberedWorkspace {
                strip: Entity::from_raw_u32(display * 100 + index).unwrap(),
                display: Entity::from_raw_u32(display).unwrap(),
                occupied,
                active: false,
            },
        }
    }

    #[test]
    fn displays_are_numbered_left_to_right_then_by_workspace() {
        // A laptop on the right with one workspace, a monitor on the left
        // with two.
        let ordered = order_workspaces(vec![
            entry(1, (1920, 0), 0, true),
            entry(2, (0, 0), 1, true),
            entry(2, (0, 0), 0, true),
        ]);
        let strips = ordered
            .iter()
            .map(|workspace| workspace.strip.index_u32())
            .collect::<Vec<_>>();
        assert_eq!(strips, [200, 201, 100]);
    }

    #[test]
    fn up_and_down_skip_empty_workspaces_and_wrap_across_displays() {
        let list = order_workspaces(vec![
            entry(2, (0, 0), 0, true),
            entry(2, (0, 0), 1, false),
            entry(1, (1920, 0), 0, true),
        ]);
        assert_eq!(
            next_occupied(&list, 0, true),
            Some(2),
            "skips the empty one"
        );
        assert_eq!(
            next_occupied(&list, 2, true),
            Some(0),
            "wraps past the last"
        );
        assert_eq!(
            next_occupied(&list, 0, false),
            Some(2),
            "wraps past the first"
        );
        let alone = order_workspaces(vec![entry(1, (0, 0), 0, true)]);
        assert_eq!(next_occupied(&alone, 0, true), None);
    }
}
