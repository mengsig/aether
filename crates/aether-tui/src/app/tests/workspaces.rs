//! Workspaces: where the TUI starts agents, which root it views on launch,
//! and switching with `/workspace` and Ctrl+O.
use std::path::Path;

use super::*;
use crate::ui::views::ViewKind;

/// A root working in `path`, last updated `age` seconds ago.
fn root_in(name: &str, path: &Path, age: i64) -> Agent {
    let mut agent = agent(AgentId::new(), name);
    agent.scope.workspace_root = Some(path.to_path_buf());
    agent.updated_at = now() - chrono::Duration::seconds(age);
    agent
}

/// A tree of `agents`; parents must come before their children.
fn tree_of(agents: Vec<Agent>) -> AgentTree {
    let mut tree = AgentTree::default();
    for agent in agents {
        let id = agent.id;
        match agent.parent {
            Some(parent) => tree
                .nodes
                .get_mut(&parent)
                .expect("parent first")
                .children
                .push(id),
            None => tree.roots.push(id),
        }
        tree.nodes.insert(
            id,
            TreeNode {
                agent,
                children: vec![],
                task: None,
                waiting_on: vec![],
                lease: None,
                subtree_usage: Usage::default(),
                hidden_usage: None,
            },
        );
    }
    tree
}

fn launched_in(dir: &Path) -> App {
    launched_with(TuiOptions {
        cwd: Some(dir.to_path_buf()),
        auto_create_root: true,
        ..Default::default()
    })
}

fn launched_with(options: TuiOptions) -> App {
    let mut app = app_with_status(options);
    app.reduce(AppEvent::Connected);
    app.status = Some(crate::ui::tests_support::quiet_status());
    app
}

fn apply(app: &mut App, tree: AgentTree) -> Vec<Effect> {
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree)),
    })
}

fn created(effects: &[Effect]) -> Vec<&CreateAgent> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Request {
                command: Command::CreateAgent(req),
                ..
            } => Some(req),
            _ => None,
        })
        .collect()
}

fn workspace_of(req: &CreateAgent) -> Option<&Path> {
    req.scope.as_ref()?.workspace_root.as_deref()
}

/// The daemon's answer to the create in `effects`: the agent it made, with that scope.
fn create_reply(app: &mut App, effects: &[Effect]) -> (AgentId, Vec<Effect>) {
    let req = created(effects)[0];
    let mut made = agent(AgentId::new(), "assistant");
    made.scope = req.scope.clone().unwrap_or_default();
    let id = made.id;
    let effects = app.reduce(AppEvent::Reply {
        then: start_then(effects),
        result: Ok(Reply::Agent(made)),
    });
    (id, effects)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.reduce(key(KeyCode::Char(c)));
    }
}

fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let one = tmp.path().join("one");
    let two = tmp.path().join("two");
    std::fs::create_dir_all(&one).expect("mkdir");
    std::fs::create_dir_all(&two).expect("mkdir");
    let one = crate::workspace::canonical(&one);
    let two = crate::workspace::canonical(&two);
    (tmp, one, two)
}

/// (a) A live root in the launch directory is viewed, the newest of them,
/// even when a root elsewhere is newer still; nothing is created.
#[test]
fn launch_views_the_latest_live_root_already_in_the_launch_directory() {
    let (_tmp, here, elsewhere) = dirs();
    let older = root_in("older", &here, 300);
    let newer = root_in("newer", &here, 60);
    let other = root_in("other", &elsewhere, 1);
    let want = newer.id;
    let mut app = launched_in(&here);
    let effects = apply(&mut app, tree_of(vec![older, newer, other]));
    assert_eq!(app.selected, Some(want));
    assert!(created(&effects).is_empty(), "{effects:?}");
    assert_eq!(app.active_workspace(), Some(here.as_path()));
}

