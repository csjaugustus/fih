//! Tests for `Operation::Pin` (`window_pin_left` / `window_pin_right`): a
//! window held at one edge of the display while the strip scrolls in the
//! remaining space. The pin belongs to its strip's virtual workspace.

use bevy::prelude::*;

use crate::assert_focused;
use crate::commands::{Command, Direction, MoveFocus, Operation};
use crate::config::Config;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::{ActiveDisplayMarker, ActiveWorkspaceMarker};
use crate::events::Event;
use crate::manager::{Display, Window};

use super::*;

/// Half the 1024px test display.
const PIN_WIDTH: i32 = TEST_DISPLAY_WIDTH / 2;
const VIEWPORT_HEIGHT: i32 = TEST_DISPLAY_HEIGHT - TEST_MENUBAR_HEIGHT;

fn frame(world: &mut World, id: i32) -> IRect {
    let mut query = world.query::<&Window>();
    query
        .iter(world)
        .find(|window| window.id() == id)
        .unwrap_or_else(|| panic!("window {id} not found"))
        .frame()
}

/// The active strip's pin, and the column of window `id` (if it has one).
fn pin_and_column(world: &mut World, id: i32) -> (Option<crate::ecs::layout::Pin>, Option<usize>) {
    let entity = find_window_entity(id, world);
    let mut query = world.query_filtered::<&LayoutStrip, With<ActiveWorkspaceMarker>>();
    let strip = query.single(world).expect("one active strip");
    (strip.pinned(), strip.index_of(entity).ok())
}

fn pin_inset(world: &mut World) -> Option<(bool, i32)> {
    let mut query = world.query_filtered::<&Display, With<ActiveDisplayMarker>>();
    query.single(world).expect("one active display").pin_inset()
}

fn command(operation: Operation) -> Event {
    Event::Command {
        command: Command::Window(operation),
    }
}

#[test]
fn pinned_window_holds_the_left_edge_while_focus_moves_through_the_strip() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        command(Operation::Focus(Direction::Nth(1))), // 1
        command(Operation::Pin(Direction::West)),     // 2
        command(Operation::Focus(Direction::East)),   // 3: pin -> first column
        command(Operation::Focus(Direction::East)),   // 4
        command(Operation::Focus(Direction::West)),   // 5
        command(Operation::Focus(Direction::West)),   // 6: first column -> pin
        command(Operation::Pin(Direction::West)),     // 7: unpin
    ];

    TestHarness::new()
        .with_windows(4)
        .on_iteration(2, |world, _state| {
            assert_eq!(
                frame(world, 1),
                IRect::new(0, 20, PIN_WIDTH, 20 + VIEWPORT_HEIGHT)
            );
            assert_eq!(pin_inset(world), Some((false, PIN_WIDTH)));
            // The strip starts where the pinned slice ends.
            assert_eq!(frame(world, 0).min.x, PIN_WIDTH);
        })
        .on_iteration(3, |world, _state| {
            assert_focused!(world, 0);
        })
        .on_iteration(4, |world, _state| {
            assert_focused!(world, 2);
            // Scrolling the strip leaves the pinned window where it is.
            assert_eq!(frame(world, 1).min.x, 0);
            assert_eq!(frame(world, 2).max.x, TEST_DISPLAY_WIDTH);
        })
        .on_iteration(6, |world, _state| {
            assert_focused!(world, 1);
            assert_eq!(frame(world, 1).min.x, 0);
        })
        .on_iteration(7, |world, _state| {
            assert_eq!(
                pin_and_column(world, 1),
                (None, Some(1)),
                "back in its column"
            );
            assert_eq!(pin_inset(world), None);
        })
        .run(commands);
}

#[test]
fn full_width_window_fits_beside_a_pin_and_regains_its_width() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        command(Operation::Focus(Direction::First)), // 1
        command(Operation::FullWidth),               // 2
        command(Operation::Focus(Direction::East)),  // 3
        command(Operation::Pin(Direction::East)),    // 4
        command(Operation::Pin(Direction::East)),    // 5: unpin
    ];

    TestHarness::new()
        .with_windows(2)
        .on_iteration(2, |world, _state| {
            assert_eq!(frame(world, 0).width(), TEST_DISPLAY_WIDTH);
        })
        .on_iteration(4, |world, _state| {
            assert_eq!(
                frame(world, 1),
                IRect::new(
                    TEST_DISPLAY_WIDTH - PIN_WIDTH,
                    20,
                    TEST_DISPLAY_WIDTH,
                    20 + VIEWPORT_HEIGHT
                )
            );
            assert_eq!(
                frame(world, 0),
                IRect::new(0, 20, PIN_WIDTH, 20 + VIEWPORT_HEIGHT)
            );
        })
        .on_iteration(5, |world, _state| {
            assert_eq!(frame(world, 0).width(), TEST_DISPLAY_WIDTH);
        })
        .run(commands);
}

