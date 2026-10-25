//! The workspace drop bar: while a window is dragged, a small panel at the
//! top of the screen shows the workspaces as numbered slots, one cluster per
//! display: a solid slot for each workspace with windows, and one hollow slot
//! for an empty workspace there, or a new one. Letting go of the window over
//! a slot moves it to that workspace.

use bevy::ecs::entity::Entity;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Commands, NonSendMut, Query, Res, ResMut};
use bevy::math::IRect;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSFont, NSFontWeightSemibold, NSImage,
    NSImageSymbolConfiguration, NSPanel, NSView,
};
use objc2_core_foundation::CGFloat;
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString,
};
use tracing::{debug, trace};

use crate::config::Config;
use crate::ecs::MouseHeldMarker;
use crate::ecs::numbering::CombinedWorkspaces;
use crate::ecs::params::{ActiveDisplay, Windows};
use crate::ecs::workspace::{DropOnWorkspaceTrigger, DropTarget};
use crate::events::{Event, InputEvent};
use crate::manager::{Origin, WindowManager, origin_from};
use crate::platform::WinID;

/// How far the pointer has to move before the window is checked for being
/// dragged with it, in points.
const DRAG_THRESHOLD: i32 = 6;
/// How far the window has to have moved by then to count as dragged.
const WINDOW_MOVED: i32 = 3;
/// Gap between the bar and the top of the usable screen area.
const TOP_MARGIN: i32 = 8;
const PANEL_PADDING: i32 = 5;
const PANEL_RADIUS: f64 = 12.0;
const SLOT_WIDTH: i32 = 30;
const SLOT_HEIGHT: i32 = 24;
const SLOT_GAP: i32 = 5;
const SLOT_RADIUS: f64 = 7.0;
/// Room at the left of a display's slots for its screen icon.
const ICON_WIDTH: i32 = 20;
const ICON_SIZE: f64 = 13.0;
/// Space between one display's slots and the next.
const GROUP_GAP: i32 = 12;
/// The pointer still hits a slot this far below the bar.
const HIT_BELOW: i32 = 16;
/// ... and this far above it, over the menu bar.
const HIT_ABOVE: i32 = 60;
const LABEL_FONT_SIZE: f64 = 11.0;

/// One slot on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DropSlot {
    pub target: DropTarget,
    /// The workspace's number, one-based; `None` for a new one.
    pub number: Option<u32>,
    /// The workspace has windows.
    pub occupied: bool,
    /// The workspace on screen, which the window is dragged from.
    pub active: bool,
}

/// One display's slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DropGroup {
    pub builtin: bool,
    pub slots: Vec<DropSlot>,
}

/// Where the bar and its slots sit, in screen coordinates (y down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DropBarGeometry {
    pub frame: IRect,
    /// Per group: its screen icon, if the displays are told apart, and its
    /// slots.
    pub groups: Vec<(Option<IRect>, Vec<IRect>)>,
}

/// Lays out groups of `counts` slots side by side, centred at the top of
/// `edge`. With more than one group, each gets its screen icon at the left.
pub(crate) fn drop_bar_geometry(counts: &[usize], edge: IRect) -> DropBarGeometry {
    let icons = counts.len() > 1;
    let group_width = |count: usize| {
        let count = i32::try_from(count).unwrap_or(i32::MAX / SLOT_WIDTH);
        let icon = if icons { ICON_WIDTH } else { 0 };
        icon + count * SLOT_WIDTH + (count - 1).max(0) * SLOT_GAP
    };
    let gaps = i32::try_from(counts.len().saturating_sub(1)).unwrap_or(0) * GROUP_GAP;
    let width = 2 * PANEL_PADDING + counts.iter().map(|&n| group_width(n)).sum::<i32>() + gaps;
    let height = 2 * PANEL_PADDING + SLOT_HEIGHT;
    let left = i32::midpoint(edge.min.x, edge.max.x) - width / 2;
    let top = edge.min.y + TOP_MARGIN;
    let slot_top = top + PANEL_PADDING;

    let mut x = left + PANEL_PADDING;
    let groups = counts
        .iter()
        .map(|&count| {
            let icon = icons.then(|| {
                let rect = IRect::new(x, slot_top, x + ICON_WIDTH, slot_top + SLOT_HEIGHT);
                x += ICON_WIDTH;
                rect
            });
            let slots = (0..count)
                .map(|_| {
                    let rect = IRect::new(x, slot_top, x + SLOT_WIDTH, slot_top + SLOT_HEIGHT);
                    x += SLOT_WIDTH + SLOT_GAP;
                    rect
                })
                .collect();
            x += GROUP_GAP - SLOT_GAP;
            (icon, slots)
        })
        .collect();
    DropBarGeometry {
        frame: IRect::new(left, top, left + width, top + height),
        groups,
    }
}

