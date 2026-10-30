use super::*;

const TITLE: &str = "Keyboard shortcuts";

/// The `?` panel at `width`, opened by typing `?` on an empty draft.
fn shortcuts_at(width: u16) -> Vec<Vec<char>> {
    let mut app = App::new(TuiOptions::default());
    type_text(&mut app, "?");
    assert!(app.bottom.shortcuts, "? on an empty draft opens the panel");
    let rows = screen_rows(&app, width, 72);
    assert!(
        rows.iter().any(|row| find_in(row, TITLE).is_some()),
        "no shortcuts panel at {width} columns"
    );
    rows
}

/// The `?` panel is the help (SPEC §8 Phase 4): at every likely width each
/// row of the registry is there whole, under its group's heading, with the
/// group's labels lined up clear of its keys.
#[test]
fn the_shortcuts_panel_lists_every_key_whole_under_its_heading() {
    let ascii = App::new(TuiOptions::default()).theme.ascii();
    for width in 60u16..=200 {
        let rows = shortcuts_at(width);
        assert!(
            rows.iter().all(|row| !row.contains(&'\u{2026}')),
            "at {width} columns the panel cut something"
        );
        for group in KeyGroup::ALL {
            let heading = group.heading();
            let mut found = rows
                .iter()
                .enumerate()
                .filter_map(|(y, row)| find_in(row, heading).map(|x| (y, x)));
            let (y, x) = found
                .next()
                .unwrap_or_else(|| panic!("no {heading:?} heading at {width} columns"));
            assert!(
                found.next().is_none(),
                "{heading:?} twice at {width} columns"
            );
            let specs: Vec<_> = KEYS.iter().filter(|k| k.group == group).collect();
            let mut label_column = None;
            for (offset, spec) in specs.iter().enumerate() {
                let (chord, label_at, rest) = split_columns(&rows[y + 1 + offset][x..]);
                // The next column starts at least two blanks after the label.
                let label = rest.split("  ").next().unwrap_or_default();
                assert_eq!(
                    chord,
                    spec.display_chord(ascii),
                    "at {width} columns {heading:?} lost or reordered {:?}",
                    spec.keys
                );
                assert_eq!(
                    label,
                    spec.display_label(ascii),
                    "at {width} columns {:?} shows the wrong label",
                    spec.keys
                );
                let at = label_at.expect("a label follows its key");
                assert_eq!(
                    *label_column.get_or_insert(at),
                    at,
                    "at {width} columns {heading:?} misaligns {:?}",
                    spec.keys
                );
            }
        }
    }
}

/// Every row of the registry is under exactly one heading, so nothing in
/// `KEYS` can fall out of the help.
#[test]
fn every_key_belongs_to_a_shown_group() {
    let grouped: usize = KeyGroup::ALL
        .into_iter()
        .map(|group| KEYS.iter().filter(|k| k.group == group).count())
        .sum();
    assert_eq!(grouped, KEYS.len());
}

/// The `/` popup is where the commands are explained. Scrolled end to end at
/// every width, each row is whole or says it was cut, and the descriptions
/// line up clear of the names.
#[test]
fn the_palette_fits_its_box_at_every_width() {
    for width in 60u16..=200 {
        let mut app = App::new(TuiOptions::default());
        type_text(&mut app, "/");
        let shown_rows = usize::from(aether_tui::ui::bottom::popup::POPUP_MAX_ROWS);
        // One frame per page: the cursor on each page's last row, then the end.
        let stops: Vec<usize> = (shown_rows - 1..COMMANDS.len())
            .step_by(shown_rows)
            .chain([COMMANDS.len() - 1])
            .collect();
        let mut cursor = 0;
        for stop in stops {
            while cursor < stop {
                press(&mut app, KeyCode::Down, KeyModifiers::NONE);
                cursor += 1;
            }
            let popup = app.bottom.popup.as_ref().expect("the popup stays open");
            assert_eq!(popup.cursor, cursor);
            let top = cursor.saturating_sub(shown_rows - 1);
            let visible = &COMMANDS[top..(top + shown_rows).min(COMMANDS.len())];
            let rows = screen_rows(&app, width, 40);
            let first = format!("/{} ", visible[0].name);
            let (y, x) = rows
                .iter()
                .enumerate()
                .find_map(|(y, row)| find_in(row, &first).map(|x| (y, x)))
                .unwrap_or_else(|| panic!("at {width} columns /{} is not shown", visible[0].name));
            let mut about_column = None;
            for (offset, spec) in visible.iter().enumerate() {
                let (name, about_at, about) = split_columns(&rows[y + offset][x..]);
                let full = format!("/{}", spec.name);
                assert!(
                    whole_or_elided(&name, &full),
                    "at {width} columns the palette cut {full:?} without saying so: {name:?}"
                );
                assert!(
                    whole_or_elided(&about, spec.about),
                    "at {width} columns the palette cut {:?} without saying so: {about:?}",
                    spec.about
                );
                let at = about_at.expect("a description follows its name");
                assert_eq!(
                    *about_column.get_or_insert(at),
                    at,
                    "at {width} columns {full:?} misaligns what it does"
                );
            }
        }
    }
}
