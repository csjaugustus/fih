use bevy::ecs::change_detection::{DetectChanges, DetectChangesMut};
use bevy::ecs::lifecycle::RemovedComponents;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::{Added, Changed};
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{NonSendMut, Query, Res, ResMut};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSBezierPath, NSBitmapImageRep, NSCellImagePosition, NSColor, NSCompositingOperation,
    NSControlStateValueOff, NSControlStateValueOn, NSDeviceRGBColorSpace, NSFont,
    NSFontWeightSemibold, NSGradient, NSGraphicsContext, NSImage, NSImageScaling, NSImageView,
    NSLayoutAttribute, NSMenu, NSMenuItem, NSScreen, NSStackView, NSStatusBar, NSStatusItem,
    NSTextField, NSUserInterfaceLayoutOrientation, NSVariableStatusItemLength, NSView,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::{
    NSArray, NSAttributedString, NSData, NSDictionary, NSInteger, NSObject, NSString,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::accessibility_prompt::{
    AccessibilitySetupAction, handle_accessibility_setup_action, permission_pane_name,
    show_accessibility_setup,
};
use crate::commands::{Command, Operation};
use crate::config::Config;
use crate::config::decorations::{
    DescriptorStyle, IndicatorFormat, IndicatorStyle, MenubarOrientation,
};
use crate::ecs::ActiveWorkspaceMarker;
use crate::ecs::layout::LayoutStrip;
use crate::ecs::params::{ActiveDisplay, Windows};
use crate::events::{Event, EventSender};
use crate::manager::Application;
use crate::util::round_px;

/// A menu or command choice overrides the configuration until changed again.
#[derive(Debug, Default, Resource, Serialize, Deserialize)]
pub struct MenuBarVisibility {
    enabled: Option<bool>,
    #[serde(skip)]
    dirty: bool,
}

impl MenuBarVisibility {
    pub fn enabled(&self, config: &Config) -> bool {
        self.enabled.unwrap_or_else(|| config.menubar_enabled())
    }

    fn load(path: &Path) -> Self {
        let data = match std::fs::read_to_string(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                warn!(%error, path = %path.display(), "unable to read menu bar preference");
                return Self::default();
            }
        };
        serde_json::from_str(&data).unwrap_or_else(|error| {
            warn!(%error, path = %path.display(), "ignoring invalid menu bar preference");
            Self::default()
        })
    }

    fn save(&self, path: &Path) -> crate::errors::Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, data)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

#[derive(Resource)]
pub struct MenuBarPreferenceFile(PathBuf);

pub fn load_menu_bar_preference(app: &mut bevy::app::App) {
    if let Some(path) = xdg::BaseDirectories::with_prefix("fih").get_state_file("menu-bar.json") {
        app.insert_resource(MenuBarVisibility::load(&path))
            .insert_resource(MenuBarPreferenceFile(path));
    }
}

pub fn setup_menu_bar(
    app: &mut bevy::app::App,
    mtm: MainThreadMarker,
    events: EventSender,
    initial_config: &Config,
) {
    // Prefer Lua setup when present; otherwise reuse the configuration already
    // loaded for the event tap rather than reading it again.
    let config = app
        .world()
        .get_resource::<Config>()
        .unwrap_or(initial_config);
    let visible = app.world().resource::<MenuBarVisibility>().enabled(config);
    app.insert_non_send(MenuBarManager::new(mtm, events, visible));
}

pub fn menu_bar_command_handler(
    mut messages: MessageReader<Event>,
    mut visibility: ResMut<MenuBarVisibility>,
) {
    for event in messages.read() {
        if let Event::Command {
            command: Command::MenuBar(enabled),
        } = event
            && visibility.enabled != Some(*enabled)
        {
            visibility.enabled = Some(*enabled);
            visibility.dirty = true;
        }
    }
}

pub fn save_menu_bar_preference(
    mut visibility: ResMut<MenuBarVisibility>,
    file: Option<Res<MenuBarPreferenceFile>>,
) {
    if !visibility.dirty {
        return;
    }
    let Some(file) = file else {
        return;
    };
    match visibility.save(&file.0) {
        Ok(()) => visibility.bypass_change_detection().dirty = false,
        Err(error) => {
            warn!(%error, path = %file.0.display(), "unable to save menu bar preference");
        }
    }
}

#[derive(Debug, Clone)]
struct MenuActionTargetIvars {
    events: EventSender,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaneruMenuActionTarget"]
    #[ivars = MenuActionTargetIvars]
    #[derive(Debug)]
    struct MenuActionTarget;

    impl MenuActionTarget {
        #[unsafe(method(switchWorkspace:))]
        fn switch_workspace(&self, item: &NSMenuItem) {
            let Ok(virtual_index) = u32::try_from(item.tag()) else {
                return;
            };
            self.send_command(Command::Window(Operation::VirtualNumber(virtual_index)));
        }

        #[unsafe(method(openConfig:))]
        fn open_config(&self, _: &NSMenuItem) {
            let Some(path) = config_file() else {
                warn!("no configuration file to open");
                return;
            };
            // `-t` opens it in the default text editor: `.toml` and `.lua`
            // often have no app of their own.
            if let Err(error) = std::process::Command::new("/usr/bin/open")
                .arg("-t")
                .arg(&path)
                .spawn()
            {
                warn!(%error, path = %path.display(), "unable to open the configuration file");
            }
        }

