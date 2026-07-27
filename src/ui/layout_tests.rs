//! Layout assertions, run headless against the real chrome placement.
//!
//! See [`crate::ui::harness`] for what this can and cannot see.

use egui::Rect;

use crate::color::Sample;
use crate::state::{Control, Mode, Point};
use crate::ui::harness::{self, assert_no_overlaps, assert_within, Harness, OVERLAY_CANVAS};
use crate::ui::theme;

/// Every state the chrome can be in, as a fresh harness plus a description.
///
/// The sweep matters more than any single case: the bug this file exists to
/// prevent was a chip that only collided in some configurations, and was
/// checked in one of the others.
/// The transient message, if any, that the chrome has to find room for.
#[derive(Clone, Copy, Debug)]
enum Message {
    None,
    Feedback,
    Prompt,
}

/// One state the chrome can be in. A description, not a built harness: feedback
/// and confirmation prompts expire on a wall clock (1.2 s and 1.5 s), so
/// building all sixty up front would let the later ones time out before they
/// were laid out — and a case whose chip has vanished silently checks nothing.
#[derive(Clone, Copy)]
struct Config {
    mode: Mode,
    session: bool,
    help: bool,
    message: Message,
}

impl Config {
    fn name(&self) -> String {
        let Config { mode, session, help, message } = self;
        format!("{mode:?}/session={session}/help={help}/message={message:?}")
    }

    fn build(&self) -> Harness {
        let mut harness = Harness::hd();
        harness.set_mode(self.mode).with_help(self.help);
        if self.session {
            harness.in_session();
        }
        match self.message {
            Message::None => {}
            Message::Feedback => {
                harness.with_feedback("Copied region with annotations");
            }
            Message::Prompt => {
                harness.with_prompt();
            }
        }
        harness
    }

    /// Whether this configuration should produce a message chip.
    fn expects_chip(&self) -> bool {
        !matches!(self.message, Message::None)
    }
}

/// Every state the chrome can be in.
///
/// The sweep matters more than any single case: the bug this file exists to
/// prevent was a chip that only collided in some configurations, and was
/// checked in one of the others.
fn every_configuration() -> Vec<Config> {
    let mut cases = Vec::new();
    for mode in Mode::ALL {
        for session in [false, true] {
            for help in [false, true] {
                for message in [Message::None, Message::Feedback, Message::Prompt] {
                    // A prompt only arms when there is work to lose.
                    if matches!(message, Message::Prompt) && !session {
                        continue;
                    }
                    cases.push(Config { mode, session, help, message });
                }
            }
        }
    }
    cases
}

#[test]
fn no_two_panels_ever_overlap() {
    // Regression: the message chip was pinned at a constant y that sat inside
    // the controls panel, so every confirmation prompt rendered on top of it.
    for case in every_configuration() {
        let name = case.name();
        let layout = case.build().layout();
        assert_eq!(
            layout.message.is_some(),
            case.expects_chip(),
            "{name}: the chip expired before it was laid out, so this case \
             checked nothing",
        );
        assert_no_overlaps(&name, &harness::panels(&layout));
    }
}

#[test]
fn nothing_is_laid_out_beyond_the_monitor() {
    for case in every_configuration() {
        let name = case.name();
        let mut harness = case.build();
        let screen = harness.screen();
        let layout = harness.layout();
        assert_within(&name, screen, &harness::everything(&layout));
    }
}

#[test]
fn the_chrome_still_fits_a_small_monitor() {
    // The panel is a fixed 380 px wide, so a narrow display is where a fixed
    // width stops fitting. 1024x768 is the smallest thing anyone still runs.
    let mut harness = Harness::new(1024.0, 768.0);
    harness.set_mode(Mode::RectDrag).in_session().with_prompt();

    let screen = harness.screen();
    let layout = harness.layout();
    assert_within("1024x768", screen, &harness::everything(&layout));
    assert_no_overlaps("1024x768", &harness::panels(&layout));
}

