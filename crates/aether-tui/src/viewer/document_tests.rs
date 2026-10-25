#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{
    app::{AppEvent, Effect},
    testing::{app_with_transcript, render_with},
    ui::transcript::ViewCache,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use tools::Tools;

fn key(app: &mut crate::App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
    app.reduce(AppEvent::Key(KeyEvent::new(code, modifiers)))
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    crate::voice::test_script::write(&path, &format!("#!/bin/sh\n{body}\n"));
    path
}

/// Stand-ins for poppler that log their argv and answer with fixed output.
fn fake_poppler(dir: &Path, renderer: bool) -> Tools {
    let calls = dir.join("calls.log").display().to_string();
    let png = dir.join("page-fixture.png");
    image::RgbaImage::from_pixel(30, 40, image::Rgba([20, 60, 200, 255]))
        .save(&png)
        .unwrap();
    Tools {
        pdfinfo: Some(script(
            dir,
            "pdfinfo",
            "printf 'Title: x\\nPages:          3\\n'",
        )),
        pdftotext: Some(script(
            dir,
            "pdftotext",
            "printf 'Text of page %s\\n' \"$2\"",
        )),
        pdftoppm: renderer.then(|| {
            let body = format!(
                "echo \"$@\" >> '{calls}'\nfor last; do :; done\ncp '{}' \"$last.png\"",
                png.display()
            );
            script(dir, "pdftoppm", &body)
        }),
        ..Tools::default()
    }
}

fn setup(files: &[(&str, &[u8])]) -> (tempfile::TempDir, crate::App) {
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in files {
        std::fs::write(dir.path().join(name), bytes).unwrap();
    }
    let mut app = app_with_transcript(&[]);
    app.options.cwd = Some(dir.path().to_owned());
    app.size = (120, 36);
    app.board.open = false;
    app.approvals.clear();
    (dir, app)
}

fn open(app: &mut crate::App, documents: &mut Documents, target: &str) {
    let effects = app.open_reference(references::Reference {
        target: target.into(),
        label: target.into(),
    });
    load(app, documents, effects);
}

fn load(app: &mut crate::App, documents: &mut Documents, effects: Vec<Effect>) {
    let [Effect::PreviewFile(request)] = &effects[..] else {
        panic!("expected one preview effect, got {effects:?}")
    };
    let result = read(request, documents).map_err(|e| format!("{e:#}"));
    app.reduce(AppEvent::PreviewReady {
        id: request.id,
        result,
    });
}