        #[unsafe(method(openAccessibilitySettings:))]
        fn open_accessibility_settings(&self, _: &NSMenuItem) {
            if let Err(error) = std::process::Command::new("/usr/bin/open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
                .spawn()
            {
                let pane = permission_pane_name();
                warn!(%error, pane, "unable to open permission settings");
            }
        }

        #[unsafe(method(showAccessibilityInstructions:))]
        fn show_accessibility_instructions(&self, _: &NSMenuItem) {
            let Some(main_thread_marker) = MainThreadMarker::new() else {
                warn!("unable to show setup instructions outside the main thread");
                return;
            };

            let action = show_accessibility_setup(main_thread_marker);
            handle_accessibility_setup_action(action);
        }

        #[unsafe(method(resetAccessibilityPermission:))]
        fn reset_accessibility_permission(&self, _: &NSMenuItem) {
            handle_accessibility_setup_action(AccessibilitySetupAction::ResetAndRetry);
        }

        #[unsafe(method(quitPaneru:))]
        fn quit_paneru(&self, _: &NSMenuItem) {
            self.send_command(Command::Quit);
        }

    }
);

/// The file fih takes its configuration from: the TOML file, or the Lua
/// script when one is in charge.
fn config_file() -> Option<PathBuf> {
    let toml = crate::config::CONFIGURATION_FILE.clone();
    #[cfg(feature = "lua")]
    let toml = toml.or_else(crate::config::discover_lua_file);
    toml
}

impl MenuActionTarget {
    fn new(mtm: MainThreadMarker, events: EventSender) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuActionTargetIvars { events });
        unsafe { msg_send![super(this), init] }
    }

    fn send_command(&self, command: Command) {
        if let Err(error) = self.ivars().events.send(Event::Command { command }) {
            warn!(%error, "unable to send menu bar command");
        }
    }
}

const MENU_BAR_SPACING: CGFloat = 5.0;
const BADGE_SPACING: CGFloat = 4.0;
const BADGE_RADIUS: CGFloat = 4.0;
const BADGE_PADDING: CGFloat = 4.0;
const BADGE_LINE_WIDTH: CGFloat = 1.2;

struct WorkspaceBadgeIvars {
    label: Retained<NSAttributedString>,
    active: bool,
    size: CGSize,
}

define_class!(
    /// A workspace number in a rounded square: filled with the number cut out
    /// for the active workspace, outlined for the rest. Only its alpha counts,
    /// since the menu bar image it is rendered into is a template.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "FihWorkspaceBadge"]
    #[ivars = WorkspaceBadgeIvars]
    struct WorkspaceBadge;

    impl WorkspaceBadge {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_content_size(&self) -> CGSize {
            self.ivars().size
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: CGRect) {
            let ivars = self.ivars();
            let inset = BADGE_LINE_WIDTH / 2.0;
            let bounds = self.bounds();
            let square = CGRect::new(
                CGPoint::new(bounds.origin.x + inset, bounds.origin.y + inset),
                CGSize::new(bounds.size.width - 2.0 * inset, bounds.size.height - 2.0 * inset),
            );
            let path =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(square, BADGE_RADIUS, BADGE_RADIUS);
            NSColor::blackColor().set();
            let context = NSGraphicsContext::currentContext();
            if ivars.active {
                path.fill();
                // Cut the number out of the fill.
                if let Some(context) = &context {
                    context.setCompositingOperation(NSCompositingOperation::DestinationOut);
                }
            } else {
                path.setLineWidth(BADGE_LINE_WIDTH);
                path.stroke();
            }

            let text_size: CGSize = unsafe { msg_send![&ivars.label, size] };
            let origin = CGPoint::new(
                bounds.origin.x + (bounds.size.width - text_size.width) / 2.0,
                bounds.origin.y + (bounds.size.height - text_size.height) / 2.0,
            );
            let _: () = unsafe { msg_send![&ivars.label, drawAtPoint: origin] };
            if let Some(context) = &context {
                context.setCompositingOperation(NSCompositingOperation::SourceOver);
            }
        }
    }
);

impl WorkspaceBadge {
    fn new(mtm: MainThreadMarker, label: &str, active: bool, font_size: f64) -> Retained<Self> {
        // A little under the menu bar's text size, so the square around it
        // still fits the bar.
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(font_size - 2.0, unsafe {
            NSFontWeightSemibold
        });
        let keys = [
            &*NSString::from_str("NSFont"),
            &*NSString::from_str("NSColor"),
        ];
        let color = NSColor::blackColor();
        let objects = [&*font as &AnyObject, &*color as &AnyObject];
        let attributes = NSDictionary::from_slices(&keys, &objects);
        let label: Retained<NSAttributedString> = unsafe {
            msg_send![
                NSAttributedString::alloc(),
                initWithString: &*NSString::from_str(label),
                attributes: &*attributes
            ]
        };
        let text_size: CGSize = unsafe { msg_send![&label, size] };
        let height = font_size + 3.0;
        let size = CGSize::new((text_size.width + 2.0 * BADGE_PADDING).max(height), height);

        let this = Self::alloc(mtm).set_ivars(WorkspaceBadgeIvars {
            label,
            active,
            size,
        });
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: CGRect::new(CGPoint::ZERO, size)] };
        this
    }
}
/// The `symbol` that selects the built-in fih mark; any other value names an
/// SF Symbol.
pub(crate) const FIH_MARK_SYMBOL: &str = "fih";
/// The fih mark, black on transparent: drawn as a template image, so only its
/// alpha matters and `AppKit` tints it for the menu bar.
const FIH_MARK: &[u8] = include_bytes!("../assets/menubar-icon.png");