#[test]
fn pin_belongs_to_its_virtual_workspace() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        command(Operation::Focus(Direction::First)), // 1
        command(Operation::Pin(Direction::West)),    // 2
        command(Operation::Focus(Direction::East)),  // 3: window 1
        command(Operation::VirtualMoveNumber(1, MoveFocus::Stay)), // 4
        command(Operation::VirtualNumber(1)),        // 5
        command(Operation::VirtualNumber(0)),        // 6
    ];

    TestHarness::new()
        .with_windows(3)
        .on_iteration(2, |world, _state| {
            assert_eq!(pin_inset(world), Some((false, PIN_WIDTH)));
        })
        .on_iteration(5, |world, _state| {
            // The other workspace has the whole display, and the pinned window
            // has gone with its own workspace.
            assert_eq!(pin_inset(world), None);
            assert_eq!(frame(world, 1).min.x, 0);
            let pinned = frame(world, 0);
            assert!(
                pinned.min.y >= TEST_DISPLAY_HEIGHT - 50,
                "pinned window should be parked with its strip, got {pinned:?}"
            );
        })
        .on_iteration(6, |world, _state| {
            assert_eq!(pin_inset(world), Some((false, PIN_WIDTH)));
            assert_eq!(
                frame(world, 0),
                IRect::new(0, 20, PIN_WIDTH, 20 + VIEWPORT_HEIGHT)
            );
            assert_eq!(frame(world, 2).min.x, PIN_WIDTH);
        })
        .run(commands);
}

#[test]
fn closing_the_pinned_window_gives_its_space_back() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        command(Operation::Focus(Direction::First)), // 1
        command(Operation::Pin(Direction::West)),    // 2
        Event::Command {
            command: Command::PrintState,
        }, // 3
        Event::Command {
            command: Command::PrintState,
        }, // 4
    ];

    TestHarness::new()
        .with_windows(3)
        .on_iteration(2, |_world, state| {
            state.os_close_window(0);
        })
        .on_iteration(4, |world, _state| {
            assert_eq!(pin_and_column(world, 1).0, None);
            assert_eq!(pin_inset(world), None);
            assert_eq!(
                frame(world, 1).min.x,
                0,
                "the strip uses the whole display again"
            );
        })
        .run(commands);
}

#[test]
fn stepping_off_a_right_pin_lands_on_the_last_column() {
    let commands = vec![
        Event::MenuOpened { window_id: 0 },
        command(Operation::Focus(Direction::First)), // 1
        command(Operation::Pin(Direction::East)),    // 2
        command(Operation::Focus(Direction::East)),  // 3: nothing past the edge
        command(Operation::Focus(Direction::West)),  // 4
    ];

    TestHarness::new()
        .with_windows(3)
        .on_iteration(3, |world, _state| {
            assert_focused!(world, 0);
        })
        .on_iteration(4, |world, _state| {
            assert_focused!(world, 2);
            // Scrolled into the viewport, which ends where the pin begins.
            assert_eq!(frame(world, 2).max.x, TEST_DISPLAY_WIDTH - PIN_WIDTH);
            assert_eq!(frame(world, 0).min.x, TEST_DISPLAY_WIDTH - PIN_WIDTH);
        })
        .run(commands);
}

/// Fails if any strip window other than `pinned` shows inside the pinned
/// slice: over the pinned window, or behind it. Parked windows don't count.
fn assert_nothing_enters_the_left_pin(world: &mut World, pinned: i32, when: &str) {
    let mut query = world.query::<&Window>();
    for window in query.iter(world) {
        let frame = window.frame();
        let parked = frame.min.y >= TEST_DISPLAY_HEIGHT - 50;
        assert!(
            window.id() == pinned || parked || frame.min.x >= PIN_WIDTH,
            "{when}: window {} at {frame:?} reaches into the pinned slice",
            window.id()
        );
    }
}