fn pane(app: &crate::App) -> String {
    let screen = render_with(app, &mut ViewCache::default(), app.size.0, app.size.1);
    let start = usize::from(app.size.0 - app.size.0 * app.viewer.share / 100);
    screen
        .lines()
        .map(|row| {
            row.chars()
                .skip(start)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_snapshot(name: &str, got: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/viewer/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("AETHER_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(got, want, "snapshot {name} differs:\n{got}");
}

#[test]
fn file_types_choose_their_viewer_by_extension() {
    let pdf = fixtures::pdf(&["One"]);
    let (_dir, mut app) = setup(&[
        ("report.PDF", &pdf),
        ("page.HTM", fixtures::HTML.as_bytes()),
        ("page.html", fixtures::HTML.as_bytes()),
        ("logo.svg", b"<svg/>"),
    ]);
    let mut documents = Documents::new(Tools::default());
    open(&mut app, &mut documents, "report.PDF");
    assert!(
        matches!(&app.viewer.preview, Some(Ok(Preview::Failed { error, .. })) if error.contains("poppler-utils")),
        "{:?}",
        app.viewer.preview
    );
    for page in ["page.HTM", "page.html"] {
        open(&mut app, &mut documents, page);
        assert!(matches!(
            &app.viewer.preview,
            Some(Ok(Preview::Text { pages: None, .. }))
        ));
    }
    open(&mut app, &mut documents, "logo.svg");
    assert!(matches!(
        &app.viewer.preview,
        Some(Ok(Preview::Unsupported(_)))
    ));
}

#[test]
fn each_partial_pdf_toolset_shows_what_it_can() {
    let pdf = fixtures::pdf(&["One", "Two"]);
    let (_dir, mut app) = setup(&[("report.pdf", &pdf)]);
    let bin = tempfile::tempdir().unwrap();
    let text = script(bin.path(), "pdftotext", "printf 'Whole document text\\n'");
    let render = script(bin.path(), "pdftoppm", "exit 1");
    let info = script(bin.path(), "pdfinfo", "printf 'Pages: 2\\n'");
    let mut documents = Documents::new(Tools {
        pdftotext: Some(text),
        ..Tools::default()
    });
    open(&mut app, &mut documents, "report.pdf");
    let shown = pane(&app);
    assert!(shown.contains("Whole document text"), "{shown}");
    assert!(shown.contains("showing the document text"), "{shown}");
    assert_eq!(app.viewer.pages(), None);
    let failures = [
        (Some(render), None, "pdftoppm needs pdfinfo"),
        (None, Some(info), "No PDF tools found"),
    ];
    for (pdftoppm, pdfinfo, reason) in failures {
        let mut documents = Documents::new(Tools {
            pdftoppm,
            pdfinfo,
            ..Tools::default()
        });
        open(&mut app, &mut documents, "report.pdf");
        assert!(
            matches!(&app.viewer.preview, Some(Ok(Preview::Failed { error, .. })) if error.contains(reason)),
            "{reason}: {:?}",
            app.viewer.preview
        );
    }
}

#[test]
fn page_counts_parse_from_poppler_and_mupdf_and_reject_nonsense() {
    assert_eq!(
        pdf::parse_page_count("Title: x\nPages:          12\n").unwrap(),
        12
    );
    assert_eq!(pdf::parse_page_count("PDF-1.4\nPages: 3\n").unwrap(), 3);
    assert!(pdf::parse_page_count("Pages: 0").is_err());
    assert!(pdf::parse_page_count("Pages: many").is_err());
    assert!(pdf::parse_page_count("Pages: 100001").is_err());
    assert!(pdf::parse_page_count("Title: none").is_err());
}

#[test]
fn render_commands_are_argv_for_poppler_or_mupdf() {
    let document = Path::new("/work/document.pdf");
    let output = Path::new("/work/page.png");
    let args = |tools: &Tools| -> Vec<String> {
        let command = pdf::render_command(tools, document, output, 4, 900).unwrap();
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    };
    let poppler = Tools {
        pdftoppm: Some("/bin/pdftoppm".into()),
        mutool: Some("/bin/mutool".into()),
        ..Tools::default()
    };
    assert_eq!(
        args(&poppler),
        [
            "/bin/pdftoppm",
            "-png",
            "-singlefile",
            "-f",
            "4",
            "-l",
            "4",
            "-scale-to",
            "900",
            "/work/document.pdf",
            "/work/page"
        ]
    );
    let mupdf = Tools {
        mutool: Some("/bin/mutool".into()),
        ..Tools::default()
    };
    assert_eq!(
        args(&mupdf),
        [
            "/bin/mutool",
            "draw",
            "-q",
            "-F",
            "png",
            "-w",
            "900",
            "-h",
            "900",
            "-o",
            "/work/page.png",
            "/work/document.pdf",
            "4"
        ]
    );
    assert!(pdf::render_command(&Tools::default(), document, output, 1, 900).is_err());
}

#[test]
fn pages_turn_and_clamp_and_rendered_pages_are_cached() {
    let pdf = fixtures::pdf(&["One", "Two", "Three"]);
    let (dir, mut app) = setup(&[("report.pdf", &pdf)]);
    let tools_dir = tempfile::tempdir().unwrap();
    let mut documents = Documents::new(fake_poppler(tools_dir.path(), true));
    open(&mut app, &mut documents, "report.pdf");
    assert_eq!(
        app.viewer.pages(),
        Some(Pages {
            current: 1,
            total: 3
        })
    );
    assert!(
        matches!(&app.viewer.preview, Some(Ok(Preview::Image { rows, .. })) if !rows.is_empty())
    );
    assert!(
        pane(&app).contains("report.pdf · page 1/3"),
        "{}",
        pane(&app)
    );
    assert!(key(&mut app, KeyCode::PageUp, KeyModifiers::NONE).is_empty());
    for expected in [2, 3] {
        let effects = key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        load(&mut app, &mut documents, effects);
        assert_eq!(app.viewer.pages().unwrap().current, expected);
    }
    let effects = key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    assert!(effects.is_empty(), "the last page stays put");
    let effects = key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    load(&mut app, &mut documents, effects);
    let calls = std::fs::read_to_string(tools_dir.path().join("calls.log")).unwrap();
    assert_eq!(
        calls.lines().count(),
        3,
        "page 2 came from the cache: {calls}"
    );
    assert!(calls.lines().all(|l| l.starts_with("-png -singlefile -f ")));
    assert!(
        !calls.contains(&dir.path().display().to_string()),
        "tools read a private copy"
    );
    let mut request = app.viewer.pending.clone().unwrap();
    request.page = 99;
    let shown = read(&request, &mut documents).unwrap();
    assert!(matches!(
        shown,
        Preview::Image {
            pages: Some(Pages {
                current: 3,
                total: 3
            }),
            ..
        }
    ));
}

#[test]
fn alt_t_switches_a_pdf_page_to_text_and_back() {
    let pdf = fixtures::pdf(&["One", "Two"]);
    let (_dir, mut app) = setup(&[("report.pdf", &pdf)]);
    let tools_dir = tempfile::tempdir().unwrap();
    let mut documents = Documents::new(fake_poppler(tools_dir.path(), true));
    open(&mut app, &mut documents, "report.pdf");
    let effects = key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    load(&mut app, &mut documents, effects);
    let effects = key(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    load(&mut app, &mut documents, effects);
    assert!(pane(&app).contains("Text of page 2"), "{}", pane(&app));
    assert_eq!(app.viewer.pages().unwrap().current, 2);
    let effects = key(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    load(&mut app, &mut documents, effects);
    assert!(matches!(
        &app.viewer.preview,
        Some(Ok(Preview::Image { .. }))
    ));
}

#[test]
fn pdf_fallbacks_match_their_snapshots() {
    let pdf = fixtures::pdf(&["One", "Two"]);
    let (_dir, mut app) = setup(&[("report.pdf", &pdf)]);
    let tools_dir = tempfile::tempdir().unwrap();
    let mut documents = Documents::new(fake_poppler(tools_dir.path(), false));
    open(&mut app, &mut documents, "report.pdf");
    assert_snapshot("pdf_text_without_renderer", &pane(&app));
    let mut documents = Documents::new(Tools::default());
    open(&mut app, &mut documents, "report.pdf");
    assert_snapshot("pdf_without_tools", &pane(&app));
}

#[test]
fn html_renders_readable_text_without_scripts_or_styles() {
    let (_dir, mut app) = setup(&[("page.html", fixtures::HTML.as_bytes())]);
    let mut documents = Documents::new(Tools::default());
    open(&mut app, &mut documents, "page.html");
    let shown = pane(&app);
    for hidden in ["SECRET", "Hidden title", "color: red"] {
        assert!(!shown.contains(hidden), "{hidden} leaked:\n{shown}");
    }
    assert_snapshot("html_text", &shown);
    let effects = key(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    load(&mut app, &mut documents, effects);
    assert_snapshot("html_screenshot_without_chromium", &pane(&app));
}

#[test]
fn html_text_styles_links_headings_and_strips_terminal_controls() {
    let theme = crate::ui::theme::Theme::resolve(Default::default());
    let html = b"<h2>Title</h2><p><a href=\"https://example.com/a\">long link text</a> \
        and <a href=\"https://example.com/a\">again</a> \x1b]52;c;bad\x07</p>";
    let lines = html::text_lines(html, 12, &theme).unwrap();
    let text: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(lines[0].style, theme.heading());
    assert!(
        text.iter()
            .all(|row| !row.contains('\x1b') && !row.contains('\x07'))
    );
    assert_eq!(
        text.iter().filter(|row| row.contains("[1]")).count(),
        3,
        "{text:?}"
    );
    assert!(
        text.iter().any(|row| row == "[1] https://example.com/a"),
        "{text:?}"
    );
    assert!(!text.iter().any(|row| row.contains("[2]")));
}

#[test]
fn chromium_runs_offline_on_a_sandboxed_copy() {
    let args = html::chromium_args(
        Path::new("/tmp/shot dir"),
        Path::new("/tmp/shot dir/page.html"),
        Path::new("/tmp/shot dir/shot.png"),
        (800, 600),
    );
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for flag in [
        "--headless",
        "--proxy-server=127.0.0.1:9",
        "--proxy-bypass-list=<-loopback>",
        "--host-resolver-rules=MAP * ~NOTFOUND",
        "--user-data-dir=/tmp/shot dir/profile",
        "--window-size=800,600",
        "--screenshot=/tmp/shot dir/shot.png",
        "--disable-features=IsolateSandboxedIframes",
    ] {
        assert!(args.iter().any(|a| a == flag), "missing {flag}: {args:?}");
    }
    assert!(!args.iter().any(|a| a.contains("no-sandbox")));
    assert_eq!(args.last().unwrap(), "file:///tmp/shot%20dir/page.html");
    let page = html::isolated_page(b"<p title=\"x\">a & b</p><script>x()</script>");
    assert!(page.contains("<iframe sandbox srcdoc=\"&lt;p title=&quot;x&quot;&gt;a &amp; b"));
    assert!(page.contains("Content-Security-Policy\" content=\"default-src 'none';"));
    assert!(!page.contains("<script>"));
}

/// A skipped isolation test fails in CI when CI must run it, and is loud anywhere else.
fn skip_isolation_test(reason: &str, required_in_ci: bool) {
    assert!(
        !(required_in_ci && std::env::var_os("CI").is_some()),
        "CI must run this test: {reason}"
    );
    // Written past libtest's capture, which would hide an eprintln! of a passing test.
    writeln!(
        std::io::stderr(),
        "\n*** SKIPPED the Chromium isolation test: {reason} ***\n"
    )
    .unwrap();
}

/// Every local-file and network route the page tries must stay blank in the screenshot.
#[test]
fn chromium_screenshot_cannot_read_local_files_or_reach_the_network() {
    let Some(chromium) = Tools::detect().chromium else {
        // The Linux runner image ships Chromium; macOS keeps Chrome off PATH.
        skip_isolation_test(
            "headless Chromium is not installed",
            cfg!(target_os = "linux"),
        );
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 0, 0, 255]))
        .save(root.join("secret.png"))
        .unwrap();
    std::fs::write(
        root.join("secret.html"),
        "<body style=background:#f00>x</body>",
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let secret = root.display();
    let page = format!(
        "<html><head><meta http-equiv=refresh content=\"0;url=file://{secret}/secret.html\">\
         <link rel=stylesheet href=\"http://127.0.0.1:{port}/a.css\"></head>\
         <body style=\"background:#fff\"><h1>Visible heading</h1>\
         <img src=\"file://{secret}/secret.png\" width=200 height=200>\
         <img src=\"http://127.0.0.1:{port}/b.png\">\
         <iframe src=\"file://{secret}/secret.html\"></iframe>\
         <div style=\"width:200px;height:200px;background:url(file://{secret}/secret.png)\"></div>\
         <script>document.body.style.background='#f00'</script></body></html>"
    );
    std::fs::write(root.join("page.html"), &page).unwrap();
    let png = match html::screenshot(&chromium, page.as_bytes(), (800, 600)) {
        Err(error) if error.to_string() == html::SANDBOX_UNAVAILABLE => {
            skip_isolation_test(&error.to_string(), true);
            return;
        }
        result => result.unwrap(),
    };
    let shot = image::load_from_memory(&png).unwrap().into_rgb8();
    let red = shot
        .pixels()
        .filter(|p| p[0] > 200 && p[1] < 80 && p[2] < 80)
        .count();
    let dark = shot
        .pixels()
        .filter(|p| p.0.iter().map(|&c| u32::from(c)).sum::<u32>() < 150);
    assert_eq!(
        red, 0,
        "a local file, script or refresh reached the screenshot"
    );
    assert!(dark.count() > 50, "the page itself must render");
    assert!(listener.accept().is_err(), "the page reached the network");
    let (_workspace, mut app) = setup(&[]);
    app.options.cwd = Some(root);
    let mut documents = Documents::new(Tools {
        chromium: Some(chromium),
        ..Tools::default()
    });
    open(&mut app, &mut documents, "page.html");
    let effects = key(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    load(&mut app, &mut documents, effects);
    assert!(matches!(
        &app.viewer.preview,
        Some(Ok(Preview::Image { .. }))
    ));
}

#[test]
fn a_chromium_without_a_usable_sandbox_says_so_instead_of_dumping_its_crash() {
    let (dir, mut app) = setup(&[("page.html", b"<h1>Heading</h1>")]);
    let chromium = script(
        dir.path(),
        "chromium",
        "echo '[1:1:FATAL:zygote_host_impl_linux.cc:130] No usable sandbox! If you are running \
         on Ubuntu 23.10+ ...' >&2\necho 'base::debug::CollectStackTrace()' >&2\nexit 134",
    );
    let mut documents = Documents::new(Tools {
        chromium: Some(chromium),
        ..Tools::default()
    });
    open(&mut app, &mut documents, "page.html");
    let effects = key(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    load(&mut app, &mut documents, effects);
    let Some(Ok(Preview::Failed { error, .. })) = &app.viewer.preview else {
        panic!("{:?}", app.viewer.preview)
    };
    assert_eq!(error, html::SANDBOX_UNAVAILABLE);
    assert!(pane(&app).contains("Chromium's sandbox could not start"));
    let other = script(dir.path(), "chromium-other", "echo 'bad flag' >&2\nexit 1");
    let error = html::screenshot(&other, b"<p>x</p>", (800, 600)).unwrap_err();
    assert!(
        format!("{error:#}").contains("chromium-other failed"),
        "{error:#}"
    );
    assert!(format!("{error:#}").contains("bad flag"), "{error:#}");
}

#[test]
fn installed_poppler_renders_pages_and_extracts_their_text() {
    let tools = Tools::detect();
    if tools.pdftoppm.is_none() || tools.pdfinfo.is_none() || tools.pdftotext.is_none() {
        eprintln!("skipped: poppler-utils is not installed");
        return;
    }
    let pdf = fixtures::pdf(&["First page", "Second page"]);
    let (_dir, mut app) = setup(&[("report.pdf", &pdf)]);
    let mut documents = Documents::new(Tools {
        mutool: None,
        chromium: None,
        ..tools
    });
    open(&mut app, &mut documents, "report.pdf");
    assert_eq!(
        app.viewer.pages(),
        Some(Pages {
            current: 1,
            total: 2
        })
    );
    let Some(Ok(Preview::Image { rows, .. })) = &app.viewer.preview else {
        panic!("{:?}", app.viewer.preview)
    };
    assert!(rows.len() > 5, "the page fills the pane");
    let effects = key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    load(&mut app, &mut documents, effects);
    let effects = key(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    load(&mut app, &mut documents, effects);
    assert!(pane(&app).contains("Second page"), "{}", pane(&app));
}

#[test]
fn ordinary_html_parses_and_its_depth_is_measured() {
    let fine = "<div>".repeat(500) + "fine" + &"</div>".repeat(500);
    let dom = html_dom::parse(fine.as_bytes(), html_dom::PARSE_TIMEOUT).unwrap();
    // document > html > body > 500 divs > text
    assert_eq!(html_dom::depth(&dom.document), 503);
    let dom = html_dom::parse(fixtures::HTML.as_bytes(), html_dom::PARSE_TIMEOUT).unwrap();
    assert!((4..20).contains(&html_dom::depth(&dom.document)));
}

#[test]
fn hostile_html_fails_fast_with_a_visible_reason() {
    let quick = std::time::Duration::from_millis(500);
    let unmatched = "<div></bogus>".repeat(100_000) + "hello";
    let deep = "<div>".repeat(600) + "deep";
    let mut formatting = String::from("<div>");
    for id in 0..13 {
        for name in [
            "b", "i", "u", "em", "strong", "s", "small", "big", "code", "font", "tt",
        ] {
            formatting.push_str(&format!("<{name} id={id}>"));
        }
    }
    formatting.push_str("</div>");
    formatting.push_str(&"<p>x".repeat(20_000));
    let cases = [
        (unmatched.as_str(), "Alt+E opens it in a browser"),
        (deep.as_str(), "deeper than 512"),
        (formatting.as_str(), "more than 200000 elements"),
    ];
    for (html, reason) in cases {
        let started = std::time::Instant::now();
        let Err(error) = html_dom::parse(html.as_bytes(), quick) else {
            panic!("{reason}: parsed")
        };
        assert!(format!("{error:#}").contains(reason), "{error:#}");
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }
}
#[test]
fn deeply_nested_or_oversized_html_fails_visibly_and_quickly() {
    let nested = "<div>".repeat(100_000) + "deep" + &"</div>".repeat(100_000);
    let (_dir, mut app) = setup(&[("deep.html", nested.as_bytes())]);
    let started = std::time::Instant::now();
    open(&mut app, &mut Documents::new(Tools::default()), "deep.html");
    assert!(started.elapsed() < html_dom::PARSE_TIMEOUT + std::time::Duration::from_secs(2));
    assert!(
        matches!(&app.viewer.preview, Some(Ok(Preview::Failed { error, .. })) if error.contains("Alt+E opens it in a browser")),
        "{:?}",
        app.viewer.preview
    );
    let fine = "<div>".repeat(500) + "fine" + &"</div>".repeat(500);
    assert!(html::text_lines(fine.as_bytes(), 60, &app.theme).is_ok());
    let big = vec![b'a'; 5 * 1024 * 1024 + 1];
    let (_dir, mut app) = setup(&[("big.html", &big)]);
    open(&mut app, &mut Documents::new(Tools::default()), "big.html");
    assert!(
        matches!(&app.viewer.preview, Some(Ok(Preview::Failed { error, .. })) if error.contains("5 MiB")),
        "{:?}",
        app.viewer.preview
    );
}