/// The slot, as `(group, slot)`, the pointer at `point` is over. A slot
/// reaches halfway into the gaps beside it, and up over the menu bar and a
/// little below the bar, so it is easy to hit.
pub(crate) fn slot_at(geometry: &DropBarGeometry, point: Origin) -> Option<(usize, usize)> {
    let frame = geometry.frame;
    if point.y < frame.min.y - HIT_ABOVE || point.y > frame.max.y + HIT_BELOW {
        return None;
    }
    geometry
        .groups
        .iter()
        .enumerate()
        .find_map(|(group, (_, slots))| {
            slots
                .iter()
                .position(|slot| {
                    point.x >= slot.min.x - SLOT_GAP / 2 && point.x < slot.max.x + SLOT_GAP / 2
                })
                .map(|slot| (group, slot))
        })
}

/// Groups the combined workspace list by display: each display's
/// workspaces with windows, then one empty slot, its first empty workspace
/// or else a new one.
fn drop_groups(combined: &CombinedWorkspaces) -> Vec<DropGroup> {
    // Per display: its group, and its first empty workspace.
    let mut groups: Vec<(Entity, DropGroup, Option<DropSlot>)> = Vec::new();
    for (number, workspace) in combined.list().into_iter().enumerate() {
        let Ok(number) = u32::try_from(number) else {
            continue;
        };
        if groups
            .last()
            .is_none_or(|(display, _, _)| *display != workspace.display)
        {
            let builtin = combined
                .display(workspace.display)
                .is_some_and(crate::manager::Display::builtin);
            groups.push((
                workspace.display,
                DropGroup {
                    builtin,
                    slots: Vec::new(),
                },
                None,
            ));
        }
        let Some((_, group, empty)) = groups.last_mut() else {
            continue;
        };
        let slot = DropSlot {
            target: DropTarget::Workspace(number),
            number: Some(number + 1),
            occupied: workspace.occupied,
            active: workspace.active,
        };
        if workspace.occupied {
            group.slots.push(slot);
        } else if empty.is_none() {
            *empty = Some(slot);
        }
    }
    groups
        .into_iter()
        .map(|(display, mut group, empty)| {
            group.slots.push(empty.unwrap_or(DropSlot {
                target: DropTarget::NewOn(display),
                number: None,
                occupied: false,
                active: false,
            }));
            group
        })
        .collect()
}

/// The window under the pointer since the button went down.
#[derive(Clone, Copy, Debug)]
struct HeldPane {
    window: Entity,
    window_id: WinID,
    /// Where the button went down.
    press: Origin,
    /// The window's frame then.
    frame: IRect,
}

/// The bar while a window is dragged.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DropBar {
    pub groups: Vec<DropGroup>,
    pub geometry: DropBarGeometry,
    /// The slot under the pointer, as `(group, slot)`.
    pub hovered: Option<(usize, usize)>,
}

impl DropBar {
    fn target(&self, (group, slot): (usize, usize)) -> Option<DropTarget> {
        Some(self.groups.get(group)?.slots.get(slot)?.target)
    }
}