/// (b) Live roots only in other projects: start one here, keep theirs on the board.
#[test]
fn launch_elsewhere_starts_a_root_in_the_launch_directory() {
    let (_tmp, here, elsewhere) = dirs();
    let other = root_in("other", &elsewhere, 1);
    let other_id = other.id;
    let mut app = launched_in(&here);
    let start = apply(&mut app, tree_of(vec![other]));
    let made = created(&start);
    assert_eq!(made.len(), 1, "{start:?}");
    assert_eq!(workspace_of(made[0]), Some(here.as_path()));
    assert_eq!(app.selected, None, "not dropped into the other project");
    assert!(app.board.model.index_of(other_id).is_some());

    // A tree arriving before the new root does not pick the other project either.
    let mut again = root_in("other", &elsewhere, 1);
    again.id = other_id;
    let effects = apply(&mut app, tree_of(vec![again]));
    assert!(created(&effects).is_empty());
    assert_eq!(app.selected, None);

    let (id, _) = create_reply(&mut app, &start);
    assert_eq!(app.selected, Some(id));
}

/// (c) No live agent at all: start one in the launch directory, as before.
#[test]
fn launch_with_nothing_live_starts_a_root_here() {
    let (_tmp, here, elsewhere) = dirs();
    let mut done = root_in("done", &elsewhere, 1);
    done.status = AgentStatus::Completed { summary: None };
    let mut app = launched_in(&here);
    let effects = apply(&mut app, tree_of(vec![done]));
    let made = created(&effects);
    assert_eq!(made.len(), 1);
    assert_eq!(workspace_of(made[0]), Some(here.as_path()));
}

/// (d) `--agent` wins wherever that agent works.
#[test]
fn a_focused_agent_wins_regardless_of_workspace() {
    let (_tmp, here, elsewhere) = dirs();
    let other = root_in("other", &elsewhere, 1);
    let focus = other.id;
    let mut app = launched_with(TuiOptions {
        cwd: Some(here.clone()),
        auto_create_root: true,
        focus: Some(focus),
        ..Default::default()
    });
    let effects = apply(&mut app, tree_of(vec![other]));
    assert_eq!(app.selected, Some(focus));
    assert!(created(&effects).is_empty(), "{effects:?}");
    assert_eq!(app.active_workspace(), Some(elsewhere.as_path()));
}

/// (e) Viewing an agent in another workspace: later trees create nothing,
/// and `/new` starts the next agent beside the viewed one.
#[test]
fn viewing_another_workspace_moves_where_new_agents_start() {
    let (_tmp, here, elsewhere) = dirs();
    let mine = root_in("mine", &here, 10);
    let theirs = root_in("theirs", &elsewhere, 5);
    let (mine_id, theirs_id) = (mine.id, theirs.id);
    let tree = tree_of(vec![mine, theirs]);
    let mut app = launched_in(&here);
    apply(&mut app, tree.clone());
    assert_eq!(app.selected, Some(mine_id));
    app.select(theirs_id);
    assert_eq!(app.active_workspace(), Some(elsewhere.as_path()));
    let effects = apply(&mut app, tree);
    assert!(created(&effects).is_empty(), "{effects:?}");
    assert_eq!(app.selected, Some(theirs_id));

    type_text(&mut app, "/new");
    let effects = app.reduce(key(KeyCode::Enter));
    let made = created(&effects);
    assert_eq!(made.len(), 1);
    assert_eq!(workspace_of(made[0]), Some(elsewhere.as_path()));
}

/// (f) Everything finished: the root that replaces it works where the
/// operator is now, not where the TUI was launched.
#[test]
fn a_root_replacing_finished_work_starts_in_the_active_workspace() {
    let (_tmp, here, elsewhere) = dirs();
    let mine = root_in("mine", &here, 10);
    let theirs = root_in("theirs", &elsewhere, 5);
    let theirs_id = theirs.id;
    let mut app = launched_in(&here);
    apply(&mut app, tree_of(vec![mine.clone(), theirs.clone()]));
    app.select(theirs_id);
    let finished: Vec<Agent> = [mine, theirs]
        .into_iter()
        .map(|mut a| {
            a.status = AgentStatus::Completed { summary: None };
            a
        })
        .collect();
    let effects = apply(&mut app, tree_of(finished));
    let made = created(&effects);
    assert_eq!(made.len(), 1, "{effects:?}");
    assert_eq!(workspace_of(made[0]), Some(elsewhere.as_path()));
}

