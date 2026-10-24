//! Tests for `Operation::FocusOrVirtual` (bound as e.g.
//! `window_virtualfocus_north`/`_south`): focuses a stack neighbor
//! above/below if one exists, otherwise switches the virtual workspace like
//! `Operation::Virtual` would — see `switch_virtual_workspace_bind` in
//! `src/ecs/workspace.rs`. Deliberately a separate command from
//! `Operation::Virtual` (`window_virtual_north`/`_south`), which must keep
//! meaning exactly "switch workspace", unconditionally.

use bevy::prelude::*;

use crate::assert_focused;
use crate::commands::{Command, Direction, Operation};
use crate::ecs::ActiveWorkspaceMarker;
use crate::ecs::layout::LayoutStrip;
use crate::events::Event;

use super::*;

fn active_virtual_index(world: &mut World) -> u32 {
    let mut query = world.query_filtered::<&LayoutStrip, With<ActiveWorkspaceMarker>>();
    query
        .single(world)
        .expect("exactly one active strip")
        .virtual_index
}

#[test]
fn test_focus_or_virtual_south_focuses_stack_sibling_before_switching_workspace() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        Event::Command {
            command: Command::Window(Operation::Focus(Direction::East)),
        },
        Event::Command {
            command: Command::Window(Operation::Stack(true)),
        },
        // Jump to VW1 and straight back so it exists for the eventual
        // fallback switch, without relying on auto-create being enabled.
        Event::Command {
            command: Command::Window(Operation::VirtualNumber(1)),
        },
        Event::Command {
            command: Command::Window(Operation::VirtualNumber(0)),
        },
        // Focus window 0 (top of the 2-item stack), then press South twice.
        Event::Command {
            command: Command::Window(Operation::Focus(Direction::First)),
        },
        Event::Command {
            command: Command::Window(Operation::FocusOrVirtual(Direction::South)),
        },
        Event::Command {
            command: Command::Window(Operation::FocusOrVirtual(Direction::South)),
        },
    ];

    TestHarness::new()
        .with_windows(2)
        .on_iteration(6, |world, _state| {
            // First South: a sibling exists below window 0 in the stack, so
            // focus must move to it instead of switching virtual workspace.
            assert_focused!(world, 1);
            assert_eq!(
                active_virtual_index(world),
                0,
                "focusing a stack sibling must not switch virtual workspace"
            );
        })
        .on_iteration(7, |world, _state| {
            // Second South: window 1 is the last stack item, nothing left to
            // focus, so this must fall through to switching workspaces.
            assert_eq!(
                active_virtual_index(world),
                1,
                "with nothing left to focus, South must fall through to switching workspace"
            );
        })
        .run(commands);
}

#[test]
fn test_focus_or_virtual_north_switches_workspace_immediately_without_a_stack() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        Event::Command {
            command: Command::Window(Operation::VirtualNumber(1)),
        },
        Event::Command {
            command: Command::Window(Operation::FocusOrVirtual(Direction::North)),
        },
    ];

    TestHarness::new()
        .with_windows(1)
        .on_iteration(2, |world, _state| {
            assert_eq!(
                active_virtual_index(world),
                0,
                "with no stack sibling to focus, North must switch virtual workspace"
            );
        })
        .run(commands);
}

/// Regression: `Operation::Virtual` (`window_virtual_north`/`_south`) must
/// keep switching the virtual workspace unconditionally, even when the
/// focused window is in a stack with a focusable neighbor — that
/// focus-or-switch behavior belongs only to the separate
/// `Operation::FocusOrVirtual` command.
#[test]
fn test_plain_virtual_south_switches_workspace_even_with_a_stack_sibling() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        Event::Command {
            command: Command::Window(Operation::Focus(Direction::East)),
        },
        Event::Command {
            command: Command::Window(Operation::Stack(true)),
        },
        Event::Command {
            command: Command::Window(Operation::VirtualNumber(1)),
        },
        Event::Command {
            command: Command::Window(Operation::VirtualNumber(0)),
        },
        Event::Command {
            command: Command::Window(Operation::Focus(Direction::First)),
        },
        Event::Command {
            command: Command::Window(Operation::Virtual(Direction::South)),
        },
    ];

    TestHarness::new()
        .with_windows(2)
        .on_iteration(6, |world, _state| {
            assert_focused!(world, 0);
            assert_eq!(
                active_virtual_index(world),
                1,
                "plain Virtual(South) must switch workspace unconditionally, \
                 not focus the stack sibling"
            );
        })
        .run(commands);
}

/// `virtualopen` steps through workspaces with windows only: an empty one in
/// between is skipped, and past either end it wraps around.
#[test]
fn virtual_open_skips_empty_workspaces_and_wraps_around() {
    use crate::commands::MoveFocus;

    let virtual_command = |operation| Event::Command {
        command: Command::Window(operation),
    };
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        // An empty workspace 2, back on 1, and a window sent on to a new
        // workspace 3.
        virtual_command(Operation::VirtualAdd),
        virtual_command(Operation::VirtualNumber(0)),
        virtual_command(Operation::VirtualMoveNumber(2, MoveFocus::Stay)),
        virtual_command(Operation::VirtualOccupied(Direction::South)),
        virtual_command(Operation::VirtualOccupied(Direction::South)),
        virtual_command(Operation::VirtualOccupied(Direction::North)),
        virtual_command(Operation::VirtualOccupied(Direction::North)),
    ];

    TestHarness::new()
        .with_windows(3)
        .on_iteration(3, |world, _state| {
            assert_eq!(active_virtual_index(world), 0);
        })
        .on_iteration(4, |world, _state| {
            assert_eq!(
                active_virtual_index(world),
                2,
                "the empty workspace 2 is skipped"
            );
        })
        .on_iteration(5, |world, _state| {
            assert_eq!(
                active_virtual_index(world),
                0,
                "down past the last wraps to the first"
            );
        })
        .on_iteration(6, |world, _state| {
            assert_eq!(
                active_virtual_index(world),
                2,
                "up past the first wraps to the last"
            );
        })
        .on_iteration(7, |world, _state| {
            assert_eq!(active_virtual_index(world), 0);
        })
        .run(commands);
}

/// With only one workspace that has windows, up and down stay put.
#[test]
fn virtual_open_with_one_workspace_does_nothing() {
    TestHarness::new()
        .with_windows(2)
        .on_iteration(1, |world, _state| {
            assert_eq!(active_virtual_index(world), 0);
        })
        .on_iteration(2, |world, _state| {
            assert_eq!(active_virtual_index(world), 0);
        })
        .run(vec![
            Event::MenuOpened { window_id: 0 },
            Event::Command {
                command: Command::Window(Operation::VirtualOccupied(Direction::South)),
            },
            Event::Command {
                command: Command::Window(Operation::VirtualOccupied(Direction::North)),
            },
        ]);
}