/// The window being dragged, and the bar it can be dropped on.
#[derive(Debug, Default, Resource)]
pub struct PaneDrag {
    held: Option<HeldPane>,
    /// The held window is moving with the pointer.
    dragging: bool,
    bar: Option<DropBar>,
}

impl PaneDrag {
    /// Whether a window is being dragged, tiled or not.
    pub(crate) fn dragging(&self) -> bool {
        self.dragging
    }

    pub(crate) fn bar(&self) -> Option<&DropBar> {
        self.bar.as_ref()
    }
}

/// Follows a window from the button going down on it: once it is dragged,
/// rather than resized, the drop bar comes up; letting go over a slot moves
/// the window to that workspace instead of sliding it back into its column.
#[allow(clippy::too_many_arguments)]
pub(crate) fn track_pane_drag(
    mut messages: MessageReader<InputEvent>,
    windows: Windows,
    window_manager: Res<WindowManager>,
    active_display: ActiveDisplay,
    combined: CombinedWorkspaces,
    config: Res<Config>,
    mouse_held: Query<Entity, With<MouseHeldMarker>>,
    mut drag: ResMut<PaneDrag>,
    mut commands: Commands,
) {
    for InputEvent(event) in messages.read() {
        match event {
            Event::MouseDown { point, .. } => {
                let held = window_manager
                    .find_window_at_point(point)
                    .ok()
                    .and_then(|window_id| {
                        let (_, window) = windows.find(window_id)?;
                        let frame = window_manager.window_bounds(window_id).ok()?;
                        Some(HeldPane {
                            window,
                            window_id,
                            press: origin_from(*point),
                            frame,
                        })
                    });
                if held.is_some() || drag.held.is_some() || drag.dragging || drag.bar.is_some() {
                    *drag = PaneDrag {
                        held,
                        ..PaneDrag::default()
                    };
                }
            }
            Event::MouseDragged { point, .. } => {
                let Some(held) = drag.held else {
                    continue;
                };
                let point = origin_from(*point);
                if let Some(bar) = &drag.bar {
                    let hovered = slot_at(&bar.geometry, point);
                    if hovered != bar.hovered
                        && let Some(bar) = drag.bar.as_mut()
                    {
                        bar.hovered = hovered;
                    }
                    continue;
                }
                if (point - held.press).abs().max_element() < DRAG_THRESHOLD {
                    continue;
                }
                let Ok(frame) = window_manager.window_bounds(held.window_id) else {
                    continue;
                };
                // Moved with the pointer, and the same size: a resize moves
                // an edge, and selecting text moves nothing.
                let moved = (frame.min - held.frame.min).abs().max_element() >= WINDOW_MOVED;
                if !moved || frame.size() != held.frame.size() {
                    continue;
                }
                if !drag.dragging {
                    drag.dragging = true;
                }
                // Checked now, not on the press: a click on another display
                // brings it to the front only after.
                if !active_display.active_strip().contains(held.window) {
                    trace!("dragged window {} is not tiled in front", held.window);
                    continue;
                }
                let groups = drop_groups(&combined);
                let counts = groups.iter().map(|g| g.slots.len()).collect::<Vec<_>>();
                let edge = active_display
                    .display()
                    .edge_bounds(active_display.dock(), &config);
                let geometry = drop_bar_geometry(&counts, edge);
                debug!("window {} dragged: drop bar up", held.window);
                drag.bar = Some(DropBar {
                    hovered: slot_at(&geometry, point),
                    groups,
                    geometry,
                });
            }
            Event::MouseUp { point, .. } => {
                if drag.dragging {
                    drag.dragging = false;
                }
                let held = drag.held.take();
                let Some(bar) = drag.bar.take() else {
                    continue;
                };
                let Some(held) = held else {
                    continue;
                };
                let Some(target) =
                    slot_at(&bar.geometry, origin_from(*point)).and_then(|slot| bar.target(slot))
                else {
                    continue;
                };
                // The window is going elsewhere: no sliding it back into view
                // here on mouse-up.
                for marker in &mouse_held {
                    if let Ok(mut entity_commands) = commands.get_entity(marker) {
                        entity_commands.try_despawn();
                    }
                }
                commands.trigger(DropOnWorkspaceTrigger {
                    window: held.window,
                    target,
                });
            }
            _ => (),
        }
    }
}