#[test]
fn scrolling_beside_a_left_pin_never_enters_the_pinned_slice() {
    let config = Config::try_from("[swipe.gesture]\nfingers_count = 3\n").unwrap();
    let mut harness = TestHarness::new().with_config(config).with_windows(5);
    harness.run(vec![
        Event::MenuOpened { window_id: 0 },
        command(Operation::Focus(Direction::First)),
        command(Operation::Pin(Direction::West)),
    ]);

    // Keyboard: walk the free pane through every column and back.
    let walk =
        std::iter::repeat_n(Direction::East, 4).chain(std::iter::repeat_n(Direction::West, 4));
    for (step, direction) in walk.enumerate() {
        harness
            .world()
            .write_message::<Event>(command(Operation::Focus(direction)));
        for frame in 0..15 {
            harness.advance(std::time::Duration::from_millis(20));
            assert_nothing_enters_the_left_pin(
                harness.world(),
                0,
                &format!("focus step {step}, frame {frame}"),
            );
        }
    }

    // Swiping: a brisk swipe in each direction, frame by frame.
    for delta in [0.04, -0.04] {
        for _ in 0..3 {
            harness.world().write_message::<Event>(Event::Swipe {
                delta,
                fingers: 3,
                distance_cm: 0.0,
            });
            harness.advance(std::time::Duration::from_millis(20));
        }
        for frame in 0..60 {
            harness.advance(std::time::Duration::from_millis(20));
            assert_nothing_enters_the_left_pin(
                harness.world(),
                0,
                &format!("swipe {delta}, frame {frame}"),
            );
        }
    }
    assert_eq!(frame(harness.world(), 0).min.x, 0, "the pin never moved");
}

#[test]
fn scrolling_beside_a_right_pin_never_enters_the_pinned_slice() {
    let mut harness = TestHarness::new().with_windows(5);
    harness.run(vec![
        Event::MenuOpened { window_id: 0 },
        command(Operation::Focus(Direction::Last)),
        command(Operation::Pin(Direction::East)),
    ]);
    let free_edge = TEST_DISPLAY_WIDTH - PIN_WIDTH;

    let walk =
        std::iter::repeat_n(Direction::West, 4).chain(std::iter::repeat_n(Direction::East, 4));
    for (step, direction) in walk.enumerate() {
        harness
            .world()
            .write_message::<Event>(command(Operation::Focus(direction)));
        for frame in 0..15 {
            harness.advance(std::time::Duration::from_millis(20));
            let mut query = harness.world().query::<&Window>();
            for window in query.iter(harness.world()) {
                let rect = window.frame();
                let parked = rect.min.y >= TEST_DISPLAY_HEIGHT - 50;
                assert!(
                    window.id() == 4 || parked || rect.max.x <= free_edge,
                    "focus step {step}, frame {frame}: window {} at {rect:?} reaches into the pinned slice",
                    window.id()
                );
            }
        }
    }
    assert_eq!(
        frame(harness.world(), 4).min.x,
        free_edge,
        "the pin never moved"
    );
}

/// A pinned window takes one of the workspace's columns: a third with three.
#[test]
fn a_pinned_window_takes_one_column() {
    let mut harness = TestHarness::new().with_windows(3);
    harness.advance(std::time::Duration::from_millis(100));
    let command = |harness: &mut TestHarness, operation| {
        harness
            .app
            .world_mut()
            .write_message::<Event>(Event::Command {
                command: Command::Window(operation),
            });
        harness.advance(std::time::Duration::from_millis(500));
    };
    let pinned_width = |harness: &mut TestHarness| {
        let world = harness.world();
        let mut strips = world.query::<&LayoutStrip>();
        let pin = strips
            .iter(world)
            .find_map(|strip| strip.pinned())
            .expect("a pinned window");
        world
            .get::<crate::manager::Window>(pin.entity)
            .expect("the pinned window")
            .frame()
            .width()
    };

    command(&mut harness, Operation::SetColumns(3));
    command(&mut harness, Operation::Pin(Direction::West));
    assert_eq!(pinned_width(&mut harness), TEST_DISPLAY_WIDTH / 3);

    command(&mut harness, Operation::SetColumns(2));
    assert_eq!(pinned_width(&mut harness), TEST_DISPLAY_WIDTH / 2);
}
