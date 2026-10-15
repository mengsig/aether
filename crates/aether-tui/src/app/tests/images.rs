//! Images in the composer: a pasted path becomes a chip, the chips leave
//! with the message, and a sent image is a line of the transcript.
use super::*;
use aether_core::attachment::{
    AttachmentId, AttachmentRef, ImageAttachment, MAX_IMAGES_PER_MESSAGE,
};
use aether_core::model::ContentBlock;
use base64::Engine as _;

use crate::commands::ImageArg;
use crate::image_input::ImageSource;
use crate::testing::{app_with_transcript, push_user_content, render_to};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\npixels";

fn png(name: &str) -> ImageAttachment {
    ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(PNG),
        name: Some(name.into()),
    }
}

/// A PNG on disk, and the directory that keeps it alive.
fn png_file(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::write(&path, PNG).expect("write");
    (dir, path)
}

/// An app viewing one agent in `status`.
fn app_viewing(status: AgentStatus) -> (App, AgentId) {
    let mut app = app_with_status(TuiOptions::default());
    app.reduce(AppEvent::Connected);
    let id = AgentId::new();
    let mut viewed = agent(id, "assistant");
    viewed.status = status;
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(viewed))),
    });
    assert_eq!(app.selected, Some(id));
    (app, id)
}

/// The one message among `effects`: who it goes to, its text and its images.
fn sent(effects: &[Effect]) -> (AgentId, String, Vec<ImageAttachment>) {
    let messages: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Request {
                command:
                    Command::SendMessage {
                        to,
                        content,
                        attachments,
                        subtree: false,
                        ..
                    },
                ..
            } => Some((*to, content.clone(), attachments.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 1, "one message sent: {effects:?}");
    messages.into_iter().next().expect("one message")
}

fn sent_none(effects: &[Effect]) -> bool {
    !effects.iter().any(|effect| {
        matches!(
            effect,
            Effect::Request {
                command: Command::SendMessage { .. },
                ..
            }
        )
    })
}

/// Attach `image` the way the clipboard key does: ask, then the read comes back.
fn attach(app: &mut App, image: ImageAttachment) {
    let asked = app.paste_clipboard_image();
    assert_eq!(asked, vec![Effect::AttachImage(ImageSource::Clipboard)]);
    app.reduce(AppEvent::ImageRead(Ok(image)));
}

fn chips(app: &App) -> Vec<String> {
    app.input
        .images
        .iter()
        .map(crate::app::image_label)
        .collect()
}

#[test]
fn a_pasted_image_path_becomes_a_chip_and_leaves_with_the_message() {
    let (_dir, path) = png_file("shot.png");
    let (mut app, agent) = app_viewing(AgentStatus::Ready);

    let pasted = crate::runner::paste_event(format!("'{}'\n", path.display()));
    assert!(
        matches!(pasted, AppEvent::ImagePathPasted { .. }),
        "{pasted:?}"
    );
    assert!(app.reduce(pasted).is_empty());
    assert_eq!(app.input.text, "", "the path is not typed into the draft");
    assert_eq!(chips(&app), ["[image: shot.png, 14 B]"]);
    let screen = render_to(&app, 100, 30);
    assert!(screen.contains("[image: shot.png, 14 B]"), "{screen}");

    app.reduce(AppEvent::Paste("what is this?".into()));
    let effects = app.reduce(key(KeyCode::Enter));
    let (to, text, images) = sent(&effects);
    assert_eq!(to, agent);
    assert_eq!(text, "what is this?");
    assert_eq!(images, vec![png("shot.png")]);
    assert!(
        app.input.images.is_empty(),
        "the chips left with the message"
    );
    assert_eq!(app.input.text, "");
}

#[test]
fn a_paste_that_is_not_an_image_file_stays_text() {
    let (_dir, path) = png_file("shot.png");
    for text in ["look at this", "/home/user/missing.png"] {
        let pasted = crate::runner::paste_event(text.into());
        assert!(matches!(pasted, AppEvent::Paste(_)), "{pasted:?}");
    }

    // A file with an image name and other bytes is pasted as its path, out loud.
    std::fs::write(&path, "not a picture").expect("write");
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    let typed = path.display().to_string();
    app.reduce(crate::runner::paste_event(typed.clone()));
    assert!(app.input.images.is_empty());
    assert_eq!(app.input.text, typed);
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("pasted as text, not attached"), "{flash}");
    assert!(flash.contains("not a supported type"), "{flash}");
}