/// Shows the bar while a window is dragged, and takes it down after.
pub(crate) fn update_drop_bar(drag: Res<PaneDrag>, manager: Option<NonSendMut<DropBarManager>>) {
    let Some(mut manager) = manager else {
        return;
    };
    match drag.bar() {
        Some(bar) => manager.show(bar),
        None => manager.hide(),
    }
}

// ── AppKit ──────────────────────────────────────────────────────────────

struct SlotTile {
    rect: NSRect,
    label: Option<Retained<NSAttributedString>>,
    occupied: bool,
    active: bool,
    hovered: bool,
}

struct ScreenIcon {
    rect: NSRect,
    builtin: bool,
}

struct DropBarViewIvars {
    slots: Vec<SlotTile>,
    icons: Vec<ScreenIcon>,
    /// Between one display's slots and the next.
    dividers: Vec<f64>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "FihDropBarView"]
    #[ivars = DropBarViewIvars]
    struct DropBarView;

    impl DropBarView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let ivars = self.ivars();
            let bounds = self.bounds();

            // The panel, as the minimap's.
            NSColor::colorWithSRGBRed_green_blue_alpha(0.11, 0.11, 0.12, 0.9).setFill();
            let panel = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                bounds,
                PANEL_RADIUS,
                PANEL_RADIUS,
            );
            panel.fill();
            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.12).setStroke();
            panel.setLineWidth(1.0);
            panel.stroke();

            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.12).setFill();
            for &x in &ivars.dividers {
                NSBezierPath::fillRect(NSRect::new(
                    NSPoint::new(x - 0.5, f64::from(PANEL_PADDING) + 3.0),
                    NSSize::new(1.0, f64::from(SLOT_HEIGHT) - 6.0),
                ));
            }
            for icon in &ivars.icons {
                draw_screen_icon(icon);
            }

            let (focus_red, focus_green, focus_blue) = crate::minimap::FOCUS_RGB;
            let teal = |alpha| {
                NSColor::colorWithSRGBRed_green_blue_alpha(focus_red, focus_green, focus_blue, alpha)
            };
            for slot in &ivars.slots {
                let (fill, border, line_width) = if slot.hovered {
                    (teal(0.9), teal(1.0), 2.0)
                } else if slot.occupied {
                    (
                        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.85),
                        if slot.active {
                            teal(1.0)
                        } else {
                            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.85)
                        },
                        if slot.active { 2.0 } else { 1.0 },
                    )
                } else {
                    (
                        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.0),
                        if slot.active {
                            teal(1.0)
                        } else {
                            NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.3)
                        },
                        if slot.active { 2.0 } else { 1.0 },
                    )
                };
                // Inset by half the line so the border stays inside the slot.
                let inset = line_width / 2.0;
                let rect = NSRect::new(
                    NSPoint::new(slot.rect.origin.x + inset, slot.rect.origin.y + inset),
                    NSSize::new(
                        slot.rect.size.width - line_width,
                        slot.rect.size.height - line_width,
                    ),
                );
                let path =
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, SLOT_RADIUS, SLOT_RADIUS);
                fill.setFill();
                path.fill();
                border.setStroke();
                path.setLineWidth(line_width);
                path.stroke();

                let Some(label) = &slot.label else {
                    continue;
                };
                let text_size: NSSize = unsafe { msg_send![label, size] };
                let origin = NSPoint::new(
                    slot.rect.origin.x + (slot.rect.size.width - text_size.width) / 2.0,
                    slot.rect.origin.y + (slot.rect.size.height - text_size.height) / 2.0,
                );
                let _: () = unsafe { msg_send![label, drawAtPoint: origin] };
            }
        }
    }
);

