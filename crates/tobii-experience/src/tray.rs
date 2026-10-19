//! The tray icon: the project's own icons with Tobii's tooltips and menu as a StatusNotifierItem
//! (docs/experience-app.md section 1).

use std::io::Cursor;
use std::sync::LazyLock;
use std::sync::mpsc::Sender;

use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip};

use crate::status::{IconName, IconState, OPEN_TEXT, QUIT_TEXT, enable_disable_text, icon_info};

const TITLE: &str = "Tobii Experience";
const ID: &str = "tobii-linux-experience";

/// What the user asked for through the tray; the GTK main loop carries it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenPage,
    ToggleEnabled,
    Quit,
}

/// The StatusNotifierItem for the current icon state and enabled value.
#[derive(Debug)]
pub struct Tray {
    state: IconState,
    enabled: bool,
    actions: Sender<TrayAction>,
}

impl Tray {
    /// A tray that sends the user's choices to `actions`.
    pub fn new(actions: Sender<TrayAction>, state: IconState, enabled: bool) -> Tray {
        Tray { state, enabled, actions }
    }

    /// Shows the icon and tooltip for `state` and the menu text for `enabled`.
    pub fn update(&mut self, state: IconState, enabled: bool) {
        self.state = state;
        self.enabled = enabled;
    }

    fn send(&self, action: TrayAction) {
        let _ = self.actions.send(action);
    }
}

static PIXMAPS: LazyLock<Pixmaps> = LazyLock::new(Pixmaps::decode);

struct Pixmaps {
    progress: Vec<Icon>,
    get_started: Vec<Icon>,
    disabled: Vec<Icon>,
    enabled: Vec<Icon>,
}

impl Pixmaps {
    fn decode() -> Pixmaps {
        Pixmaps {
            progress: vec![
                argb(include_bytes!("../assets/tray/Progress-16.png")),
                argb(include_bytes!("../assets/tray/Progress-24.png")),
                argb(include_bytes!("../assets/tray/Progress-32.png")),
                argb(include_bytes!("../assets/tray/Progress-48.png")),
                argb(include_bytes!("../assets/tray/Progress-64.png")),
            ],
            get_started: vec![argb(include_bytes!("../assets/tray/GetStarted.png"))],
            disabled: vec![argb(include_bytes!("../assets/tray/Disabled.png"))],
            enabled: vec![argb(include_bytes!("../assets/tray/Enabled.png"))],
        }
    }

    fn icons(&self, name: IconName) -> Vec<Icon> {
        match name {
            IconName::Progress => self.progress.clone(),
            IconName::GetStarted => self.get_started.clone(),
            IconName::Disabled => self.disabled.clone(),
            IconName::Enabled => self.enabled.clone(),
        }
    }
}