/// The active workspace follows the viewed agent's tree root, so a child in
/// an isolated worktree still counts as its root's project.
#[test]
fn the_active_workspace_is_the_viewed_trees_root() {
    let (tmp, here, elsewhere) = dirs();
    let root = root_in("root", &elsewhere, 5);
    let mut child = root_in("child", &tmp.path().join("wt/workspace"), 1);
    child.parent = Some(root.id);
    let child_id = child.id;
    let mut app = launched_in(&here);
    apply(&mut app, tree_of(vec![root, child]));
    app.select(child_id);
    assert_eq!(app.active_workspace(), Some(elsewhere.as_path()));
}

#[test]
fn ctrl_o_toggles_the_workspace_picker_and_slash_workspace_opens_it() {
    let (_tmp, here, _) = dirs();
    let mut app = launched_in(&here);
    app.reduce(ctrl_key(KeyCode::Char('o')));
    assert!(app.bottom.showing(ViewKind::Workspace));
    app.reduce(ctrl_key(KeyCode::Char('o')));
    assert!(!app.bottom.showing(ViewKind::Workspace));
    for alias in ["/workspace", "/cd", "/ws", "/workspaces"] {
        type_text(&mut app, alias);
        app.reduce(key(KeyCode::Enter));
        assert!(app.bottom.showing(ViewKind::Workspace), "{alias}");
        app.reduce(key(KeyCode::Esc));
        assert!(!app.bottom.showing(ViewKind::Workspace), "{alias}");
    }
}

/// Enter on a workspace with a live root views it; on one without, starts
/// a root there, and a message typed meanwhile waits for that root.
#[test]
fn choosing_a_workspace_views_its_root_or_starts_one() {
    let (tmp, here, elsewhere) = dirs();
    let prefs = tmp.path().join("prefs");
    let mine = root_in("mine", &here, 10);
    let theirs = root_in("theirs", &elsewhere, 5);
    let theirs_id = theirs.id;
    let mut app = launched_with(TuiOptions {
        cwd: Some(here.clone()),
        auto_create_root: true,
        prefs_dir: Some(prefs.clone()),
        ..Default::default()
    });
    apply(&mut app, tree_of(vec![mine, theirs]));

    app.reduce(ctrl_key(KeyCode::Char('o')));
    type_text(&mut app, &elsewhere.display().to_string());
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(!app.bottom.showing(ViewKind::Workspace));
    assert!(created(&effects).is_empty(), "{effects:?}");
    assert_eq!(app.selected, Some(theirs_id));
    assert!(
        app.live_flash()
            .is_some_and(|f| f.starts_with("working in"))
    );

    let fresh = crate::workspace::canonical(&{
        let dir = tmp.path().join("fresh");
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    });
    app.reduce(ctrl_key(KeyCode::Char('o')));
    type_text(&mut app, &fresh.display().to_string());
    let start = app.reduce(key(KeyCode::Enter));
    let made = created(&start);
    assert_eq!(made.len(), 1, "{start:?}");
    assert_eq!(workspace_of(made[0]), Some(fresh.as_path()));
    assert_eq!(app.selected, None);
    assert!(
        app.live_flash()
            .is_some_and(|f| f.starts_with("new agent in"))
    );
    assert_eq!(app.active_workspace(), Some(fresh.as_path()));

    type_text(&mut app, "hello there");
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(created(&effects).is_empty(), "one root per pick");
    let (id, effects) = create_reply(&mut app, &start);
    assert_eq!(app.selected, Some(id));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request { command: Command::SendMessage { to, content, .. }, .. }
            if *to == id && content == "hello there"
    )));

    let saved = crate::prefs::UiPrefs::load(&prefs).expect("prefs");
    assert_eq!(
        saved.recent_workspaces,
        [fresh.display().to_string(), elsewhere.display().to_string()]
    );
    assert_eq!(app.recent_workspaces, saved.recent_workspaces);
}