impl DropBarView {
    fn new(mtm: MainThreadMarker, frame: NSRect, ivars: DropBarViewIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

/// A slot's workspace number: dark on a solid slot, light on a hollow one.
fn slot_label(number: u32, dark: bool) -> Retained<NSAttributedString> {
    let font = NSFont::monospacedDigitSystemFontOfSize_weight(LABEL_FONT_SIZE, unsafe {
        NSFontWeightSemibold
    });
    let color = if dark {
        NSColor::colorWithSRGBRed_green_blue_alpha(0.11, 0.11, 0.12, 0.9)
    } else {
        NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.6)
    };
    let keys = [
        &*NSString::from_str("NSFont"),
        &*NSString::from_str("NSColor"),
    ];
    let objects = [&*font as &AnyObject, &*color as &AnyObject];
    let attributes = NSDictionary::from_slices(&keys, &objects);
    unsafe {
        msg_send![
            NSAttributedString::alloc(),
            initWithString: &*NSString::from_str(&number.to_string()),
            attributes: &*attributes
        ]
    }
}

/// A laptop for the Mac's own screen, a monitor for an external one.
fn draw_screen_icon(icon: &ScreenIcon) {
    let name = if icon.builtin {
        "laptopcomputer"
    } else {
        "display"
    };
    let Some(symbol) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    ) else {
        return;
    };
    let grey = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.55);
    let colored = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[grey]),
    );
    let Some(image) = symbol.imageWithSymbolConfiguration(&colored) else {
        return;
    };
    let natural = image.size();
    if natural.width <= 0.0 || natural.height <= 0.0 {
        return;
    }
    let scale = (ICON_SIZE / natural.width).min(ICON_SIZE / natural.height);
    let size = NSSize::new(natural.width * scale, natural.height * scale);
    let rect = NSRect::new(
        NSPoint::new(
            icon.rect.origin.x + (icon.rect.size.width - size.width) / 2.0 - 1.0,
            icon.rect.origin.y + (icon.rect.size.height - size.height) / 2.0,
        ),
        size,
    );
    image.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
}

/// Owns the drop bar's panel.
pub struct DropBarManager {
    mtm: MainThreadMarker,
    panel: Option<Retained<NSPanel>>,
    /// What is drawn, so an unchanged bar is not redrawn.
    shown: Option<DropBar>,
}