pub struct MenuBarManager {
    mtm: MainThreadMarker,
    status_bar: Retained<NSStatusBar>,
    status_item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    action_target: Retained<MenuActionTarget>,
    /// The workspaces the menu was last built for, with the active one.
    menu_workspaces: Option<(u32, Vec<WorkspaceSummary>)>,
    current_content: Option<MenuBarContent>,
    visible: bool,
}

/// One virtual workspace of the current macOS Space, and the apps that have a
/// window in it, in strip order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceSummary {
    pub virtual_index: u32,
    pub apps: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum MenuBarContent {
    Text(String),
    Workspaces { current: Option<u32>, all: Vec<u32> },
}

impl MenuBarManager {
    fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.status_item.setVisible(visible);
            self.visible = visible;
        }
    }

    pub fn new(mtm: MainThreadMarker, events: EventSender, visible: bool) -> Self {
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        let action_target = MenuActionTarget::new(mtm, events);

        menu.setAutoenablesItems(false);
        status_item.setMenu(Some(&menu));
        // AppKit creates a visible item, so the default needs no transaction.
        if !visible {
            status_item.setVisible(false);
        }

        Self {
            mtm,
            status_bar,
            status_item,
            menu,
            action_target,
            menu_workspaces: None,
            current_content: None,
            visible,
        }
    }

    pub fn new_accessibility_required(mtm: MainThreadMarker, events: EventSender) -> Self {
        let mut manager = Self::new(mtm, events, true);
        manager.rebuild_accessibility_menu();
        manager.show_text("!");
        manager
    }

    fn rebuild_accessibility_menu(&mut self) {
        let pane = permission_pane_name();
        self.menu.removeAllItems();

        let status = self.add_item(&format!("fih — {pane} Required"), None);
        status.setEnabled(false);

        let hint = self.add_item("Grant access; fih will start automatically", None);
        hint.setEnabled(false);

        self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        self.add_item(
            "Show Setup Instructions…",
            Some(sel!(showAccessibilityInstructions:)),
        );
        self.add_item(
            &format!("Open {pane} Settings…"),
            Some(sel!(openAccessibilitySettings:)),
        );
        self.add_item(
            "Reset Permission & Retry…",
            Some(sel!(resetAccessibilityPermission:)),
        );

        self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        self.add_item("Quit fih", Some(sel!(quitPaneru:)));
    }

    pub fn update(&mut self, virtual_index: u32, workspaces: &[WorkspaceSummary], config: &Config) {
        if self
            .menu_workspaces
            .as_ref()
            .is_none_or(|(index, shown)| *index != virtual_index || shown != workspaces)
        {
            self.rebuild_menu(virtual_index, workspaces);
        }

        let current = config.workspace_menu_status().then_some(virtual_index);
        let all = match current {
            None => Vec::new(),
            Some(current) if config.menubar_indicator_style() == IndicatorStyle::Occupied => {
                occupied_indices(current, workspaces)
            }
            Some(_) => workspaces
                .iter()
                .map(|workspace| workspace.virtual_index)
                .collect(),
        };
        self.show_workspaces(config, current, &all);
    }

    fn rebuild_menu(&mut self, virtual_index: u32, workspaces: &[WorkspaceSummary]) {
        self.menu.removeAllItems();

        let status = self.add_item("fih — Running", None);
        status.setEnabled(false);
        self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));

        // What is open where; picking a workspace switches to it.
        let open = workspaces
            .iter()
            .filter(|workspace| !workspace.apps.is_empty())
            .collect::<Vec<_>>();
        for workspace in &open {
            let title = format!(
                "{}  {}",
                virtual_workspace_label(workspace.virtual_index),
                apps_label(&workspace.apps)
            );
            let item = self.add_item(&title, Some(sel!(switchWorkspace:)));
            item.setTag(isize::try_from(workspace.virtual_index).unwrap_or(isize::MAX));
            item.setState(if workspace.virtual_index == virtual_index {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        if !open.is_empty() {
            self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        }

        let open_config = self.add_item("Open Config File…", Some(sel!(openConfig:)));
        if let Some(path) = config_file() {
            open_config.setToolTip(Some(&NSString::from_str(&path.display().to_string())));
        }

        self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        self.add_item("Quit fih", Some(sel!(quitPaneru:)));
        self.menu_workspaces = Some((virtual_index, workspaces.to_vec()));
    }

    fn add_item(&self, title: &str, action: Option<objc2::runtime::Sel>) -> Retained<NSMenuItem> {
        let item = unsafe {
            self.menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(title),
                action,
                &NSString::from_str(""),
            )
        };
        if action.is_some() {
            unsafe { item.setTarget(Some(&self.action_target)) };
        }
        item
    }

    fn show_text(&mut self, label: &str) {
        let content = MenuBarContent::Text(label.to_owned());
        if self.current_content.as_ref() == Some(&content) {
            return;
        }

        let field = NSTextField::labelWithString(&NSString::from_str(label), self.mtm);
        field.setFont(Some(&NSFont::menuBarFontOfSize(0.0)));
        field.sizeToFit();
        let size = field.frame().size;

        if self.install(self.content_image(&field, size).as_deref()) {
            self.current_content = Some(content);
        }
    }

    fn show_workspaces(&mut self, config: &Config, current: Option<u32>, all: &[u32]) {
        let content = MenuBarContent::Workspaces {
            current,
            all: all.to_vec(),
        };
        if self.current_content.as_ref() == Some(&content) {
            return;
        }

        let descriptor = self.build_descriptor(config);
        let indicator = self.build_indicator(config, current, all);
        let ordered = match config.menubar_orientation() {
            MenubarOrientation::Default => [descriptor, indicator],
            MenubarOrientation::Flipped => [indicator, descriptor],
        };

        let stack = NSStackView::new(self.mtm);
        stack.setSpacing(MENU_BAR_SPACING);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        for view in ordered.into_iter().flatten() {
            stack.addArrangedSubview(&view);
        }

        let fitting = stack.fittingSize();
        let size = CGSize::new(
            fitting.width,
            fitting.height.min(self.status_bar.thickness()),
        );

        let image = self.content_image(&stack, size).map(|image| {
            self.colorized(config, &image, size)
                .unwrap_or_else(|| image.clone())
        });
        if self.install(image.as_deref()) {
            self.current_content = Some(content);
        }
    }

    /// Bakes `content` into the image the status item button draws.
    ///
    /// The button gets an image rather than `content` itself as a subview:
    /// `AppKit` snapshots a status item's button into its menu bar replicants,
    /// and a live subview - auto layout, or a hosted layer - re-dirties itself
    /// on every snapshot, so the two feed each other in a loop that pins the
    /// main thread at ~75% of a core and starves the run loop the event tap is
    /// serviced on: the tap times out, macOS switches it off and every
    /// keybinding falls through to whatever app is focused. A flat image is
    /// snapshotted once and never invalidates itself.
    ///
    /// The image is a template, so `AppKit` tints it for the current menu bar
    /// appearance - light or dark, and inverted while the menu is open - which
    /// only the alpha channel of the render takes part in. [`colorized`]
    /// replaces that with the configured colours where there are any.
    ///
    /// [`colorized`]: Self::colorized
    fn content_image(&self, content: &NSView, size: CGSize) -> Option<Retained<NSImage>> {
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }
        let bounds = CGRect::new(CGPoint::ZERO, size);
        let rep = render_content_bitmap(content, bounds, self.backing_scale_factor())?;

        let image = NSImage::initWithSize(NSImage::alloc(), size);
        image.addRepresentation(&rep);
        image.setTemplate(true);
        Some(image)
    }

    /// Fills the glyphs of `content` with the configured colours, so they
    /// carry the gradient themselves rather than the box around them.
    ///
    /// Returns `None` when no colours are configured, or when the render fails
    /// at any step: the template image the caller already has follows the menu
    /// bar's own appearance, which a baked colour would override in one of the
    /// two themes anyway.
    fn colorized(
        &self,
        config: &Config,
        content: &NSImage,
        size: CGSize,
    ) -> Option<Retained<NSImage>> {
        let colors = config
            .menubar_gradient()
            .into_iter()
            .map(|(red, green, blue)| {
                NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, 1.0)
            })
            .collect::<Vec<_>>();
        if colors.is_empty() {
            return None;
        }

        let bounds = CGRect::new(CGPoint::ZERO, size);
        let rep = bitmap_rep(bounds, self.backing_scale_factor())?;
        let canvas = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;

        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&canvas));
        let color_refs = colors.iter().map(|color| &**color).collect::<Vec<_>>();
        // A single colour is a gradient with nowhere to go, and `NSGradient`
        // refuses to be built from one.
        if let Some(gradient) =
            NSGradient::initWithColors(NSGradient::alloc(), &NSArray::from_slice(&color_refs))
        {
            gradient.drawInRect_angle(bounds, config.menubar_gradient_angle());
        } else {
            colors[0].setFill();
            NSBezierPath::fillRect(bounds);
        }
        // Keeps the fill only where the content drew, punching the glyphs out
        // of the colour rather than painting a coloured box behind them.
        content.drawInRect_fromRect_operation_fraction(
            bounds,
            CGRect::ZERO,
            NSCompositingOperation::DestinationIn,
            1.0,
        );
        NSGraphicsContext::restoreGraphicsState_class();

        let image = NSImage::initWithSize(NSImage::alloc(), size);
        image.addRepresentation(&rep);
        Some(image)
    }

    /// The scale the mask bitmap has to be rendered at. It cannot be read back
    /// from the content view, which is still outside any window while it is
    /// being rendered.
    fn backing_scale_factor(&self) -> CGFloat {
        self.status_item
            .button(self.mtm)
            .and_then(|button| button.window())
            .map(|window| window.backingScaleFactor())
            .or_else(|| NSScreen::mainScreen(self.mtm).map(|screen| screen.backingScaleFactor()))
            .filter(|scale| *scale > 0.0)
            .unwrap_or(1.0)
    }

    /// Draws `image` in the status item, or empties the item when there is
    /// nothing to draw.
    ///
    /// The item keeps [`NSVariableStatusItemLength`]: `AppKit` sizes it around
    /// the image, padding included, which is what the item's own content
    /// insets are for - measuring the content and setting the length by hand
    /// left it off-centre by exactly one padding.
    fn install(&self, image: Option<&NSImage>) -> bool {
        let Some(button) = self.status_item.button(self.mtm) else {
            warn!("unable to update menu bar: status item has no button");
            return false;
        };

        let Some(image) = image else {
            button.setImage(None);
            self.status_item.setLength(0.0);
            return true;
        };

        button.setImage(Some(image));
        // A button starts out at `NSNoImage`, which draws its (empty) title and
        // nothing else: the item goes blank without this.
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        button.setToolTip(Some(&NSString::from_str("fih window manager")));
        self.status_item.setLength(NSVariableStatusItemLength);
        true
    }

    fn build_indicator(
        &self,
        config: &Config,
        current: Option<u32>,
        all: &[u32],
    ) -> Option<Retained<NSStackView>> {
        let current = current?;
        let style = config.menubar_indicator_style();
        // Both formats that distinguish the active workspace from the rest say
        // nothing on their own, so a single-item indicator - `mono`, `paged` -
        // falls back to numbers, as does `occupied`, whose badges already mark
        // the active workspace.
        let format = match (style, config.menubar_indicator_format()) {
            (
                IndicatorStyle::Mono | IndicatorStyle::Paged | IndicatorStyle::Occupied,
                IndicatorFormat::Unicode | IndicatorFormat::Marked,
            ) => IndicatorFormat::Default,
            (_, format) => format,
        };

        let stack = NSStackView::new(self.mtm);
        stack.setSpacing(MENU_BAR_SPACING);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setAlignment(NSLayoutAttribute::CenterY);
        match style {
            IndicatorStyle::Mono => {
                stack.addArrangedSubview(&self.format_indicator(config, format, current, false));
            }
            IndicatorStyle::Multi => {
                for &virtual_index in all {
                    let field = self.format_indicator(
                        config,
                        format,
                        virtual_index,
                        virtual_index == current,
                    );
                    stack.addArrangedSubview(&field);
                }
            }
            IndicatorStyle::Occupied => {
                stack.setSpacing(BADGE_SPACING);
                for &virtual_index in all {
                    let label = indicator_label(format, virtual_index, false, ' ', ' ');
                    let badge = WorkspaceBadge::new(
                        self.mtm,
                        &label,
                        virtual_index == current,
                        config.menubar_indicator_font_size(),
                    );
                    stack.addArrangedSubview(&badge);
                }
            }
            IndicatorStyle::Paged => {
                let last = paged_last_index(all.len());
                stack.addArrangedSubview(&self.format_indicator(config, format, current, true));
                stack.addArrangedSubview(&self.indicator_field(config, "/", false));
                stack.addArrangedSubview(&self.format_indicator(config, format, last, false));
            }
        }
        Some(stack)
    }

    fn format_indicator(
        &self,
        config: &Config,
        format: IndicatorFormat,
        virtual_index: u32,
        is_active: bool,
    ) -> Retained<NSTextField> {
        let label = indicator_label(
            format,
            virtual_index,
            is_active,
            config.menubar_indicator_active_character(),
            config.menubar_indicator_inactive_character(),
        );
        self.indicator_field(
            config,
            &label,
            is_active && format != IndicatorFormat::Unicode,
        )
    }

    /// One label of the indicator, bold when it stands for the active
    /// workspace.
    fn indicator_field(&self, config: &Config, label: &str, bold: bool) -> Retained<NSTextField> {
        let font_size = config.menubar_indicator_font_size();
        let font = if bold {
            NSFont::boldSystemFontOfSize(font_size)
        } else {
            NSFont::systemFontOfSize(font_size)
        };
        let field = NSTextField::labelWithString(&NSString::from_str(label), self.mtm);
        field.setFont(Some(&font));
        field
    }

    fn build_descriptor(&self, config: &Config) -> Option<Retained<NSStackView>> {
        let style = config.menubar_descriptor_style();
        if style == DescriptorStyle::Hidden {
            return None;
        }
        let stack = NSStackView::new(self.mtm);
        stack.setSpacing(MENU_BAR_SPACING);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setAlignment(NSLayoutAttribute::CenterY);
        if matches!(style, DescriptorStyle::Symbol | DescriptorStyle::Both)
            && let Some(image_view) = self.build_descriptor_symbol(config)
        {
            stack.addArrangedSubview(&image_view);
        }
        if matches!(style, DescriptorStyle::Text | DescriptorStyle::Both) {
            stack.addArrangedSubview(&self.build_descriptor_text(config));
        }
        (!stack.arrangedSubviews().is_empty()).then_some(stack)
    }

    fn build_descriptor_symbol(&self, config: &Config) -> Option<Retained<NSImageView>> {
        let symbol = config.menubar_descriptor_symbol();
        let image = if symbol == FIH_MARK_SYMBOL {
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(FIH_MARK))
        } else {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(&symbol),
                None,
            )
        };
        let Some(image) = image else {
            warn!(%symbol, "unable to load menu bar descriptor symbol");
            return None;
        };
        image.setTemplate(true);

        let size = image.size();
        let aspect_ratio = if size.height > 0.0 {
            size.width / size.height
        } else {
            1.0
        };
        let scaled = CGSize::new(14.0 * aspect_ratio, 14.0);
        image.setSize(scaled);

        let image_view = NSImageView::new(self.mtm);
        image_view.setImage(Some(&image));
        image_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        image_view.setFrameSize(scaled);
        if config.menubar_has_external_gradient() {
            image_view.setContentTintColor(Some(&*NSColor::whiteColor()));
        }
        Some(image_view)
    }

    fn build_descriptor_text(&self, config: &Config) -> Retained<NSTextField> {
        let field = NSTextField::labelWithString(
            &NSString::from_str(&config.menubar_descriptor_text()),
            self.mtm,
        );
        field.setFont(Some(&NSFont::menuBarFontOfSize(0.0)));
        field
    }
}

