//! The widgets of each screen.

use gtk4 as gtk;
use gtk4::prelude::*;

use crate::alignment::{ATTACH, DONE, THIS_DISPLAY, WHERE};
use crate::positioning_view::{SIT_UPRIGHT, WELCOME_ONE, WELCOME_TWO};

pub const CONNECTING: &str = "Connecting to the eye tracker...";
pub const SAVE_QUESTION: &str = "Save the new display setup?";
pub const SAVE_EXPLANATION: &str = "The eye tracker uses it to know where the screen is.";
pub const SORRY: &str = "We\u{2019}re sorry...";
pub const ASPECT_BODY: &str = "Eye tracking may not work as expected with this screen resolution. Do you want to change the screen settings before continuing?";
pub const NO_DETECTION: &str = "Oops. No detection.";
pub const NO_DETECTION_BODY: &str =
    "The eye tracker could not see your eyes well enough. Let's try again, and maybe follow these tips:";
pub const TIPS: [&str; 4] = [
    "Keep looking at each dot until it explodes.",
    "If you wear glasses, give them a wipe.",
    "Avoid bright, direct light on you or the eye tracker.",
    "Relax, it's fine to blink.",
];
pub const SUCCESS: &str = "Calibration successful!";
pub const PROFILE_BODY: &str = "It is kept as a profile, so you will not have to do this again. You can make more profiles later, for example with and without glasses.";
pub const PROFILE_LABEL: &str = "Profile name:";
pub const WELCOME: &str = "Let's calibrate for your eyes...";
pub const INSTRUCTION: &str = "Look at each dot until it explodes!";
pub const BANNER: &str = "This window has to fill the screen. Click anywhere or press F11.";

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    label.set_can_target(false);
    for class in classes {
        label.add_css_class(class);
    }
    label
}

pub fn left_label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = label(text, classes);
    label.set_justify(gtk::Justification::Left);
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Fill);
    label
}

pub fn button(text: &str, primary: bool) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    if primary {
        button.add_css_class("primary");
    }
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    button
}

pub fn spinner_area(size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_halign(gtk::Align::Center);
    area.set_can_target(false);
    area
}

fn centred_column(spacing: i32) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    column.set_halign(gtk::Align::Center);
    column.set_valign(gtk::Align::Center);
    column
}

fn buttons_row(buttons: &[&gtk::Button], margin_top: i32, align: gtk::Align) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 15);
    row.set_halign(align);
    row.set_margin_top(margin_top);
    for button in buttons {
        row.append(*button);
    }
    row
}

/// Connecting, saving and moving: a spinner and one line of text.
#[derive(Debug, Clone)]
pub struct BusyPage {
    pub root: gtk::Box,
    pub spinner: gtk::DrawingArea,
    pub connecting: gtk::Label,
    pub title: gtk::Label,
}

impl BusyPage {
    pub fn new() -> Self {
        let root = centred_column(0);
        let spinner = spinner_area(48);
        spinner.set_margin_bottom(30);
        let connecting = label(CONNECTING, &["explaining"]);
        let title = label("", &["headline"]);
        root.append(&spinner);
        root.append(&connecting);
        root.append(&title);
        Self { root, spinner, connecting, title }
    }
}

/// The last screen: what happened and Close.
#[derive(Debug, Clone)]
pub struct EndPage {
    pub root: gtk::Box,
    pub title: gtk::Label,
    pub text: gtk::Label,
    pub close: gtk::Button,
    pub hint: gtk::Label,
}

impl EndPage {
    pub fn new() -> Self {
        let root = centred_column(0);
        let title = label("", &["explaining"]);
        let text = label("", &["headline", "end-text"]);
        text.set_margin_top(18);
        let close = button("Close", true);
        let hint = label("", &["subtext"]);
        hint.set_margin_top(20);
        root.append(&title);
        root.append(&text);
        root.append(&buttons_row(&[&close], 50, gtk::Align::Center));
        root.append(&hint);
        Self { root, title, text, close, hint }
    }
}

/// "Save the new display setup?"
#[derive(Debug, Clone)]
pub struct SavePage {
    pub root: gtk::Box,
    pub summary: gtk::Label,
    pub save: gtk::Button,
    pub keep: gtk::Button,
}