/// Decodes one of the committed RGBA PNGs into the ARGB32, network byte order data that
/// StatusNotifierItem expects. The PNGs are build inputs, so a bad one is a bug.
fn argb(png_bytes: &[u8]) -> Icon {
    let mut reader = png::Decoder::new(Cursor::new(png_bytes)).read_info().expect("the tray PNG header is valid");
    let mut rgba = vec![0; reader.output_buffer_size().expect("the tray PNG size fits in memory")];
    let info = reader.next_frame(&mut rgba).expect("the tray PNG data is valid");
    assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Rgba, png::BitDepth::Eight));
    let mut data = rgba;
    for pixel in data.as_chunks_mut::<4>().0.iter_mut() {
        pixel.rotate_right(1);
    }
    Icon { width: info.width as i32, height: info.height as i32, data }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        ID.into()
    }

    fn title(&self) -> String {
        TITLE.into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        PIXMAPS.icons(icon_info(self.state).0)
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: icon_info(self.state).1.into(), ..ToolTip::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayAction::OpenPage);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, action: TrayAction| {
            MenuItem::Standard(StandardItem {
                label: label.into(),
                activate: Box::new(move |tray: &mut Tray| tray.send(action)),
                ..StandardItem::default()
            })
        };
        vec![
            item(OPEN_TEXT, TrayAction::OpenPage),
            item(enable_disable_text(self.enabled), TrayAction::ToggleEnabled),
            item(QUIT_TEXT, TrayAction::Quit),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver};

    use ksni::Tray as _;

    use super::*;

    fn tray(state: IconState, enabled: bool) -> (Tray, Receiver<TrayAction>) {
        let (sender, receiver) = mpsc::channel();
        (Tray::new(sender, state, enabled), receiver)
    }

    fn labels(tray: &Tray) -> Vec<String> {
        tray.menu()
            .into_iter()
            .map(|item| match item {
                MenuItem::Standard(item) => item.label,
                _ => panic!("the menu holds standard items only"),
            })
            .collect()
    }

    fn click(tray: &mut Tray, index: usize) {
        match tray.menu().remove(index) {
            MenuItem::Standard(item) => (item.activate)(tray),
            _ => panic!("the menu holds standard items only"),
        }
    }

    #[test]
    fn menu_order_matches_tobii() {
        let (mut tray, _) = tray(IconState::Tracking, true);
        assert_eq!(labels(&tray), ["Open Tobii Experience", "Disable Tobii Experience", "Quit"]);
        tray.update(IconState::TrackingPaused, false);
        assert_eq!(labels(&tray), ["Open Tobii Experience", "Enable Tobii Experience", "Quit"]);
    }

    #[test]
    fn menu_items_and_left_click_send_their_actions() {
        let (mut tray, actions) = tray(IconState::Tracking, true);
        click(&mut tray, 0);
        click(&mut tray, 1);
        click(&mut tray, 2);
        tray.activate(0, 0);
        let sent: Vec<_> = actions.try_iter().collect();
        assert_eq!(sent, [TrayAction::OpenPage, TrayAction::ToggleEnabled, TrayAction::Quit, TrayAction::OpenPage]);
    }

    #[test]
    fn pixmap_is_argb_big_endian() {
        let (tray, _) = tray(IconState::Tracking, true);
        let [icon] = &tray.icon_pixmap()[..] else { panic!("Enabled has one size") };
        assert_eq!((icon.width, icon.height), (33, 33));
        let pixel = |x: usize, y: usize| icon.data[(y * 33 + x) * 4..][..4].to_vec();
        assert_eq!(pixel(2, 16), [0xff, 0x2e, 0x9c, 0xdb], "opaque blue outline: A, R, G, B");
        assert_eq!(pixel(0, 0), [0x00, 0xff, 0xff, 0xff], "transparent corner");
    }

    #[test]
    fn pixmap_sizes_per_icon() {
        let sizes = |state| {
            let (tray, _) = tray(state, true);
            tray.icon_pixmap().iter().map(|icon| (icon.width, icon.height)).collect::<Vec<_>>()
        };
        assert_eq!(sizes(IconState::Initializing), [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64)]);
        assert_eq!(sizes(IconState::Configuring), sizes(IconState::Initializing));
        for state in [IconState::Tracking, IconState::TrackingPaused, IconState::GetStarted] {
            assert_eq!(sizes(state), [(33, 33)], "{state:?}");
        }
    }

    #[test]
    fn icon_follows_the_state() {
        let pixels = |state| tray(state, true).0.icon_pixmap()[0].data.clone();
        assert_ne!(pixels(IconState::Tracking), pixels(IconState::TrackingPaused));
        assert_ne!(pixels(IconState::Tracking), pixels(IconState::GetStarted));
        assert_eq!(pixels(IconState::Error), pixels(IconState::Disconnected));
    }

    #[test]
    fn tooltip_per_state() {
        for state in [
            IconState::GetStarted,
            IconState::Tracking,
            IconState::Error,
            IconState::TrackingPaused,
            IconState::InvalidConfiguration,
            IconState::Initializing,
            IconState::Disconnected,
            IconState::Configuring,
        ] {
            let (tray, _) = tray(state, true);
            assert_eq!(tray.tool_tip().title, icon_info(state).1, "{state:?}");
        }
        let (mut tray, _) = tray(IconState::Tracking, true);
        assert_eq!(tray.tool_tip().title, "Tobii Experience - In use");
        tray.update(IconState::TrackingPaused, false);
        assert_eq!(tray.tool_tip().title, "Tobii Experience - Disabled");
    }

    #[test]
    fn identity() {
        let (tray, _) = tray(IconState::Tracking, true);
        assert_eq!(tray.id(), "tobii-linux-experience");
        assert_eq!(tray.title(), "Tobii Experience");
    }
}