impl Drop for MenuBarManager {
    fn drop(&mut self) {
        self.status_bar.removeStatusItem(&self.status_item);
    }
}

/// Position-only animation and swipe frames do not affect menu content.
pub(crate) fn menu_bar_dirty(
    // Any strip: a window moved to another workspace changes what it shows.
    strip_changed: Query<(), Changed<LayoutStrip>>,
    workspace_changed: Query<(), Added<ActiveWorkspaceMarker>>,
    strips_added: Query<(), Added<LayoutStrip>>,
    mut strips_removed: RemovedComponents<LayoutStrip>,
) -> bool {
    // Drain the removal reader even when another input already dirtied us.
    let strips_removed = strips_removed.read().count() > 0;
    !strip_changed.is_empty()
        || !workspace_changed.is_empty()
        || !strips_added.is_empty()
        || strips_removed
}

pub fn update_menu_bar(
    active_display: ActiveDisplay,
    workspaces: Query<&LayoutStrip>,
    windows: Windows,
    apps: Query<&Application>,
    config: Res<Config>,
    visibility: Res<MenuBarVisibility>,
    menu_bar: Option<NonSendMut<MenuBarManager>>,
) {
    let Some(mut menu_bar) = menu_bar else {
        return;
    };
    if config.is_changed() {
        menu_bar.current_content = None;
    }
    let enabled = visibility.enabled(&config);
    menu_bar.set_visible(enabled);
    if !enabled {
        return;
    }
    let strip = active_display.active_strip();
    let app_name = |entity| {
        let window = windows.get(entity)?;
        let (_, _, parent) = windows.find_parent(window.id())?;
        apps.get(parent).ok().map(|app| app.name().to_string())
    };
    let mut summaries = workspaces
        .iter()
        .filter(|workspace| workspace.id() == strip.id())
        .map(|workspace| WorkspaceSummary {
            virtual_index: workspace.virtual_index,
            apps: workspace
                .all_windows()
                .into_iter()
                .filter_map(app_name)
                .collect(),
        })
        .collect::<Vec<_>>();
    summaries.sort_by_key(|summary| summary.virtual_index);
    summaries.dedup_by_key(|summary| summary.virtual_index);

    menu_bar.update(strip.virtual_index, &summaries, &config);
}