#[test]
fn a_modes_sliders_share_one_column() {
    // Two dials with titles of different lengths would otherwise start at
    // different x and read as two unrelated controls.
    for mode in Mode::ALL {
        let mut harness = Harness::hd();
        harness.set_mode(mode);
        let sliders: Vec<_> = harness.layout().panel.sliders().copied().collect();

        assert_eq!(
            sliders.len(),
            mode.controls().len(),
            "{mode:?} should show one slider per declared dial",
        );

        let Some(first) = sliders.first() else {
            panic!("{mode:?} shows no slider at all");
        };
        for slider in &sliders {
            assert!(
                (slider.track.min.x - first.track.min.x).abs() < 0.5,
                "{mode:?}: {:?} track starts at x={} but {:?} starts at x={}",
                slider.control,
                slider.track.min.x,
                first.control,
                first.track.min.x,
            );
            assert!(
                (slider.track.width() - first.track.width()).abs() < 0.5,
                "{mode:?}: {:?} track is {} px wide but {:?} is {} px",
                slider.control,
                slider.track.width(),
                first.control,
                first.track.width(),
            );
            assert!(
                (slider.label.width() - theme::CONTROL_LABEL_WIDTH).abs() < 0.5,
                "{mode:?}: {:?} label column is {} px, not the fixed {}",
                slider.control,
                slider.label.width(),
                theme::CONTROL_LABEL_WIDTH,
            );
        }
    }
}

#[test]
fn the_sliders_a_mode_shows_are_the_dials_it_declares() {
    // The panel and the wheel read the same list, so if this drifts the wheel
    // is moving something the user cannot see.
    for mode in Mode::ALL {
        let mut harness = Harness::hd();
        harness.set_mode(mode);
        let shown: Vec<Control> = harness
            .layout()
            .panel
            .sliders()
            .map(|slider| slider.control)
            .collect();
        assert_eq!(shown, mode.controls(), "{mode:?}");
    }
}

#[test]
fn a_two_dial_mode_is_taller_than_a_one_dial_mode() {
    let mut one = Harness::hd();
    one.set_mode(Mode::Crosshair);
    let one = one.layout().controls;

    let mut two = Harness::hd();
    two.set_mode(Mode::RectDrag);
    let two = two.layout().controls;

    assert_eq!(Mode::Crosshair.controls().len(), 1);
    assert_eq!(Mode::RectDrag.controls().len(), 2);
    assert!(
        two.height() > one.height(),
        "a second dial should make the panel taller: {} vs {}",
        two.height(),
        one.height(),
    );
}

#[test]
fn the_message_chip_holds_still_when_the_help_overlay_fades() {
    // Regression: the chip was stacked below the help overlay, so a
    // confirmation prompt appeared far down the screen and then jumped upward
    // the moment the fade finished — while the user was reading it.
    let with_help = message_rect(true);
    let without_help = message_rect(false);

    assert!(
        (with_help.min.y - without_help.min.y).abs() < 0.5,
        "the prompt moved {:.0} px when help disappeared: {with_help:?} then {without_help:?}",
        (with_help.min.y - without_help.min.y).abs(),
    );
}

fn message_rect(help: bool) -> Rect {
    let mut harness = Harness::hd();
    harness
        .set_mode(Mode::RectDrag)
        .in_session()
        .with_prompt()
        .with_help(help);
    harness
        .layout()
        .message
        .expect("an armed prompt should produce a chip")
}

#[test]
fn the_message_chip_sits_between_the_panel_and_the_help_overlay() {
    let mut harness = Harness::hd();
    harness
        .set_mode(Mode::RectDrag)
        .in_session()
        .with_prompt()
        .with_help(true);
    let layout = harness.layout();

    let message = layout.message.expect("prompt");
    let help = layout.help.expect("help overlay");
    assert!(
        message.min.y >= layout.controls.max.y,
        "the chip should start below the panel: {message:?} vs {:?}",
        layout.controls,
    );
    assert!(
        help.min.y >= message.max.y,
        "help should start below the chip: {help:?} vs {message:?}",
    );
}

#[test]
fn every_mode_button_is_big_enough_to_click() {
    // Mode buttons are the only thing on the panel a user aims at with the
    // pointer rather than reaching by number key.
    const MIN_HIT: f32 = 24.0;

    let mut harness = Harness::hd();
    harness.in_session();
    let layout = harness.layout();

    for (mode, rect) in &layout.panel.mode_buttons {
        assert!(
            rect.width() >= MIN_HIT && rect.height() >= MIN_HIT,
            "{mode:?}'s button is {}x{}, below the {MIN_HIT} px hit target",
            rect.width(),
            rect.height(),
        );
    }
}

#[test]
fn the_session_badge_appears_only_in_a_session() {
    let mut quick = Harness::hd();
    assert!(
        quick.layout().panel.session_buttons.is_none(),
        "the SESSION badge and its exports belong to a session",
    );

    let mut session = Harness::hd();
    session.in_session();
    assert!(session.layout().panel.session_buttons.is_some());
}