impl SavePage {
    pub fn new() -> Self {
        let root = centred_column(0);
        let summary = label("", &["headline"]);
        summary.set_margin_top(8);
        let explanation = label(SAVE_EXPLANATION, &["subtext"]);
        explanation.set_margin_top(20);
        let save = button("Save", true);
        let keep = button("Keep the old one", false);
        root.append(&label(SAVE_QUESTION, &["explaining"]));
        root.append(&summary);
        root.append(&explanation);
        root.append(&buttons_row(&[&save, &keep], 50, gtk::Align::Center));
        Self { root, summary, save, keep }
    }
}

/// CustomMessageBoxWindow: the aspect ratio warning.
#[derive(Debug, Clone)]
pub struct AspectPage {
    pub root: gtk::Box,
    pub yes: gtk::Button,
    pub no: gtk::Button,
    pub close: gtk::Button,
}

impl AspectPage {
    pub fn new() -> Self {
        let root = centred_column(0);
        let message = gtk::Box::new(gtk::Orientation::Vertical, 0);
        message.add_css_class("message-box");
        message.set_size_request(470, -1);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let title = left_label(SORRY, &["box-title"]);
        title.set_hexpand(true);
        let close = gtk::Button::with_label("\u{2715}");
        close.add_css_class("flat-icon");
        close.add_css_class("box-close");
        close.set_valign(gtk::Align::Start);
        header.append(&title);
        header.append(&close);
        let body = left_label(ASPECT_BODY, &["box-body"]);
        body.set_max_width_chars(48);
        let yes = button("Yes", false);
        let no = button("No", false);
        let buttons = buttons_row(&[&yes, &no], 0, gtk::Align::Fill);
        buttons.add_css_class("box-buttons");
        buttons.set_spacing(10);
        yes.set_hexpand(true);
        yes.set_halign(gtk::Align::End);
        message.append(&header);
        message.append(&body);
        message.append(&buttons);
        root.append(&message);
        Self { root, yes, no, close }
    }
}

/// The content width of the failure and profile messages.
pub const MESSAGE_WIDTH: f64 = 760.0;

/// A page whose column the window places centred at [`MESSAGE_WIDTH`].
fn message_page(column: &gtk::Box) -> gtk::Overlay {
    let root = gtk::Overlay::new();
    root.set_child(Some(&gtk::Box::new(gtk::Orientation::Vertical, 0)));
    root.add_overlay(column);
    root
}

/// "Oops. No detection." with the tips, Try again and Cancel.
#[derive(Debug, Clone)]
pub struct FailedPage {
    pub root: gtk::Overlay,
    pub column: gtk::Box,
    pub retry: gtk::Button,
    pub cancel: gtk::Button,
}

impl FailedPage {
    pub fn new() -> Self {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let headline = left_label(NO_DETECTION, &["headline"]);
        headline.set_margin_bottom(30);
        column.append(&headline);
        column.append(&left_label(NO_DETECTION_BODY, &["subtext"]));
        let tips = gtk::Box::new(gtk::Orientation::Vertical, 4);
        tips.set_margin_top(14);
        tips.set_margin_start(20);
        for tip in TIPS {
            tips.append(&left_label(&format!("\u{2022}  {tip}"), &["subtext"]));
        }
        column.append(&tips);
        let retry = button("Try again", true);
        let cancel = button("Cancel", false);
        column.append(&buttons_row(&[&retry, &cancel], 50, gtk::Align::End));
        Self { root: message_page(&column), column, retry, cancel }
    }
}

/// "Calibration successful!" and the new profile's name.
#[derive(Debug, Clone)]
pub struct ProfilePage {
    pub root: gtk::Overlay,
    pub column: gtk::Box,
    pub entry: gtk::Entry,
    pub save: gtk::Button,
    pub problem: gtk::Label,
}

impl ProfilePage {
    pub fn new() -> Self {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let headline = left_label(SUCCESS, &["headline"]);
        headline.set_margin_bottom(30);
        let name = left_label(PROFILE_LABEL, &["subtext"]);
        name.set_margin_top(28);
        name.set_margin_bottom(8);
        let entry = gtk::Entry::new();
        entry.set_max_length(15);
        entry.set_size_request(400, -1);
        let save = button("Save", true);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 50);
        row.append(&entry);
        row.append(&save);
        let problem = left_label("", &["problem"]);
        problem.set_margin_top(9);
        column.append(&headline);
        column.append(&left_label(PROFILE_BODY, &["subtext"]));
        column.append(&name);
        column.append(&row);
        column.append(&problem);
        Self { root: message_page(&column), column, entry, save, problem }
    }
}