/// The workspaces the `occupied` indicator shows: the active one, and every
/// one with a window in it.
fn occupied_indices(current: u32, workspaces: &[WorkspaceSummary]) -> Vec<u32> {
    let mut indices = workspaces
        .iter()
        .filter(|workspace| !workspace.apps.is_empty())
        .map(|workspace| workspace.virtual_index)
        .chain([current])
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// The apps of a workspace as a menu reads them: in order, with repeats
/// folded into a count ("Ghostty ×2 · Safari").
fn apps_label(apps: &[String]) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for app in apps {
        match counted.iter_mut().find(|(name, _)| name == app) {
            Some((_, count)) => *count += 1,
            None => counted.push((app, 1)),
        }
    }
    counted
        .into_iter()
        .map(|(name, count)| {
            if count > 1 {
                format!("{name} ×{count}")
            } else {
                name.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(crate) fn virtual_workspace_label(virtual_index: u32) -> String {
    (virtual_index + 1).to_string()
}

/// What a single indicator position reads as.
fn indicator_label(
    format: IndicatorFormat,
    virtual_index: u32,
    is_active: bool,
    active_character: char,
    inactive_character: char,
) -> String {
    match format {
        IndicatorFormat::Default => virtual_workspace_label(virtual_index),
        IndicatorFormat::Roman => roman_numeral(virtual_index.saturating_add(1)),
        IndicatorFormat::Unicode => if is_active {
            active_character
        } else {
            inactive_character
        }
        .to_string(),
        IndicatorFormat::Marked if is_active => virtual_workspace_label(virtual_index),
        IndicatorFormat::Marked => inactive_character.to_string(),
    }
}

/// The index the paged indicator's total is formatted from: the workspace
/// count reads as the last workspace's own label, so it goes through the same
/// formatter and comes out Roman when the format asks for it.
fn paged_last_index(count: usize) -> u32 {
    u32::try_from(count.max(1) - 1).unwrap_or(u32::MAX)
}

fn roman_numeral(value: u32) -> String {
    const NUMERALS: [(u32, &str); 7] = [
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];

    let mut remaining = value;
    let mut numeral = String::new();
    while remaining > 0 {
        let Some(&(weight, symbol)) = NUMERALS.iter().find(|(weight, _)| *weight <= remaining)
        else {
            break;
        };
        remaining -= weight;
        numeral.push_str(symbol);
    }
    numeral
}

/// Renders `content` into a bitmap at `scale`, the flat stand-in the status
/// item button draws instead of `content` itself.
fn render_content_bitmap(
    content: &NSView,
    bounds: CGRect,
    scale: CGFloat,
) -> Option<Retained<NSBitmapImageRep>> {
    content.setFrame(bounds);
    content.layoutSubtreeIfNeeded();

    let rep = bitmap_rep(bounds, scale)?;
    content.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    Some(rep)
}

/// An empty `bounds`-sized bitmap to draw into, backed by `scale` pixels per
/// point so the render survives a Retina menu bar.
fn bitmap_rep(bounds: CGRect, scale: CGFloat) -> Option<Retained<NSBitmapImageRep>> {
    let pixels_wide = NSInteger::try_from(round_px(bounds.size.width * scale)).ok()?;
    let pixels_high = NSInteger::try_from(round_px(bounds.size.height * scale)).ok()?;

    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            pixels_wide,
            pixels_high,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    rep.setSize(bounds.size);
    Some(rep)
}

#[cfg(test)]
mod tests {
    use super::{
        Command, Config, Event, IndicatorFormat, MenuBarPreferenceFile, MenuBarVisibility,
        indicator_label, paged_last_index, roman_numeral, virtual_workspace_label,
    };

    #[test]
    fn menu_bar_choice_survives_workspace_changes_and_restarts() {
        use crate::commands::{Direction, Operation};
        use crate::tests::TestHarness;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "paneru-menu-preference-{}-{stamp}.json",
            std::process::id()
        ));
        let hidden_path = path.clone();
        let shown_path = path.clone();
        let mut harness = TestHarness::new().with_windows(2);
        harness
            .app
            .insert_resource(MenuBarPreferenceFile(path.clone()));
        harness
            .on_iteration(0, move |world, _| {
                let config = world.resource::<Config>();
                assert!(!world.resource::<MenuBarVisibility>().enabled(config));
                assert!(!MenuBarVisibility::load(&hidden_path).enabled(config));
            })
            .on_iteration(1, |world, _| {
                assert!(
                    !world
                        .resource::<MenuBarVisibility>()
                        .enabled(world.resource::<Config>())
                );
            })
            .on_iteration(2, move |world, _| {
                let config = world.resource::<Config>();
                assert!(world.resource::<MenuBarVisibility>().enabled(config));
                assert!(MenuBarVisibility::load(&shown_path).enabled(config));
            })
            .run(vec![
                Event::Command {
                    command: Command::MenuBar(false),
                },
                Event::Command {
                    command: Command::Window(Operation::Focus(Direction::East)),
                },
                Event::Command {
                    command: Command::MenuBar(true),
                },
            ]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_or_invalid_menu_bar_preference_uses_config() {
        let config =
            Config::try_from("[options]\n[bindings]\n[decorations.menu]\nenabled = false").unwrap();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("paneru-menu-fallback-{stamp}.json"));
        assert!(!MenuBarVisibility::load(&path).enabled(&config));
        MenuBarVisibility {
            enabled: Some(true),
            ..MenuBarVisibility::default()
        }
        .save(&path)
        .unwrap();
        assert!(MenuBarVisibility::load(&path).enabled(&config));
        std::fs::write(&path, "invalid JSON").unwrap();
        assert!(!MenuBarVisibility::load(&path).enabled(&config));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn loaded_menu_preference_is_not_rewritten_until_a_new_choice() {
        use crate::tests::TestHarness;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("paneru-menu-loaded-{stamp}.json"));
        let original = "{ \"enabled\": false }\n";
        std::fs::write(&path, original).unwrap();
        let mut harness = TestHarness::new();
        harness
            .app
            .insert_resource(MenuBarVisibility::load(&path))
            .insert_resource(MenuBarPreferenceFile(path.clone()));
        harness.app.update();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!path.with_extension("json.tmp").exists());

        harness.world().write_message(Event::Command {
            command: Command::MenuBar(false),
        });
        harness.app.update();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        harness.world().write_message(Event::Command {
            command: Command::MenuBar(true),
        });
        harness.app.update();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(MenuBarVisibility::load(&path).enabled(harness.world().resource::<Config>()));
        assert!(!saved.contains("dirty"));
        assert!(!harness.world().resource::<MenuBarVisibility>().dirty);

        // A repeated show command must not rewrite an already saved choice.
        let shown = "{ \"enabled\": true }\n";
        std::fs::write(&path, shown).unwrap();
        harness.world().write_message(Event::Command {
            command: Command::MenuBar(true),
        });
        harness.app.update();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), shown);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn menu_updates_ignore_position_animation_but_track_workspaces() {
        use crate::ecs::layout::LayoutStrip;
        use crate::ecs::{ActiveWorkspaceMarker, Bounds, FocusedMarker, Position};
        use bevy::app::{App, PostUpdate};
        use bevy::ecs::{resource::Resource, schedule::IntoScheduleConfigs, system::ResMut};
        use bevy::math::IVec2;

        #[derive(Resource, Default)]
        struct Updates(usize);
        fn count(mut updates: ResMut<Updates>) {
            updates.0 += 1;
        }
        let mut app = App::new();
        app.init_resource::<Updates>()
            .add_systems(PostUpdate, count.run_if(super::menu_bar_dirty));
        let strip = app
            .world_mut()
            .spawn((LayoutStrip::new(1, 0), ActiveWorkspaceMarker))
            .id();
        let window = app
            .world_mut()
            .spawn((
                Bounds(IVec2::new(400, 600)),
                Position(IVec2::ZERO),
                FocusedMarker,
            ))
            .id();
        app.update();
        assert_eq!(app.world().resource::<Updates>().0, 1);
        for x in 1..=30 {
            app.world_mut().get_mut::<Position>(window).unwrap().0.x = x;
            app.update();
        }
        // Window size and focus no longer show in the menu.
        app.world_mut().get_mut::<Bounds>(window).unwrap().0.x = 500;
        app.world_mut().entity_mut(window).remove::<FocusedMarker>();
        app.update();
        assert_eq!(app.world().resource::<Updates>().0, 1);
        app.world_mut()
            .get_mut::<LayoutStrip>(strip)
            .unwrap()
            .virtual_index = 1;
        app.update();
        assert_eq!(app.world().resource::<Updates>().0, 2);
        let offscreen = app.world_mut().spawn(LayoutStrip::new(1, 2)).id();
        app.update();
        assert_eq!(app.world().resource::<Updates>().0, 3);
        app.world_mut().despawn(offscreen);
        app.update();
        assert_eq!(app.world().resource::<Updates>().0, 4);
        app.update();
        assert_eq!(app.world().resource::<Updates>().0, 4);
    }

    #[test]
    fn menu_commands_apply_in_the_same_tick_as_pumped_events() {
        use crate::tests::TestHarness;
        use bevy::app::PreUpdate;
        use bevy::ecs::{message::MessageWriter, schedule::IntoScheduleConfigs, system::Local};

        // Inject a command immediately before the real pump. The handler must
        // run after the pump and see it in this update, including its condition.
        fn incoming(mut events: MessageWriter<Event>, mut sent: Local<bool>) {
            if !*sent {
                events.write(Event::Command {
                    command: Command::MenuBar(false),
                });
                *sent = true;
            }
        }
        let mut harness = TestHarness::new();
        harness
            .app
            .add_systems(PreUpdate, incoming.before(crate::ecs::systems::pump_events));
        harness.app.update();
        let world = harness.world();
        assert!(
            !world
                .resource::<MenuBarVisibility>()
                .enabled(world.resource::<Config>())
        );
    }

    fn summary(virtual_index: u32, apps: &[&str]) -> super::WorkspaceSummary {
        super::WorkspaceSummary {
            virtual_index,
            apps: apps.iter().map(|app| (*app).to_string()).collect(),
        }
    }

    #[test]
    fn occupied_indicator_shows_the_active_and_non_empty_workspaces() {
        let workspaces = [
            summary(0, &["Ghostty"]),
            summary(1, &[]),
            summary(2, &["WeChat"]),
        ];
        assert_eq!(super::occupied_indices(0, &workspaces), [0, 2]);
        // An empty active workspace still shows, so the bar always says where
        // you are.
        assert_eq!(super::occupied_indices(1, &workspaces), [0, 1, 2]);
        assert_eq!(super::occupied_indices(4, &[]), [4]);
    }

    #[test]
    fn app_list_folds_repeats_into_a_count() {
        let apps = ["Ghostty", "Edge", "Ghostty", "Finder", "Finder"].map(String::from);
        assert_eq!(super::apps_label(&apps), "Ghostty ×2 · Edge · Finder ×2");
        assert_eq!(super::apps_label(&[]), "");
    }

    #[test]
    fn virtual_workspace_label_is_one_based() {
        assert_eq!(virtual_workspace_label(0), "1");
        assert_eq!(virtual_workspace_label(4), "5");
    }

    #[test]
    fn paged_total_reads_as_the_last_workspace_label() {
        assert_eq!(
            indicator_label(
                IndicatorFormat::Default,
                paged_last_index(4),
                false,
                '☉',
                '○'
            ),
            "4"
        );
        assert_eq!(
            indicator_label(IndicatorFormat::Roman, paged_last_index(4), false, '☉', '○'),
            "IV"
        );
        // A lone workspace still reads as "1 / 1" rather than underflowing.
        assert_eq!(
            indicator_label(
                IndicatorFormat::Default,
                paged_last_index(1),
                false,
                '☉',
                '○'
            ),
            "1"
        );
        assert_eq!(
            indicator_label(
                IndicatorFormat::Default,
                paged_last_index(0),
                false,
                '☉',
                '○'
            ),
            "1"
        );
    }

    #[test]
    fn marked_labels_number_the_active_workspace_among_marks() {
        let marks: Vec<String> = [0, 1, 2, 3]
            .into_iter()
            .map(|index| indicator_label(IndicatorFormat::Marked, index, index == 1, '☉', '·'))
            .collect();
        assert_eq!(marks, ["·", "2", "·", "·"]);
    }

    #[test]
    fn unicode_labels_only_distinguish_active_from_inactive() {
        assert_eq!(
            indicator_label(IndicatorFormat::Unicode, 2, true, '☉', '○'),
            "☉"
        );
        assert_eq!(
            indicator_label(IndicatorFormat::Unicode, 2, false, '☉', '○'),
            "○"
        );
    }

    #[test]
    fn roman_numerals_terminate_and_subtract_correctly() {
        assert_eq!(roman_numeral(0), "");
        assert_eq!(roman_numeral(1), "I");
        assert_eq!(roman_numeral(3), "III");
        assert_eq!(roman_numeral(4), "IV");
        assert_eq!(roman_numeral(5), "V");
        assert_eq!(roman_numeral(9), "IX");
        assert_eq!(roman_numeral(10), "X");
        assert_eq!(roman_numeral(40), "XL");
        assert_eq!(roman_numeral(49), "XLIX");
        assert_eq!(roman_numeral(50), "L");
        assert_eq!(roman_numeral(89), "LXXXIX");
    }
}
