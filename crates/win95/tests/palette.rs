//! Every color scheme stays readable.

use egui::Color32;
use win95::palette::{Scheme, contrast};

fn check(name: &str, fg: Color32, bg: Color32, min: f32, problems: &mut Vec<String>) {
    let c = contrast(fg, bg);
    if c < min {
        problems.push(format!("{name}: {c:.2} < {min}"));
    }
}

#[test]
fn every_scheme_is_readable() {
    let mut problems = Vec::new();
    for scheme in Scheme::ALL {
        let p = scheme.palette();
        let n = scheme.name();
        let mut c =
            |what: &str, fg, bg, min| check(&format!("{n} {what}"), fg, bg, min, &mut problems);
        c("text/face", p.text, p.face, 4.5);
        c("window_text/window", p.window_text, p.window, 4.5);
        c("selection", p.selection_text, p.selection, 4.5);
        c("title", p.title_text, p.title, 4.5);
        for (what, bg) in [
            ("added", p.added),
            ("removed", p.removed),
            ("hunk", p.hunk),
            ("note", p.note_bg),
            ("comment", p.comment_bg),
            ("pending", p.pending_bg),
            ("line_selected", p.line_selected),
            ("conflict_block", p.conflict_block),
            ("conflict_current", p.conflict_current),
            ("code", p.code_bg),
            ("ref_head", p.ref_head),
            ("ref_branch", p.ref_branch),
            ("ref_remote", p.ref_remote),
            ("ref_tag", p.ref_tag),
        ] {
            c(&format!("window_text/{what}"), p.window_text, bg, 4.5);
        }
        for (what, fg) in [
            ("link", p.link),
            ("error", p.error),
            ("warning", p.warning),
            ("success", p.success),
            ("merged", p.merged),
        ] {
            c(&format!("{what}/window"), fg, p.window, 3.0);
            c(&format!("{what}/face"), fg, p.face, 3.0);
        }
        for (i, lane) in p.lanes.iter().enumerate() {
            c(&format!("lane {i}/window"), *lane, p.window, 2.5);
        }
        c("gray_text/face", p.gray_text, p.face, 2.0);
        assert_ne!(p.added, p.removed, "{n}: added and removed look the same");
        assert_ne!(p.gray_text, p.text, "{n}: disabled text looks enabled");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn schemes_have_names_and_standard_is_the_classic_look() {
    assert_eq!(Scheme::ALL.len(), 6);
    assert_eq!(Scheme::default(), Scheme::Standard);
    for s in Scheme::ALL {
        assert_eq!(Scheme::from_name(s.name()), Some(s));
    }
    assert_eq!(Scheme::from_name("Nope"), None);
    let p = Scheme::Standard.palette();
    assert_eq!(p.face, Color32::from_rgb(0xC0, 0xC0, 0xC0));
    assert_eq!(p.selection, Color32::from_rgb(0x00, 0x00, 0x80));
    assert!(!p.dark);
    assert!(Scheme::Dark.palette().dark && Scheme::HighContrastBlack.palette().dark);
}

#[test]
fn contrast_follows_wcag() {
    assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
    assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.01);
}
