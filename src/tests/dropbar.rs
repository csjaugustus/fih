use bevy::ecs::query::With;
use bevy::ecs::world::World;
use bevy::math::IRect;
use objc2_core_foundation::CGPoint;

use crate::commands::Command;
use crate::dropbar::PaneDrag;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::workspace::DropTarget;
use crate::ecs::{ActiveWorkspaceMarker, Bounds, Position, SelectedVirtualMarker};
use crate::events::Event;
use crate::manager::Origin;
use crate::platform::Modifiers;

use super::*;

fn tick() -> Event {
    Event::Command {
        command: Command::PrintState,
    }
}

fn point(origin: Origin) -> CGPoint {
    CGPoint::new(origin.x.into(), origin.y.into())
}

/// The middle of window `id`'s frame.
fn window_center(world: &mut World, id: crate::platform::WinID) -> Origin {
    let entity = find_window_entity(id, world);
    let Position(origin) = *world.get::<Position>(entity).expect("position");
    let Bounds(size) = *world.get::<Bounds>(entity).expect("bounds");
    origin + size / 2
}

/// The virtual index of the strip holding window `id`, and whether it is the
/// active one.
fn strip_of(world: &mut World, id: crate::platform::WinID) -> (u32, bool) {
    let entity = find_window_entity(id, world);
    let mut strips = world.query::<(&LayoutStrip, bevy::ecs::query::Has<ActiveWorkspaceMarker>)>();
    strips
        .iter(world)
        .find(|(strip, _)| strip.contains(entity))
        .map(|(strip, active)| (strip.virtual_index, active))
        .expect("window in a strip")
}

fn active_strip_len(world: &mut World) -> usize {
    let mut query = world.query_filtered::<&LayoutStrip, With<ActiveWorkspaceMarker>>();
    query.single(world).expect("one active strip").len()
}

fn mouse(world: &mut World, event: fn(CGPoint) -> Event, at: Origin) {
    world.write_message(event(point(at)));
}

fn down(point: CGPoint) -> Event {
    Event::MouseDown {
        point,
        modifiers: Modifiers::empty(),
    }
}

fn dragged(point: CGPoint) -> Event {
    Event::MouseDragged {
        point,
        modifiers: Modifiers::empty(),
    }
}

fn up(point: CGPoint) -> Event {
    Event::MouseUp {
        point,
        modifiers: Modifiers::empty(),
    }
}

/// Presses on window 0 and drags it 100 points left, which brings up the bar
/// by iteration 2. Returns the harness to add the rest to.
fn drag_window_zero(harness: TestHarness) -> TestHarness {
    harness
        .on_iteration(0, |world, _state| {
            let center = window_center(world, 0);
            mouse(world, down, center);
        })
        .on_iteration(1, |world, state| {
            let center = window_center(world, 0);
            let entity = find_window_entity(0, world);
            let Position(origin) = *world.get::<Position>(entity).expect("position");
            state.os_move_window(0, origin - Origin::new(100, 0));
            mouse(world, dragged, center - Origin::new(100, 0));
        })
        .on_iteration(2, |world, _state| {
            let drag = world.resource::<PaneDrag>();
            assert!(
                drag.bar().is_some(),
                "the window's own frame shows it moving with the pointer"
            );
            assert!(drag.dragging(), "which hides the focus border");
        })
}

#[test]
fn dragging_a_window_shows_occupied_workspaces_and_one_empty_slot() {
    drag_window_zero(TestHarness::new().with_windows(3))
        .on_iteration(3, |world, _state| {
            let drag = world.resource::<PaneDrag>();
            let bar = drag.bar().expect("dragging the window brings up the bar");
            assert_eq!(bar.groups.len(), 1, "one display");
            let slots = &bar.groups[0].slots;
            assert_eq!(slots.len(), 2, "the workspace with windows, and one empty");
            assert!(slots[0].occupied && slots[0].active);
            assert_eq!(slots[0].number, Some(1));
            assert!(!slots[1].occupied);
            assert!(matches!(slots[1].target, DropTarget::NewOn(_)));
            assert_eq!(bar.hovered, None, "the pointer is down on the window");
        })
        .run(vec![tick(), tick(), tick(), tick()]);
}