#[test]
fn slash_workspace_with_a_path_switches_directly_and_refuses_non_directories() {
    let (tmp, here, _) = dirs();
    let home = crate::workspace::canonical(tmp.path());
    let mut app = launched_with(TuiOptions {
        cwd: Some(here.clone()),
        home: Some(home.clone()),
        auto_create_root: true,
        ..Default::default()
    });
    apply(&mut app, tree_of(vec![root_in("mine", &here, 1)]));

    type_text(&mut app, "/workspace ~/two");
    let effects = app.reduce(key(KeyCode::Enter));
    let made = created(&effects);
    assert_eq!(made.len(), 1, "{effects:?}");
    assert_eq!(workspace_of(made[0]), Some(home.join("two").as_path()));
    assert!(
        !app.bottom.showing(ViewKind::Workspace),
        "no picker for a path"
    );

    for bad in ["/cd ~/missing", "/cd ~nobody/x"] {
        type_text(&mut app, bad);
        let effects = app.reduce(key(KeyCode::Enter));
        assert!(effects.is_empty(), "{bad}: {effects:?}");
        assert!(
            app.live_flash()
                .is_some_and(|f| f.starts_with("no such directory")),
            "{bad}: {:?}",
            app.live_flash()
        );
    }
    // Relative to the workspace in use: `two` holds the root being started.
    type_text(&mut app, "/cd ../one");
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(created(&effects).is_empty(), "one is viewed, not recreated");
    assert_eq!(app.active_workspace(), Some(here.as_path()));
}

/// A recent that cannot be saved is said, not swallowed; the switch still happens.
#[test]
fn a_failed_recent_save_is_reported() {
    let (tmp, here, elsewhere) = dirs();
    let not_a_dir = tmp.path().join("file");
    std::fs::write(&not_a_dir, "").expect("write");
    let mut app = launched_with(TuiOptions {
        cwd: Some(here.clone()),
        prefs_dir: Some(not_a_dir),
        ..Default::default()
    });
    let effects = app.use_workspace(&elsewhere);
    assert_eq!(created(&effects).len(), 1);
    assert!(
        app.live_flash()
            .is_some_and(|f| f.contains("recent workspace not saved")),
        "{:?}",
        app.live_flash()
    );
    assert_eq!(app.active_workspace(), Some(elsewhere.as_path()));
}

/// Ctrl+P in the picker saves a pin at once and again removes it; a pin
/// whose directory is gone stays in the file.
#[test]
fn ctrl_p_in_the_picker_pins_and_unpins_in_ui_toml() {
    let (tmp, here, elsewhere) = dirs();
    let prefs = tmp.path().join("prefs");
    let gone = tmp.path().join("gone").display().to_string();
    crate::prefs::UiPrefs {
        pinned_workspaces: vec![gone.clone()],
        ..Default::default()
    }
    .save(&prefs)
    .expect("save");
    let mut app = launched_with(TuiOptions {
        cwd: Some(here.clone()),
        home: Some(crate::workspace::canonical(tmp.path())),
        prefs_dir: Some(prefs.clone()),
        recent_workspaces: vec![elsewhere.display().to_string()],
        pinned_workspaces: vec![gone.clone()],
        ..Default::default()
    });
    let saved = || {
        crate::prefs::UiPrefs::load(&prefs)
            .expect("prefs")
            .pinned_workspaces
    };

    app.reduce(ctrl_key(KeyCode::Char('o')));
    // The cursor starts on the current workspace, the launch directory.
    app.reduce(ctrl_key(KeyCode::Char('p')));
    let here_text = here.display().to_string();
    assert_eq!(saved(), [gone.clone(), here_text.clone()]);
    assert_eq!(app.pinned_workspaces, saved());
    assert!(
        app.live_flash().is_some_and(|f| f == "pinned ~/one"),
        "{:?}",
        app.live_flash()
    );
    assert_eq!(
        app.bottom.top_view().and_then(|view| view.kind()),
        Some(ViewKind::Workspace),
        "the picker stays open"
    );

    app.reduce(ctrl_key(KeyCode::Char('p')));
    assert_eq!(saved(), [gone]);
    assert_eq!(app.pinned_workspaces, saved());
    assert!(app.live_flash().is_some_and(|f| f == "unpinned ~/one"));
}