/// The display setup: the canvas, the texts, This display and Done, and the monitor switcher.
#[derive(Debug, Clone)]
pub struct DisplayPage {
    pub root: gtk::Overlay,
    pub canvas: gtk::DrawingArea,
    pub text: gtk::Box,
    pub explaining: gtk::Label,
    pub headline: gtk::Label,
    pub buttons: gtk::Box,
    pub this_display: gtk::Button,
    pub done: gtk::Button,
    pub previous: gtk::Button,
    pub next: gtk::Button,
    pub previous_icon: gtk::DrawingArea,
    pub next_icon: gtk::DrawingArea,
}

impl DisplayPage {
    pub fn new() -> Self {
        let root = gtk::Overlay::new();
        let canvas = gtk::DrawingArea::new();
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        root.set_child(Some(&canvas));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_valign(gtk::Align::End);
        text.set_can_target(false);
        let explaining = label(WHERE, &["ta-explaining"]);
        let headline = label(ATTACH, &["ta-headline"]);
        text.append(&explaining);
        text.append(&headline);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        buttons.set_halign(gtk::Align::Center);
        buttons.set_valign(gtk::Align::Center);
        let this_display = button(THIS_DISPLAY, false);
        let done = button(DONE, false);
        for button in [&this_display, &done] {
            button.add_css_class("ta");
            buttons.append(button);
        }
        done.set_margin_end(60);
        let switch = |name: &str| {
            let button = gtk::Button::new();
            button.add_css_class("flat-icon");
            button.update_property(&[gtk::accessible::Property::Label(name)]);
            let icon = gtk::DrawingArea::new();
            icon.set_can_target(false);
            button.set_child(Some(&icon));
            (button, icon)
        };
        let (previous, previous_icon) = switch("Left monitor");
        let (next, next_icon) = switch("Right monitor");
        for child in [text.upcast_ref::<gtk::Widget>(), buttons.upcast_ref(), previous.upcast_ref(), next.upcast_ref()]
        {
            root.add_overlay(child);
        }
        Self {
            root,
            canvas,
            text,
            explaining,
            headline,
            buttons,
            this_display,
            done,
            previous,
            next,
            previous_icon,
            next_icon,
        }
    }
}

/// Eye positioning: the track box and eyes on a canvas, and the texts around them.
#[derive(Debug, Clone)]
pub struct PositioningPage {
    pub root: gtk::Overlay,
    pub canvas: gtk::DrawingArea,
    pub upright: gtk::Label,
    pub welcome_one: gtk::Label,
    pub welcome_two: gtk::Label,
    pub hint: gtk::Label,
    pub status: gtk::Label,
}

impl PositioningPage {
    pub fn new() -> Self {
        let root = gtk::Overlay::new();
        let canvas = gtk::DrawingArea::new();
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        root.set_child(Some(&canvas));
        let upright = label(SIT_UPRIGHT, &["subtext"]);
        let welcome_one = label(WELCOME_ONE, &["explaining"]);
        let welcome_two = label(WELCOME_TWO, &["headline"]);
        let hint = label("", &["headline"]);
        let status = label("", &["subtext"]);
        for child in [&upright, &welcome_one, &welcome_two, &hint, &status] {
            root.add_overlay(child);
        }
        Self { root, canvas, upright, welcome_one, welcome_two, hint, status }
    }
}

/// The calibration: black, the targets on a canvas, and the two intro texts.
#[derive(Debug, Clone)]
pub struct CalibrationPage {
    pub root: gtk::Overlay,
    pub canvas: gtk::DrawingArea,
    pub welcome: gtk::Label,
    pub instruction: gtk::Label,
}

impl CalibrationPage {
    pub fn new() -> Self {
        let root = gtk::Overlay::new();
        let canvas = gtk::DrawingArea::new();
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        root.set_child(Some(&canvas));
        let welcome = label(WELCOME, &["explaining"]);
        let instruction = label(INSTRUCTION, &["explaining"]);
        root.add_overlay(&welcome);
        root.add_overlay(&instruction);
        Self { root, canvas, welcome, instruction }
    }
}