#[test]
fn a_path_pasted_into_the_find_bar_is_searched_not_attached() {
    let (_dir, path) = png_file("shot.png");
    let mut app = app_with_transcript(&["hello", "hi"]);
    app.reduce(ctrl_key(KeyCode::Char('f')));
    assert!(app.bottom.find.is_some(), "the find bar is open");
    app.reduce(crate::runner::paste_event(path.display().to_string()));
    assert!(app.input.images.is_empty());
    assert_eq!(app.input.text, "");
}

#[test]
fn an_image_needs_no_words_and_steers_a_running_agent() {
    let (mut app, agent) = app_viewing(AgentStatus::Running);
    attach(&mut app, png("shot.png"));
    let effects = app.reduce(key(KeyCode::Enter));
    let (to, text, images) = sent(&effects);
    assert_eq!((to, text.as_str()), (agent, ""));
    assert_eq!(images.len(), 1);
    let steering = &app.transcripts[&agent].steering;
    assert_eq!(steering.len(), 1);
    assert_eq!(steering[0].text, "[image: shot.png, 14 B]");
}

#[test]
fn the_first_message_to_a_new_agent_keeps_its_image() {
    let mut app = app_with_status(TuiOptions::default());
    attach(&mut app, png("shot.png"));
    app.reduce(AppEvent::Paste("what is this?".into()));
    let start = app.reduce(key(KeyCode::Enter));
    assert!(sent_none(&start), "nothing is sent before the agent exists");
    assert!(app.input.images.is_empty());

    let fresh = AgentId::new();
    let effects = app.reduce(AppEvent::Reply {
        then: start_then(&start),
        result: Ok(Reply::Agent(agent(fresh, "assistant"))),
    });
    let (to, text, images) = sent(&effects);
    assert_eq!((to, text.as_str()), (fresh, "what is this?"));
    assert_eq!(images, vec![png("shot.png")]);
}

#[test]
fn chips_come_off_with_backspace_and_with_the_clear_flag() {
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    for name in ["a.png", "b.png", "c.png"] {
        attach(&mut app, png(name));
    }
    assert_eq!(app.input.images.len(), 3);

    // Backspace takes a chip only from the start of the draft.
    app.reduce(key(KeyCode::Char('x')));
    app.reduce(key(KeyCode::Backspace));
    assert_eq!((app.input.text.as_str(), app.input.images.len()), ("", 3));
    app.reduce(key(KeyCode::Backspace));
    assert_eq!(
        chips(&app),
        ["[image: a.png, 14 B]", "[image: b.png, 14 B]"]
    );

    // A command keeps the chips for the message after it; --clear drops them.
    app.reduce(AppEvent::Paste("/help".into()));
    app.reduce(key(KeyCode::Enter));
    assert_eq!(app.input.images.len(), 2);
    app.reduce(AppEvent::Paste("/image --clear".into()));
    assert!(app.reduce(key(KeyCode::Enter)).is_empty());
    assert!(app.input.images.is_empty());
    assert_eq!(app.live_flash(), Some("2 images removed"));
}

#[test]
fn the_image_command_and_its_key_ask_for_a_file_or_the_clipboard() {
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    app.options.home = Some("/home/user".into());
    app.options.cwd = Some("/work".into());
    let asked = |app: &mut App, typed: &str| {
        app.reduce(AppEvent::Paste(typed.into()));
        app.reduce(key(KeyCode::Enter))
    };
    assert_eq!(
        asked(&mut app, "/image ~/Pictures/shot one.png"),
        vec![Effect::AttachImage(ImageSource::File(
            "/home/user/Pictures/shot one.png".into()
        ))]
    );
    assert_eq!(
        asked(&mut app, "/image out/shot.png"),
        vec![Effect::AttachImage(ImageSource::File(
            "/work/out/shot.png".into()
        ))]
    );
    assert_eq!(
        asked(&mut app, "/image"),
        vec![Effect::AttachImage(ImageSource::Clipboard)]
    );
    let alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
    assert_eq!(
        app.reduce(AppEvent::Key(alt_v)),
        vec![Effect::AttachImage(ImageSource::Clipboard)]
    );
    assert_eq!(
        commands::parse("/attach --clear"),
        InputAction::Command(SlashCommand::Image(ImageArg::Clear))
    );

    // What could not be read is one line in the status row, and no chip.
    app.reduce(AppEvent::ImageRead(Err(
        "install wl-clipboard to paste images, or use /image <path>".into(),
    )));
    assert!(app.input.images.is_empty());
    assert_eq!(
        app.live_flash(),
        Some("no image attached: install wl-clipboard to paste images, or use /image <path>")
    );
}