/// A pin that cannot be saved is said, not swallowed; it holds for this run.
#[test]
fn a_failed_pin_save_is_reported() {
    let (tmp, here, _) = dirs();
    let not_a_dir = tmp.path().join("file");
    std::fs::write(&not_a_dir, "").expect("write");
    let mut app = launched_with(TuiOptions {
        cwd: Some(here.clone()),
        prefs_dir: Some(not_a_dir),
        ..Default::default()
    });
    app.reduce(ctrl_key(KeyCode::Char('o')));
    app.reduce(ctrl_key(KeyCode::Char('p')));
    assert!(
        app.live_flash()
            .is_some_and(|f| f.contains("pin not saved")),
        "{:?}",
        app.live_flash()
    );
    assert_eq!(app.pinned_workspaces, [here.display().to_string()]);
}

/// A configured discovery root that is not there is said once per opening;
/// the picker still opens with what the other roots hold.
#[test]
fn a_missing_discovery_root_is_flagged_when_the_picker_opens() {
    let (tmp, here, _) = dirs();
    let mut app = launched_with(TuiOptions {
        cwd: Some(here.clone()),
        home: Some(crate::workspace::canonical(tmp.path())),
        workspace_roots: Some(vec!["~/one".into(), "~/nope".into(), "relative".into()]),
        ..Default::default()
    });
    app.reduce(ctrl_key(KeyCode::Char('o')));
    assert_eq!(
        app.live_flash(),
        Some("workspace roots not found: ~/nope, relative")
    );
    assert_eq!(
        app.bottom.top_view().and_then(|view| view.kind()),
        Some(ViewKind::Workspace)
    );

    let mut quiet = launched_with(TuiOptions {
        cwd: Some(here),
        home: Some(crate::workspace::canonical(tmp.path())),
        ..Default::default()
    });
    quiet.reduce(ctrl_key(KeyCode::Char('o')));
    assert!(
        !quiet.live_flash().is_some_and(|f| f.contains("not found")),
        "home by default is never flagged"
    );
}

/// A root listed many times is checked once and named once, and a long list
/// of missing roots is cut short in the flash.
#[test]
fn repeated_missing_roots_are_named_once_and_briefly() {
    let (tmp, here, _) = dirs();
    let mut roots = vec!["~/nope".to_owned(); 5000];
    roots.extend(["~/a".into(), "~/b".into(), "~/c".into()]);
    let mut app = launched_with(TuiOptions {
        cwd: Some(here),
        home: Some(crate::workspace::canonical(tmp.path())),
        workspace_roots: Some(roots),
        ..Default::default()
    });
    app.reduce(ctrl_key(KeyCode::Char('o')));
    assert_eq!(
        app.live_flash(),
        Some("workspace roots not found: ~/nope, ~/a, ~/b and 1 more")
    );
}

#[test]
fn live_workspaces_count_agents_and_order_by_activity() {
    let (_tmp, here, elsewhere) = dirs();
    let old = root_in("old", &here, 100);
    let mut child = root_in("child", &here, 100);
    child.parent = Some(old.id);
    child.status = AgentStatus::Running;
    let newer = root_in("newer", &elsewhere, 3);
    let mut done = root_in("done", &elsewhere, 1);
    done.status = AgentStatus::Completed { summary: None };
    let mut app = launched_with(TuiOptions::default());
    app.tree = tree_of(vec![old, child, newer, done]);
    let live = app.live_workspaces();
    let summary: Vec<(&Path, usize, usize)> = live
        .iter()
        .map(|w| (w.path.as_path(), w.agents, w.running))
        .collect();
    assert_eq!(
        summary,
        [(elsewhere.as_path(), 1, 0), (here.as_path(), 2, 1)]
    );
}

mod races;