// ---------------------------------------------------------------- overlay

/// Puts `harness` into a state where `mode` actually has something to draw.
///
/// A mode with nothing measured paints nothing, and a bounds test over an empty
/// shape list passes without checking anything.
fn measuring(harness: &mut Harness, mode: Mode, x: f32, y: f32) {
    let canvas = harness.canvas();
    harness.state.set_mode(mode);
    harness.cursor_at(x, y);

    // Geometry is placed *inward* from the cursor. A drag that ran off the
    // bottom of the screen is not a state a pointer can reach, and asserting
    // that the overlay keeps it on screen would be asking the UI to fix up an
    // impossible input rather than testing anything real.
    let inward = |v: f32, extent: f32, span: f32| if v > span / 2.0 { -extent } else { extent };
    let (dx, dy) = (inward(x, 60.0, canvas.x), inward(y, 40.0, canvas.y));

    match mode {
        // Crosshair measures wherever the cursor is, with no further setup.
        Mode::Crosshair => {}
        Mode::RectDrag | Mode::ShrinkToFit | Mode::Container => {
            harness.state.drag.monitor = 0;
            harness.state.drag.has_selection = true;
            harness.state.drag.start = (x, y);
            harness.state.drag.end = (x + dx, y + dy);
            if mode == Mode::Container {
                // Container resolves through `active_rect`, which reads only
                // `state.container` — the drag is cleared so a regression there
                // cannot hide behind the drag fallback.
                harness.state.container = harness.state.drag.rect();
                harness.state.drag = Default::default();
            }
        }
        Mode::ColorPicker => {
            harness.state.sample = Some((
                Point { monitor: 0, x, y },
                Sample::from_rgb(230, 25, 94),
            ));
        }
        Mode::Distance => {
            harness.state.distance_anchor = Some(Point {
                monitor: 0,
                x: x + dx / 2.0,
                y: y + dy / 2.0,
            });
        }
    }
}

/// Cursor positions that put a chip against each edge and corner.
fn probe_points() -> Vec<(f32, f32)> {
    let (w, h) = (OVERLAY_CANVAS.x, OVERLAY_CANVAS.y);
    let xs = [0.0, 1.0, w / 2.0, w - 2.0, w - 1.0];
    let ys = [0.0, 1.0, h / 2.0, h - 2.0, h - 1.0];
    xs.iter()
        .flat_map(|x| ys.iter().map(move |y| (*x, *y)))
        .collect()
}

#[test]
fn measurement_text_never_leaves_the_monitor() {
    // What `floating_panel_position` exists to guarantee, checked on the
    // composed overlay rather than on the helper in isolation: a chip anchored
    // in a corner has to flip to the other side of its anchor, not hang off.
    //
    // Text only. Stroke decorations -- the crosshair's end caps especially --
    // are drawn centred on a point that can legitimately be the last pixel of
    // the screen, so half a cap lands outside by design. A clipped *reading* is
    // the thing a user cannot recover from.
    for mode in Mode::ALL {
        for (x, y) in probe_points() {
            let mut harness = Harness::overlay();
            let screen = harness.screen();
            measuring(&mut harness, mode, x, y);

            let Some(text) = harness.text_bounds() else {
                panic!("{mode:?} at ({x}, {y}) drew no reading at all");
            };
            assert!(
                screen.contains_rect(text),
                "{mode:?} at ({x}, {y}) put its reading at {text:?}, outside {screen:?}",
            );
        }
    }
}

#[test]
fn measurement_marks_stay_near_the_monitor() {
    // The loose companion to the strict text check. Not an arbitrary slack:
    // the one thing that legitimately overhangs is a crosshair end cap centred
    // on the last pixel of the screen, which reaches half its length plus half
    // its stroke past the edge. Anything beyond that flipped the wrong way.
    const STRAY: f32 = crate::ui::overlay::TICK_HALF_LENGTH + 1.0;

    for mode in Mode::ALL {
        for (x, y) in probe_points() {
            let mut harness = Harness::overlay();
            let allowed = harness.screen().expand(STRAY);
            measuring(&mut harness, mode, x, y);

            let Some(painted) = harness.painted_bounds() else {
                panic!("{mode:?} drew nothing at ({x}, {y})");
            };
            assert!(
                allowed.contains_rect(painted),
                "{mode:?} at ({x}, {y}) painted {painted:?}, well outside {allowed:?}",
            );
        }
    }
}

