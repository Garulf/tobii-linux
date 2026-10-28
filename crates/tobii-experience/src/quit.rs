//! Tobii's quit (docs/experience-app.md section 3): the confirmation dialog, the stop of the
//! service unit and the retry box. On Linux Tobii's `SystemQuit` write is
//! `systemctl --user stop <unit>`, and the app clears it with a `start` when it launches.

use std::fmt;
use std::process::Command;

pub const DIALOG_TITLE: &str = "Are you sure you want to quit Tobii Experience?";
pub const DIALOG_BODY: &str =
    "All supported sensors will be disabled.\nYou can launch the app again from the start menu.";
pub const CONFIRM_TEXT: &str = "Quit Tobii Experience";
pub const CANCEL_TEXT: &str = "Cancel";
pub const CHECKBOX_TEXT: &str = "Don\u{2019}t show this again.";
pub const FAILURE_BODY: &str = "There was a problem quitting the app.\nDo you want to try again?";
pub const FAILURE_CAPTION: &str = "Tobii Experience";

/// Starts and stops the service the app belongs to.
pub trait ServiceUnit: fmt::Debug + Send + Sync {
    fn start(&self, unit: &str) -> Result<(), String>;
    fn stop(&self, unit: &str) -> Result<(), String>;
}

/// The real [`ServiceUnit`]: `systemctl --user`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Systemctl;

/// `systemctl --user <verb> <unit>`.
pub fn systemctl_command(verb: &str, unit: &str) -> Command {
    let mut command = Command::new("systemctl");
    command.args(["--user", verb, unit]);
    command
}

impl ServiceUnit for Systemctl {
    fn start(&self, unit: &str) -> Result<(), String> {
        run(systemctl_command("start", unit))
    }

    fn stop(&self, unit: &str) -> Result<(), String> {
        run(systemctl_command("stop", unit))
    }
}