#[test]
fn a_message_takes_no_more_images_than_the_daemon_does() {
    let (mut app, _) = app_viewing(AgentStatus::Ready);
    for n in 0..MAX_IMAGES_PER_MESSAGE {
        attach(&mut app, png(&format!("{n}.png")));
    }
    app.reduce(AppEvent::ImagePathPasted {
        text: "/home/user/one-more.png".into(),
        image: Ok(png("one-more.png")),
    });
    assert_eq!(app.input.images.len(), MAX_IMAGES_PER_MESSAGE);
    assert_eq!(app.input.text, "", "a refused image is not typed either");
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("at most 10 images"), "{flash}");
    assert!(
        app.paste_clipboard_image().is_empty(),
        "nothing is read for a full message"
    );
}

#[test]
fn a_message_waits_for_an_image_that_is_still_being_read() {
    let (mut app, agent) = app_viewing(AgentStatus::Ready);
    assert_eq!(app.paste_clipboard_image().len(), 1);
    app.reduce(AppEvent::Paste("what is this?".into()));
    for send in [KeyCode::Enter, KeyCode::Tab] {
        assert!(app.reduce(key(send)).is_empty(), "sent without its image");
        assert_eq!(app.input.text, "what is this?", "the draft is kept");
        let flash = app.live_flash().unwrap_or_default().to_owned();
        assert!(flash.contains("still being read"), "{flash}");
    }

    app.reduce(AppEvent::ImageRead(Ok(png("shot.png"))));
    assert_eq!(app.live_flash(), Some("attached shot.png, 14 B"));
    let (to, text, images) = sent(&app.reduce(key(KeyCode::Enter)));
    assert_eq!(
        (to, text.as_str(), images.len()),
        (agent, "what is this?", 1)
    );

    // A read that failed is over too: the next message is not held for it.
    assert_eq!(app.paste_clipboard_image().len(), 1);
    app.reduce(AppEvent::ImageRead(Err(
        "the clipboard holds no image".into()
    )));
    app.reduce(AppEvent::Paste("never mind".into()));
    let (_, text, images) = sent(&app.reduce(key(KeyCode::Enter)));
    assert_eq!((text.as_str(), images.len()), ("never mind", 0));
}

#[test]
fn images_are_not_put_in_the_text_only_queue() {
    let (mut app, agent) = app_viewing(AgentStatus::Running);
    attach(&mut app, png("shot.png"));
    app.reduce(AppEvent::Paste("after this turn".into()));
    assert!(app.reduce(key(KeyCode::Tab)).is_empty());
    assert_eq!(app.input.text, "after this turn", "the draft is kept");
    assert_eq!(app.input.images.len(), 1, "and so is its image");
    assert!(
        app.transcripts
            .get(&agent)
            .is_none_or(|t| t.queue.is_empty())
    );
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.contains("not queued"), "{flash}");
}

fn stored(name: &str) -> ContentBlock {
    ContentBlock::ImageRef(AttachmentRef {
        id: AttachmentId::of(PNG),
        media_type: "image/png".into(),
        size_bytes: PNG.len() as u64,
        name: Some(name.into()),
    })
}

/// `standard`, the built-in quiet mode `focus`, and a compact mode of the
/// kind a person adds as `modes/calm.toml`.
#[test]
fn a_sent_image_is_a_line_of_the_transcript_in_every_view_mode() {
    const CALM: &str = r#"
version = 1
label = "Calm"
density = "compact"
show = ["user_text", "assistant_text", "question", "approval", "error"]
"#;
    let prefs = tempfile::tempdir().expect("tempdir");
    let mut app = app_with_transcript(&["hello", "hi"]);
    app.options.prefs_dir = Some(prefs.path().into());
    app.view_modes.modes.insert(
        "calm".into(),
        crate::view_modes::Mode {
            name: "calm".into(),
            source: "fixture".into(),
            definition: crate::view_modes::Definition::parse(CALM).expect("a valid mode"),
        },
    );
    push_user_content(
        &mut app,
        vec![
            ContentBlock::text("what is this?"),
            stored("shot.png"),
            stored("second.png"),
        ],
    );
    // An image sent without words is still a message.
    push_user_content(&mut app, vec![stored("alone.png")]);

    for mode in ["standard", "focus", "calm"] {
        app.set_view_mode(mode);
        assert_eq!(app.view_mode, mode);
        let screen = render_to(&app, 100, 40);
        for line in [
            "what is this?",
            "[image: shot.png, 14 B]",
            "[image: second.png, 14 B]",
            "[image: alone.png, 14 B]",
        ] {
            assert!(screen.contains(line), "{mode} hides {line:?}:\n{screen}");
        }
    }
}
