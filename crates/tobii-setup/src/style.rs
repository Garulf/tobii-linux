//! The page's stylesheet in GTK CSS: Tobii's colours, type sizes and buttons
//! (docs/setup-flow.md). GTK CSS has no viewport units, so the sizes the page clamps to the
//! window height are computed here and the sheet is reloaded when the height changes.

fn clamp_px(low: f64, share: f64, high: f64, height: f64) -> f64 {
    (share * height).clamp(low, high)
}

/// The stylesheet for a window `height` pixels high.
pub fn css(height: f64) -> String {
    let explaining = clamp_px(22.0, 0.026, 37.0, height);
    let headline = clamp_px(18.0, 0.0195, 28.0, height);
    let subtext = clamp_px(14.0, 0.0135, 19.0, height);
    let button = clamp_px(13.0, 0.0115, 16.0, height);
    format!(
        r#"
window, window.background {{ background-color: #021419; color: #d9f4ff; }}
window.calibrating {{ background-color: #000000; }}
* {{ font-family: "Segoe UI Light", "Segoe UI", "Selawik", "Open Sans", "Noto Sans", "DejaVu Sans", sans-serif;
     font-weight: 300; }}
label {{ color: #d9f4ff; }}
.explaining {{ font-size: {explaining:.1}px; }}
.headline {{ font-size: {headline:.1}px; }}
.subtext {{ font-size: {subtext:.1}px; color: #bfd6e0; }}
.problem {{ font-size: {subtext:.1}px; color: #00ace9; }}
.end-text {{ color: #bfd6e0; }}
.ta-explaining {{ font-size: 37px; }}
.ta-headline {{ font-size: 28px; }}
.banner {{ background-color: #3b3000; color: #fdd663; font-size: {subtext:.1}px; padding: 10px; }}

button {{
  font-size: {button:.1}px; min-width: 70px; min-height: 33px; padding: 0 25px; border: none; border-radius: 17px;
  color: #d9f4ff; background-color: #004b66; background-image: none; box-shadow: none; text-shadow: none;
  outline: none; transition: background-color 100ms;
}}
button label {{ color: inherit; font-size: inherit; }}
button:hover {{ background-color: #018ec1; color: #ffffff; }}
button.primary {{ background-color: #008dbf; color: #ffffff; }}
button.primary:hover {{ background-color: #00ace9; }}
button:disabled {{ background-color: #2b3940; color: #637279; }}
button:focus-visible {{ outline: 1px solid #00ace9; outline-offset: 2px; }}

button.ta {{ font-size: 16px; color: #ffffff; border-radius: 16.5px; transform: scale(0.95);
             transition: background-color 100ms, transform 100ms; }}
button.ta:hover {{ background-color: #018ec1; transform: scale(0.975); }}
button.ta:active {{ background-color: #00beff; transform: scale(0.95); }}
button.ta:disabled {{ background-color: #2b3940; color: #637279; transform: scale(0.95); }}

button.flat-icon {{ min-width: 0; min-height: 0; padding: 0; border-radius: 0; background-color: transparent;
                    color: #afc4cd; transform: none; }}
button.flat-icon:hover {{ background-color: transparent; color: #ffffff; }}
button.flat-icon:active {{ background-color: transparent; transform: translateY(1px); }}
button.close:active {{ transform: none; }}

.message-box {{ background-color: #021419; border: 1px solid rgba(29, 76, 95, 0.3); padding: 15px 15px 20px 30px; }}
.box-title {{ font-size: 21.3px; margin-top: 15px; margin-bottom: 10px; }}
.box-body {{ font-size: 16px; color: #bfd6e0; margin-right: 15px; }}
.box-buttons {{ border-top: 1px solid rgba(29, 76, 95, 0.3); padding-top: 15px; margin-top: 20px; }}
.box-buttons button {{ border-radius: 17px; }}
button.box-close {{ min-width: 15px; min-height: 15px; padding: 0; color: #ffffff; font-size: 12px; }}

entry {{ font-size: 19px; padding: 6px 10px; color: #ffffff; background-color: #0b2a33; background-image: none;
         border: 1px solid #1d4c5f; border-radius: 3px; box-shadow: none; outline: none; min-height: 0; }}
entry:focus-within {{ border-color: #008dbf; }}
entry selection {{ background-color: #0b4fa8; color: #ffffff; }}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_sizes_follow_the_window_height() {
        assert!(css(720.0).contains(".explaining { font-size: 22.0px; }"));
        assert!(css(1440.0).contains(".explaining { font-size: 37.0px; }"));
        assert!(css(1440.0).contains(".headline { font-size: 28.0px; }"));
    }
}