fn run(mut command: Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().map_err(|error| format!("cannot run {program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("{program} failed ({}): {}", output.status, stderr.trim()))
}

/// What a click on Quit does (Tobii's `NotificationAreaIconFeature` steps 1 to 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitStep {
    /// A quit dialog is open already.
    Ignore,
    /// Ask with the confirmation dialog.
    Confirm,
    /// Stop right away (`SkipConfirmQuitDialog`).
    Stop,
}

pub fn quit_step(dialog_open: bool, skip_confirmation: bool) -> QuitStep {
    if dialog_open {
        QuitStep::Ignore
    } else if skip_confirmation {
        QuitStep::Stop
    } else {
        QuitStep::Confirm
    }
}

/// Tobii's confirmation dialog and failure box as GTK windows.
#[cfg(feature = "gtk")]
pub mod dialog {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gtk::prelude::*;
    use gtk4 as gtk;

    use super::{CANCEL_TEXT, CHECKBOX_TEXT, CONFIRM_TEXT, DIALOG_BODY, DIALOG_TITLE, FAILURE_BODY, FAILURE_CAPTION};

    /// Tobii's `TobiiSecondaryWindowStyle`, the dialog body and its two buttons
    /// (`TobiiButtonStyle` and `TobiiActionButtonStyle`), docs/experience-app.md 3 and 5.4.
    /// The pills are 40 px high, so a 20 px radius is half their height; 6 px between them is
    /// what drawing each 120 px pill at scale 0.95 leaves.
    const CSS: &str = r#"
window.tobii-quit { background: #151617; border: 0.7px solid #808080; }
window.tobii-quit .tobii-quit-titlebar { min-height: 30px; }
window.tobii-quit .tobii-quit-close { min-width: 30px; min-height: 30px; padding: 0; border: none;
  border-radius: 0; background: transparent; color: #D9F4FF; box-shadow: none; }
window.tobii-quit .tobii-quit-close:hover { background: #1D4C5F; }
window.tobii-quit .tobii-quit-body { background: #FFFFFF; }
window.tobii-quit label, window.tobii-quit button {
  font-family: "Selawik Light", "Selawik", "Segoe UI Light", "DejaVu Sans", sans-serif; font-weight: 300; }
window.tobii-quit .tobii-quit-title { font-size: 16pt; color: #000000; }
window.tobii-quit .tobii-quit-text { font-size: 12pt; color: #000000; }
window.tobii-quit checkbutton label { font-size: 8pt; color: #000000; }
window.tobii-quit checkbutton check { min-width: 21px; min-height: 21px; }
window.tobii-quit .tobii-button { min-width: 120px; min-height: 40px; padding: 0 25px; border: none;
  border-radius: 20px; box-shadow: none; font-size: 12pt; color: #D9F4FF; background: #004B66;
  transition: background 100ms, color 100ms; }
window.tobii-quit .tobii-button:hover { background: #018EC1; color: #FFFFFF; }
window.tobii-quit .tobii-button:active { background: #00BEFF; color: #FFFFFF; transition: none; }
window.tobii-quit .tobii-action-button { background: #008DBF; }
window.tobii-quit .tobii-action-button:hover { background: #00ACE9; }
window.tobii-quit .tobii-action-button:active { background: #00BEFF; }
"#;

    fn install_css() {
        thread_local!(static INSTALLED: Cell<bool> = const { Cell::new(false) });
        if INSTALLED.replace(true) {
            return;
        }
        let Some(display) = gtk::gdk::Display::default() else { return };
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    /// WPF's `Thickness` order: left, top, right, bottom.
    fn margins(widget: &impl IsA<gtk::Widget>, left: i32, top: i32, right: i32, bottom: i32) {
        widget.set_margin_start(left);
        widget.set_margin_top(top);
        widget.set_margin_end(right);
        widget.set_margin_bottom(bottom);
    }

    fn text(content: &str, class: &str) -> gtk::Label {
        let label = gtk::Label::builder().label(content).wrap(true).xalign(0.0).css_classes([class]).build();
        label.set_halign(gtk::Align::Fill);
        label
    }

    fn button(label: &str, classes: &[&str]) -> gtk::Button {
        gtk::Button::builder().label(label).css_classes(classes.to_vec()).valign(gtk::Align::End).build()
    }

    /// Calls `answer` once when the window goes, however it is closed.
    fn on_close<T: Default + 'static>(window: &gtk::Window, chosen: Rc<Cell<T>>, answer: impl FnOnce(T) + 'static) {
        let answer = RefCell::new(Some(answer));
        window.connect_close_request(move |_| {
            if let Some(answer) = answer.borrow_mut().take() {
                answer(chosen.take());
            }
            gtk::glib::Propagation::Proceed
        });
    }

    /// The confirmation dialog: 600 x 300, undecorated, centred (Tobii's fallback position).
    /// `answer` gets `Some(dont_show_again)` for Quit Tobii Experience and `None` for Cancel
    /// or close; the window is closed first, as Tobii closes it before quitting.
    pub fn confirm(app: &gtk::Application, answer: impl FnOnce(Option<bool>) + 'static) -> gtk::Window {
        install_css();
        let window = gtk::Window::builder()
            .application(app)
            .title("Confirmation Dialog")
            .default_width(600)
            .default_height(300)
            .resizable(false)
            .decorated(false)
            .css_classes(["tobii-quit"])
            .build();

        let close =
            gtk::Button::builder().label("\u{2715}").css_classes(["tobii-quit-close"]).halign(gtk::Align::End).build();
        let titlebar = gtk::Box::builder().css_classes(["tobii-quit-titlebar"]).build();
        close.set_hexpand(true);
        titlebar.append(&close);

        let title = text(DIALOG_TITLE, "tobii-quit-title");
        margins(&title, 40, 80, 40, 20);
        let body = text(DIALOG_BODY, "tobii-quit-text");
        margins(&body, 40, 0, 40, 0);

        let checkbox = gtk::CheckButton::with_label(CHECKBOX_TEXT);
        margins(&checkbox, 20, 10, 20, 0);
        checkbox.set_valign(gtk::Align::Center);
        let cancel = button(CANCEL_TEXT, &["tobii-button"]);
        let confirm = button(CONFIRM_TEXT, &["tobii-button", "tobii-action-button"]);
        let buttons = gtk::Box::builder().spacing(6).halign(gtk::Align::End).hexpand(true).build();
        buttons.append(&confirm);
        buttons.append(&cancel);
        let bottom = gtk::Box::builder().vexpand(true).valign(gtk::Align::End).build();
        margins(&bottom, 0, 0, 10, 10);
        bottom.append(&checkbox);
        bottom.append(&buttons);

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tobii-quit-body"])
            .height_request(270)
            .vexpand(true)
            .build();
        panel.append(&title);
        panel.append(&body);
        panel.append(&bottom);

        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        content.append(&titlebar);
        content.append(&panel);
        window.set_child(Some(&content));

        let chosen: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
        on_close(&window, chosen.clone(), answer);
        let closing = window.clone();
        close.connect_clicked(move |_| closing.close());
        let closing = window.clone();
        cancel.connect_clicked(move |_| closing.close());
        let closing = window.clone();
        confirm.connect_clicked(move |_| {
            chosen.set(Some(checkbox.is_active()));
            closing.close();
        });
        window.present();
        window
    }

    /// Tobii's `MessageBox.Show(body, caption, YesNo, Error)`: `answer(true)` for Yes, which
    /// retries; No or closing the box is `false`.
    pub fn failure(app: &gtk::Application, answer: impl FnOnce(bool) + 'static) -> gtk::Window {
        let window = gtk::Window::builder().application(app).title(FAILURE_CAPTION).resizable(false).build();
        let icon = gtk::Image::from_icon_name("dialog-error");
        icon.set_pixel_size(32);
        icon.set_valign(gtk::Align::Start);
        let message = gtk::Label::builder().label(FAILURE_BODY).xalign(0.0).build();
        let row = gtk::Box::builder().spacing(12).build();
        row.append(&icon);
        row.append(&message);
        let yes = gtk::Button::with_mnemonic("_Yes");
        let no = gtk::Button::with_mnemonic("_No");
        let buttons = gtk::Box::builder().spacing(6).halign(gtk::Align::End).build();
        buttons.append(&yes);
        buttons.append(&no);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).build();
        margins(&content, 18, 18, 18, 12);
        content.append(&row);
        content.append(&buttons);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&yes));

        let chosen = Rc::new(Cell::new(false));
        on_close(&window, chosen.clone(), answer);
        let closing = window.clone();
        yes.connect_clicked(move |_| {
            chosen.set(true);
            closing.close();
        });
        let closing = window.clone();
        no.connect_clicked(move |_| closing.close());
        window.present();
        window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_tobiis() {
        assert_eq!(DIALOG_TITLE, "Are you sure you want to quit Tobii Experience?");
        assert_eq!(
            DIALOG_BODY.as_bytes(),
            b"All supported sensors will be disabled.\nYou can launch the app again from the start menu."
        );
        assert_eq!(CONFIRM_TEXT, "Quit Tobii Experience");
        assert_eq!(CANCEL_TEXT, "Cancel");
        assert_eq!(CHECKBOX_TEXT.as_bytes(), b"Don\xE2\x80\x99t show this again.");
        assert_eq!(FAILURE_BODY.as_bytes(), b"There was a problem quitting the app.\nDo you want to try again?");
        assert_eq!(FAILURE_CAPTION, "Tobii Experience");
    }

    #[test]
    fn quit_asks_unless_told_not_to_and_ignores_a_second_click() {
        assert_eq!(quit_step(false, false), QuitStep::Confirm);
        assert_eq!(quit_step(false, true), QuitStep::Stop);
        assert_eq!(quit_step(true, false), QuitStep::Ignore);
        assert_eq!(quit_step(true, true), QuitStep::Ignore);
    }

    #[test]
    fn systemctl_runs_in_the_user_manager() {
        for verb in ["start", "stop"] {
            let command = systemctl_command(verb, "tobii-head-pose.service");
            assert_eq!(command.get_program(), "systemctl");
            assert_eq!(command.get_args().collect::<Vec<_>>(), ["--user", verb, "tobii-head-pose.service"]);
        }
    }

    #[test]
    fn a_failed_command_reports_its_output() {
        let mut failing = Command::new("sh");
        failing.args(["-c", "echo no such unit >&2; exit 5"]);
        let error = run(failing).unwrap_err();
        assert!(error.contains("no such unit"), "{error}");
        assert!(run(Command::new("true")).is_ok());
        assert!(run(Command::new("/nonexistent/systemctl")).is_err());
    }
}