#[test]
fn every_mode_draws_its_measurement() {
    // Guards the inverse of the bounds sweep: a mode that silently stopped
    // drawing would satisfy every containment check trivially.
    for mode in Mode::ALL {
        let mut harness = Harness::overlay();
        measuring(&mut harness, mode, 120.0, 80.0);
        assert!(
            !harness.shapes().is_empty(),
            "{mode:?} drew nothing while measuring",
        );
    }
}

#[test]
fn an_armed_export_hides_the_live_measurement() {
    // The export drag is a different gesture layered over the same screen, so
    // the mode's own marks would be noise on top of the region being chosen.
    let mut harness = Harness::overlay();
    measuring(&mut harness, Mode::Crosshair, 120.0, 80.0);
    let measuring_shapes = harness.shapes().len();

    harness.state.set_session(true);
    harness.state.export_armed = true;
    harness.state.export_drag.has_selection = true;
    harness.state.export_drag.start = (20.0, 20.0);
    harness.state.export_drag.end = (140.0, 100.0);

    let shapes = harness.shapes();
    assert_eq!(
        shapes.len(),
        1,
        "an armed export should draw its selection and nothing else, \
         but drew {} shapes (the mode alone draws {measuring_shapes})",
        shapes.len(),
    );
}

#[test]
fn an_inactive_monitor_draws_only_its_annotations() {
    // The cursor is on one screen at a time; the others still show what was
    // placed on them, but no live marks.
    let mut harness = Harness::overlay();
    measuring(&mut harness, Mode::Crosshair, 120.0, 80.0);
    assert!(!harness.shapes().is_empty(), "a live cursor draws marks");

    harness.state.pointer = None;
    assert!(
        harness.shapes().is_empty(),
        "with no pointer there is nothing live to draw",
    );
}

// ------------------------------------------------------- input transparency

#[test]
fn a_transient_message_does_not_swallow_the_input_behind_it() {
    // Regression: making the chip a real widget gave its Area a rect, and
    // `wants_pointer_input()` is true over *any* interactable area, so
    // `handle_input` returned early inside that band. The export prompt lands
    // exactly where it is telling the user to start dragging, so the gesture it
    // asks for was the one it ate.
    let mut harness = Harness::hd();
    harness
        .set_mode(Mode::RectDrag)
        .in_session()
        .with_feedback("Drag a region to copy it with annotations");

    let chip = harness.layout().message.expect("a chip to test");
    assert!(
        !harness.pointer_is_captured_at(chip.center()),
        "the message chip at {chip:?} is swallowing clicks and wheel notches",
    );
}

#[test]
fn the_help_overlay_does_not_swallow_the_input_behind_it() {
    // Same class: help covers a large part of the screen and has nothing to
    // click, so measuring under it has to keep working.
    let mut harness = Harness::hd();
    harness.set_mode(Mode::Crosshair).with_help(true);

    let help = harness.layout().help.expect("the help overlay");
    assert!(
        !harness.pointer_is_captured_at(help.center()),
        "the help overlay at {help:?} is swallowing clicks",
    );
}

#[test]
fn the_controls_panel_still_swallows_its_own_clicks() {
    // The inverse: the panel does have buttons, and a click that presses one
    // must not also place an annotation on the screen behind it.
    let mut harness = Harness::hd();
    let layout = harness.layout();
    let (_, button) = layout.panel.mode_buttons[0];

    assert!(
        harness.pointer_is_captured_at(button.center()),
        "a click on a mode button at {button:?} would fall through to the canvas",
    );
}

#[test]
fn the_panel_and_the_message_chip_share_a_centre() {
    // Regression: `left` centred the panel on PANEL_WIDTH while the frame added
    // a margin either side, so the panel sat half a margin right of centre. It
    // went unnoticed until the chip below it started centring properly.
    let mut harness = Harness::hd();
    harness.set_mode(Mode::RectDrag).in_session().with_prompt();

    let middle = harness.canvas().x / 2.0;
    let layout = harness.layout();
    let chip = layout.message.expect("prompt");

    for (name, rect) in harness::panels(&layout) {
        assert!(
            (rect.center().x - middle).abs() < 1.0,
            "{name} {rect:?} is centred on {}, not the monitor's {middle}",
            rect.center().x,
        );
    }
    assert!((layout.controls.center().x - chip.center().x).abs() < 1.0);
}