#[test]
fn dropping_a_window_on_the_empty_slot_moves_it_to_a_new_workspace() {
    drag_window_zero(TestHarness::new().with_windows(3))
        .on_iteration(3, |world, _state| {
            let slot = {
                let drag = world.resource::<PaneDrag>();
                let bar = drag.bar().expect("bar up");
                bar.geometry.groups[0].1[1].center()
            };
            mouse(world, dragged, slot);
        })
        .on_iteration(4, |world, _state| {
            let (slot, hovered) = {
                let drag = world.resource::<PaneDrag>();
                let bar = drag.bar().expect("bar up");
                (bar.geometry.groups[0].1[1].center(), bar.hovered)
            };
            assert_eq!(hovered, Some((0, 1)), "the empty slot is under the pointer");
            mouse(world, up, slot);
        })
        .on_iteration(5, |world, _state| {
            assert!(world.resource::<PaneDrag>().bar().is_none(), "the bar goes");
            assert_eq!(
                strip_of(world, 0),
                (1, false),
                "onto a new workspace, behind"
            );
            assert_eq!(strip_of(world, 1), (0, true), "the rest stay in front");
            assert_eq!(active_strip_len(world), 2);
        })
        .run(vec![tick(), tick(), tick(), tick(), tick(), tick()]);
}

#[test]
fn letting_go_away_from_the_bar_keeps_the_window_in_place() {
    drag_window_zero(TestHarness::new().with_windows(3))
        .on_iteration(3, |world, _state| {
            let center = window_center(world, 0);
            mouse(world, up, center);
        })
        .on_iteration(4, |world, _state| {
            let drag = world.resource::<PaneDrag>();
            assert!(drag.bar().is_none());
            assert!(!drag.dragging(), "the focus border comes back");
            assert_eq!(strip_of(world, 0), (0, true));
            assert_eq!(active_strip_len(world), 3);
        })
        .run(vec![tick(), tick(), tick(), tick(), tick()]);
}

#[test]
fn each_display_gets_its_own_slots_and_its_empty_one_opens_a_workspace_there() {
    let harness = TestHarness::new()
        .with_display(
            EXT_DISPLAY_ID,
            IRect::new(0, -EXT_DISPLAY_HEIGHT, EXT_DISPLAY_WIDTH, 0),
            vec![EXT_WORKSPACE_ID],
        )
        .with_windows(3)
        .with_workspace_window(100, EXT_WORKSPACE_ID, |window| {
            window.workspace_id = EXT_WORKSPACE_ID;
        });
    drag_window_zero(harness)
        .on_iteration(3, |world, _state| {
            let slot = {
                let drag = world.resource::<PaneDrag>();
                let bar = drag.bar().expect("bar up");
                // The external display sits above, so it is numbered first.
                let numbers = bar
                    .groups
                    .iter()
                    .map(|group| group.slots.iter().map(|slot| slot.number).collect())
                    .collect::<Vec<Vec<_>>>();
                assert_eq!(numbers, [[Some(1), None], [Some(2), None]]);
                assert!(bar.groups[1].slots[0].active);
                bar.geometry.groups[0].1[1].center()
            };
            mouse(world, dragged, slot);
        })
        .on_iteration(4, |world, _state| {
            let slot = {
                let drag = world.resource::<PaneDrag>();
                let bar = drag.bar().expect("bar up");
                assert_eq!(bar.hovered, Some((0, 1)));
                bar.geometry.groups[0].1[1].center()
            };
            mouse(world, up, slot);
        })
        .on_iteration(5, |world, _state| {
            let entity = find_window_entity(0, world);
            let mut strips =
                world.query::<(&LayoutStrip, bevy::ecs::query::Has<SelectedVirtualMarker>)>();
            let (strip, selected) = strips
                .iter(world)
                .find(|(strip, _)| strip.contains(entity))
                .expect("window 0 in a strip");
            assert_eq!(strip.id(), EXT_WORKSPACE_ID, "onto the external display");
            assert_eq!(strip.virtual_index, 1, "in a new workspace there");
            assert!(!selected, "not the one the display shows");
            assert_eq!(active_strip_len(world), 2);
            let other = find_window_entity(100, world);
            let shown = strips
                .iter(world)
                .find(|(strip, _)| strip.contains(other))
                .is_some_and(|(_, selected)| selected);
            assert!(shown, "the external display keeps showing what it was");
        })
        .run(vec![tick(), tick(), tick(), tick(), tick(), tick()]);
}

#[test]
fn dragging_inside_a_window_that_stays_put_shows_no_bar() {
    TestHarness::new()
        .with_windows(3)
        .on_iteration(0, |world, _state| {
            let center = window_center(world, 0);
            mouse(world, down, center);
        })
        .on_iteration(1, |world, _state| {
            // Selecting text: the pointer moves, the window does not.
            let center = window_center(world, 0);
            mouse(world, dragged, center - Origin::new(100, 0));
        })
        .on_iteration(2, |world, _state| {
            let drag = world.resource::<PaneDrag>();
            assert!(drag.bar().is_none());
            assert!(!drag.dragging(), "the border stays while selecting text");
        })
        .run(vec![tick(), tick(), tick()]);
}
