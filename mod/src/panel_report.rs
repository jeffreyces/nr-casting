//! The report row: the two diagnostics buttons, and the prompt that starts a guided run.
//!
//! On its own line below the controls, because those are about editing an assignment while these
//! are about handing a stranger evidence — different weight, different frequency, and a button that
//! is disabled for five minutes at a time has no business sitting in the row you use constantly.

use imgui::Ui;

use crate::window::{help_line, vspace, SECTION_GAP};

/// What a click did, and where the prompt ended up.
pub(crate) struct ReportRow {
    pub diagnostics: bool,
    pub start_run: bool,
    /// The prompt's state after this frame (the buttons below are drawn from it next frame).
    pub prompt: bool,
}

/// Draw the row. `prompt` is whether the bundle-run explanation is currently on screen.
pub(crate) fn draw(ui: &Ui, prompt: bool) -> ReportRow {
    let mut row = ReportRow {
        diagnostics: false,
        start_run: false,
        prompt,
    };

    // Its own row, at the same distance as every other section. This used to call `new_line`,
    // which put the cursor a whole *line* below the row above - and that row ends in a combo, taller
    // than a button, so the gap came out visibly larger than the one above it and grew the content
    // past the window when the panel was resized.
    vspace(ui, SECTION_GAP);

    if prompt {
        help_line(
            ui,
            &format!(
                "Bundle run: load a character with the controller connected, stand somewhere \
                 quiet and still, then start. {} phases of {} s: everything on, then the raw-input \
                 hooks, the cursor hooks and the pad hook switched off one at a time, then every \
                 hook group off, then everything back on. The report is written and opened when \
                 the {} minutes are up. Casting stops during the pad phases - that is expected; the \
                 mouse and keyboard keep working.",
                crate::bisect::phases(),
                crate::bisect::PHASE_SECONDS,
                crate::bisect::total_seconds() / 60
            ),
        );
        vspace(ui, 6.0);
        // A run with no frame clock records nine empty minutes, so the start is refused here and
        // the reason is on screen rather than in the log alone.
        let clock = crate::perf::frame_clock_live();
        if !clock {
            help_line(
                ui,
                "No frame clock right now: load a character and wait a moment. The pad poll or \
                 the game's message pump is what measures frames; with neither, nine minutes would \
                 record nothing.",
            );
            vspace(ui, 4.0);
        }
        if clock
            && ui.button(format!(
                "start the {} minute run and close the panel",
                crate::bisect::total_seconds() / 60
            ))
        {
            row.start_run = true;
            row.prompt = false;
        }
        ui.same_line();
        if ui.button("cancel") {
            row.prompt = false;
        }
        vspace(ui, 6.0);
    }

    if let Some(status) = crate::bisect::status() {
        help_line(ui, &status);
        vspace(ui, 6.0);
    }

    vspace(ui, 8.0);
    if crate::bisect::is_running() {
        if ui.button("cancel bundle run") {
            crate::bisect::cancel();
        }
        if ui.is_item_hovered() {
            ui.tooltip_text("stop the run and switch everything back on");
        }
        return row;
    }

    // One button, in the order a report actually needs: the file first, then the issue page, then
    // the file manager opened on that file so it can be dragged into the page. Nothing is uploaded
    // - there is no endpoint - the player attaches it.
    //
    // It counts down the five minutes the report wants of frame data, measured from the character
    // loading. A report that says "300 s sampled" should have 300 seconds in it, so the button is
    // disabled until it does.
    let waiting = crate::perf::remaining_seconds();
    if waiting > 0 {
        ui.disabled(true, || {
            ui.button(format!("diagnostics & report ({}s)", waiting));
        });
        if ui.is_item_hovered() {
            ui.tooltip_text(format!(
                "collecting frame data since the character loaded - ready in {waiting}s"
            ));
        }
    } else if ui.button("diagnostics & report") {
        row.diagnostics = true;
    }
    ui.same_line();
    // The guided run: no five-minute wait, no ini editing, and it switches the subsystems itself so
    // the comparison happens in one sitting rather than four.
    //
    // It needs the hooks installed: the whole point is to switch them off one at a time, and with
    // `hooks = off` there is nothing to switch - nine identical phases and a report that looks like
    // a measurement. Say so instead of offering a button that cannot answer anything.
    if crate::config::load().hooks {
        // Named "extended" because it is: the hook groups one at a time, the combinations, and two
        // closing baselines so drift shows up as drift. It is the debug run for someone reporting
        // a stutter - nine minutes is a lot to ask of a player who just wants to cast.
        if ui.button("extended bundle & report") {
            row.prompt = true;
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "nine one-minute phases: each hook group alone, the combinations, and two closing \
                 baselines. Casting is disabled in two of them on purpose.",
            );
        }
    } else {
        ui.text_disabled("bundle run needs hooks = on (they are already off in the ini)");
    }
    row
}