impl DropBarManager {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            panel: None,
            shown: None,
        }
    }

    fn show(&mut self, bar: &DropBar) {
        if self.shown.as_ref() == Some(bar) {
            return;
        }
        let frame = bar.geometry.frame;
        // From screen coordinates (y down) to the panel's (y up).
        let local = |rect: IRect| {
            NSRect::new(
                NSPoint::new(
                    f64::from(rect.min.x - frame.min.x),
                    f64::from(frame.max.y - rect.max.y),
                ),
                NSSize::new(f64::from(rect.width()), f64::from(rect.height())),
            )
        };
        let mut slots = Vec::new();
        let mut icons = Vec::new();
        let mut dividers = Vec::new();
        for (index, (group, (icon, rects))) in
            bar.groups.iter().zip(&bar.geometry.groups).enumerate()
        {
            if index > 0
                && let Some(start) = icon.or_else(|| rects.first().copied())
            {
                dividers.push(f64::from(start.min.x - frame.min.x - GROUP_GAP / 2));
            }
            if let Some(icon) = icon {
                icons.push(ScreenIcon {
                    rect: local(*icon),
                    builtin: group.builtin,
                });
            }
            for (slot_index, (slot, rect)) in group.slots.iter().zip(rects).enumerate() {
                let hovered = bar.hovered == Some((index, slot_index));
                slots.push(SlotTile {
                    rect: local(*rect),
                    label: slot
                        .number
                        .map(|number| slot_label(number, slot.occupied || hovered)),
                    occupied: slot.occupied,
                    active: slot.active,
                    hovered,
                });
            }
        }

        let size = NSSize::new(f64::from(frame.width()), f64::from(frame.height()));
        let screen_height = crate::overlay::primary_screen_height(self.mtm);
        let panel_frame = NSRect::new(
            NSPoint::new(
                f64::from(frame.min.x),
                screen_height - f64::from(frame.max.y),
            ),
            size,
        );
        let view = DropBarView::new(
            self.mtm,
            NSRect::new(NSPoint::new(0.0, 0.0), size),
            DropBarViewIvars {
                slots,
                icons,
                dividers,
            },
        );
        let panel = self
            .panel
            .get_or_insert_with(|| crate::minimap::make_panel(self.mtm, panel_frame));
        // The pointer is busy dragging a window; the bar only shows where it
        // would land.
        panel.setIgnoresMouseEvents(true);
        panel.setAlphaValue(CGFloat::from(1.0));
        panel.setContentView(Some(&view));
        panel.setFrame_display(panel_frame, true);
        panel.orderFront(None::<&AnyObject>);
        self.shown = Some(bar.clone());
    }

    fn hide(&mut self) {
        if self.shown.take().is_some()
            && let Some(panel) = &self.panel
        {
            panel.orderOut(None::<&AnyObject>);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::IRect;

    use super::{
        GROUP_GAP, ICON_WIDTH, PANEL_PADDING, SLOT_GAP, SLOT_HEIGHT, SLOT_WIDTH, TOP_MARGIN,
        drop_bar_geometry, slot_at,
    };
    use crate::manager::Origin;

    const EDGE: IRect = IRect {
        min: Origin::new(0, 25),
        max: Origin::new(1000, 800),
    };

    #[test]
    fn one_display_is_centred_at_the_top_without_an_icon() {
        let geometry = drop_bar_geometry(&[3], EDGE);
        let width = 2 * PANEL_PADDING + 3 * SLOT_WIDTH + 2 * SLOT_GAP;
        assert_eq!(geometry.frame.width(), width);
        assert_eq!(geometry.frame.min.x, 500 - width / 2);
        assert_eq!(geometry.frame.min.y, 25 + TOP_MARGIN);
        assert_eq!(geometry.frame.height(), 2 * PANEL_PADDING + SLOT_HEIGHT);
        let (icon, slots) = &geometry.groups[0];
        assert!(icon.is_none());
        assert_eq!(slots.len(), 3);
        assert_eq!(slots[1].min.x - slots[0].min.x, SLOT_WIDTH + SLOT_GAP);
    }

    #[test]
    fn displays_get_an_icon_each_and_a_gap_between() {
        let geometry = drop_bar_geometry(&[3, 4], EDGE);
        let (first_icon, first) = &geometry.groups[0];
        let (second_icon, second) = &geometry.groups[1];
        let first_icon = first_icon.expect("an icon per display");
        let second_icon = second_icon.expect("an icon per display");
        assert_eq!(first[0].min.x, first_icon.max.x);
        assert_eq!(second_icon.min.x - first[2].max.x, GROUP_GAP);
        assert_eq!(second[0].min.x, second_icon.min.x + ICON_WIDTH);
        assert_eq!(second.len(), 4);
        assert_eq!(geometry.frame.max.x, second[3].max.x + PANEL_PADDING);
    }

    #[test]
    fn the_pointer_hits_a_slot_over_the_bar_and_the_menu_bar() {
        let geometry = drop_bar_geometry(&[3, 3], EDGE);
        let slot = geometry.groups[1].1[2];
        let middle = slot.center();
        assert_eq!(slot_at(&geometry, middle), Some((1, 2)));
        assert_eq!(
            slot_at(&geometry, Origin::new(middle.x, 0)),
            Some((1, 2)),
            "up in the menu bar"
        );
        assert_eq!(
            slot_at(&geometry, Origin::new(slot.min.x - 1, middle.y)),
            Some((1, 2)),
            "just into the gap"
        );
        assert_eq!(slot_at(&geometry, Origin::new(middle.x, 300)), None);
        assert_eq!(slot_at(&geometry, Origin::new(5, middle.y)), None);
    }
}
