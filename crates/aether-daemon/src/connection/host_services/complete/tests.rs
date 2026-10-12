use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;

use aether_protocol::ErrorCode;
use aether_protocol::remote::{PathCompletionKind, PathCompletions};

use super::*;
use crate::connection::host_services::test_support::{init_repo, run_git, running_as_root};

fn dirs(parent: &Path, names: &[&str]) {
    for name in names {
        std::fs::create_dir_all(parent.join(name)).unwrap();
    }
}

fn paths(completions: &PathCompletions) -> Vec<&str> {
    completions
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .collect()
}

fn directories(prefix: &str, home: Option<&Path>, limit: usize) -> PathCompletions {
    let target = directory_target(prefix, home).expect("target");
    complete_directories(&target, limit)
}

fn files(workspace: &Path, prefix: &str) -> PathCompletions {
    complete_workspace_files(workspace, prefix, MAX_LIMIT).expect("completions")
}

#[test]
fn directories_are_listed_absolute_sorted_and_with_a_trailing_slash() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["beta", "alpha", "alps"]);
    std::fs::write(root.join("alpine.txt"), "").unwrap();

    let all = directories(&format!("{}/", root.display()), None, 50);
    let some = directories(&format!("{}/al", root.display()), None, 50);

    let at = |name: &str| format!("{}/{name}/", root.display());
    assert_eq!(paths(&all), [at("alpha"), at("alps"), at("beta")]);
    assert_eq!(paths(&some), [at("alpha"), at("alps")]);
    assert!(all.entries.iter().all(|e| e.is_dir));
    assert!(!all.truncated);
}

#[test]
fn a_prefix_without_a_trailing_slash_completes_its_own_segment() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["src", "src-old", "docs/src"]);

    let found = directories(&format!("{}/src", root.display()), None, 50);

    let at = |name: &str| format!("{}/{name}/", root.display());
    assert_eq!(paths(&found), [at("src"), at("src-old")]);
    let inside = directories(&format!("{}/src/", root.display()), None, 50);
    assert!(inside.entries.is_empty(), "src has no sub-directories");
}

#[test]
fn hidden_directories_need_a_dot_and_matching_ignores_case_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &[".config", ".cache", "Code", "code-old", "Docs"]);
    let base = format!("{}/", root.display());

    let plain = directories(&base, None, 50);
    let dotted = directories(&format!("{base}."), None, 50);
    let lower = directories(&format!("{base}co"), None, 50);
    let upper = directories(&format!("{base}Co"), None, 50);

    let at = |name: &str| format!("{base}{name}/");
    assert_eq!(paths(&plain), [at("Code"), at("Docs"), at("code-old")]);
    assert_eq!(paths(&dotted), [at(".cache"), at(".config")]);
    assert_eq!(paths(&lower), [at("Code"), at("code-old")]);
    assert_eq!(paths(&upper), [at("Code")]);
}

#[test]
fn unicode_names_match_by_character_and_fold_case() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["Élan", "école", "日本語", "ünï"]);
    let base = format!("{}/", root.display());

    let accented = directories(&format!("{base}é"), None, 50);
    let cjk = directories(&format!("{base}日"), None, 50);
    let capital = directories(&format!("{base}Ü"), None, 50);

    assert_eq!(
        paths(&accented),
        [format!("{base}Élan/"), format!("{base}école/")]
    );
    assert_eq!(paths(&cjk), [format!("{base}日本語/")]);
    assert!(
        capital.entries.is_empty(),
        "an uppercase fragment is exact: ünï is lower case"
    );
    let exact = directories(&format!("{base}ü"), None, 50);
    assert_eq!(paths(&exact), [format!("{base}ünï/")]);
}

#[test]
fn symlinks_to_directories_are_listed_unresolved_and_other_links_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["real"]);
    std::fs::write(root.join("file"), "").unwrap();
    symlink(root.join("real"), root.join("link")).unwrap();
    symlink(root.join("file"), root.join("filelink")).unwrap();
    symlink(root.join("gone"), root.join("dangling")).unwrap();

    let found = directories(&format!("{}/", root.display()), None, 50);

    let at = |name: &str| format!("{}/{name}/", root.display());
    assert_eq!(paths(&found), [at("link"), at("real")]);
}

#[test]
fn tilde_stands_for_the_home_directory() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    dirs(&home, &["projects", "pictures", "notes"]);
    let at = |name: &str| format!("{}/{name}/", home.display());

    let by_slash = directories("~/p", Some(&home), 50);
    let bare = directories("~", Some(&home), 50);
    let empty = directories("", Some(&home), 50);

    assert_eq!(paths(&by_slash), [at("pictures"), at("projects")]);
    assert_eq!(paths(&bare), [at("notes"), at("pictures"), at("projects")]);
    assert_eq!(bare, empty);
    let error = directory_target("~/x", None).expect_err("no home");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn only_absolute_and_home_prefixes_are_accepted() {
    for bad in ["src/", "./x", "../x", "~root/x", " /x"] {
        let error = directory_target(bad, Some(Path::new("/home/u"))).expect_err(bad);
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{bad}");
    }
}

#[test]
fn dot_segments_are_folded_without_resolving_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["a/deep", "b/target"]);
    symlink(root.join("a/deep"), root.join("b/via")).unwrap();

    let folded = directories(&format!("{}/a/../b/t", root.display()), None, 50);
    let beyond_root = directory_target("/../../x", None).unwrap();

    assert_eq!(paths(&folded), [format!("{}/b/target/", root.display())]);
    assert_eq!(beyond_root.parent, Path::new("/"));
}

#[test]
fn a_missing_or_unreadable_parent_is_an_empty_reply() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let missing = directories(&format!("{}/nope/x", root.display()), None, 50);
    assert_eq!(missing, PathCompletions::default());
    if running_as_root() {
        return;
    }
    let locked = root.join("locked");
    dirs(&root, &["locked/inside"]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let denied = directories(&format!("{}/", locked.display()), None, 50);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(denied, PathCompletions::default());
}

#[test]
fn a_huge_directory_is_cut_at_the_limit_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for n in 0..1_000 {
        std::fs::create_dir(root.join(format!("d{n:04}"))).unwrap();
    }

    let first = directories(&format!("{}/", root.display()), None, 50);
    let exact = directories(&format!("{}/", root.display()), None, 1_000);

    assert_eq!(first.entries.len(), 50);
    assert!(first.truncated);
    assert_eq!(first.entries[0].path, format!("{}/d0000/", root.display()));
    assert_eq!(first.entries[49].path, format!("{}/d0049/", root.display()));
    assert_eq!(exact.entries.len(), 1_000);
    assert!(!exact.truncated);
}

#[test]
fn the_scan_gives_up_after_its_entry_budget() {
    let mut scan = Scan::new();
    scan.entries = SCAN_ENTRIES - 1;
    assert!(scan.next_entry());
    assert!(!scan.next_entry());
    assert!(scan.cut);
    let completions = finish(Vec::new(), 10, &scan);
    assert!(completions.truncated, "a cut scan may have missed matches");
}

#[test]
fn names_with_control_characters_are_never_offered() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["fine", "bad\nname", "esc\u{1b}[31m"]);

    let found = directories(&format!("{}/", root.display()), None, 50);

    assert_eq!(paths(&found), [format!("{}/fine/", root.display())]);
}

#[test]
fn workspace_files_are_relative_with_directories_marked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["src/inner", "docs"]);
    std::fs::write(root.join("src/main.rs"), "").unwrap();
    std::fs::write(root.join("src/mod.rs"), "").unwrap();
    std::fs::write(root.join("README.md"), "").unwrap();

    let top = files(&root, "");
    let src = files(&root, "src/");
    let narrowed = files(&root, "src/ma");
    let smart = files(&root, "readme");

    assert_eq!(paths(&top), ["README.md", "docs/", "src/"]);
    assert_eq!(paths(&src), ["src/inner/", "src/main.rs", "src/mod.rs"]);
    assert_eq!(paths(&narrowed), ["src/main.rs"]);
    assert_eq!(paths(&smart), ["README.md"]);
    assert!(src.entries[0].is_dir && !src.entries[1].is_dir);
}

#[test]
fn files_ignored_by_git_are_left_out_but_hidden_ones_need_a_dot() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    std::fs::write(repo.join(".gitignore"), "target/\n*.log\n").unwrap();
    dirs(&repo, &["target", "src"]);
    std::fs::write(repo.join("debug.log"), "").unwrap();
    std::fs::write(repo.join("keep.txt"), "").unwrap();
    std::fs::write(repo.join("src/.gitignore"), "generated.rs\n").unwrap();
    std::fs::write(repo.join("src/generated.rs"), "").unwrap();
    std::fs::write(repo.join("src/lib.rs"), "").unwrap();
    run_git(&repo, &["add", "."]);

    let top = files(&repo, "");
    let hidden = files(&repo, ".");
    let nested = files(&repo, "src/");

    assert_eq!(paths(&top), ["keep.txt", "src/"]);
    assert_eq!(paths(&hidden), [".gitignore"], "`.git` is never offered");
    assert_eq!(paths(&nested), ["src/lib.rs"]);
}

#[test]
fn a_workspace_below_a_repository_root_honours_the_roots_ignore_file() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path(), "repo");
    std::fs::write(repo.join(".gitignore"), "*.tmp\n").unwrap();
    dirs(&repo, &["pkg"]);
    std::fs::write(repo.join("pkg/a.tmp"), "").unwrap();
    std::fs::write(repo.join("pkg/a.rs"), "").unwrap();

    assert_eq!(paths(&files(&repo.join("pkg"), "")), ["a.rs"]);
}

#[test]
fn nothing_outside_the_workspace_is_ever_listed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let workspace = root.join("ws");
    dirs(&root, &["ws/inside", "outside/secret"]);
    std::fs::write(root.join("outside/secret/key"), "").unwrap();
    std::fs::write(root.join("outside/loot.txt"), "").unwrap();
    std::fs::write(workspace.join("inside/ok.txt"), "").unwrap();
    symlink(root.join("outside"), workspace.join("escape")).unwrap();
    symlink(root.join("outside/loot.txt"), workspace.join("loot")).unwrap();
    symlink(workspace.join("inside"), workspace.join("shortcut")).unwrap();
    symlink("../outside", workspace.join("inside/up")).unwrap();

    let top = files(&workspace, "");
    let through_link = files(&workspace, "escape/");
    let through_nested = files(&workspace, "inside/up/");
    let through_inner_link = files(&workspace, "shortcut/");

    assert_eq!(
        paths(&top),
        ["inside/", "shortcut/"],
        "links out are omitted"
    );
    assert!(through_link.entries.is_empty());
    assert!(through_nested.entries.is_empty());
    assert_eq!(paths(&through_inner_link), ["shortcut/ok.txt"]);
    for bad in [
        "../",
        "../outside/",
        "inside/../../outside/",
        "/",
        "/etc/",
        "inside/..",
        "./../",
    ] {
        assert!(files(&workspace, bad).entries.is_empty(), "{bad}");
    }
}

#[test]
fn a_dot_prefix_and_repeated_slashes_stay_inside_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    dirs(&root, &["src"]);
    std::fs::write(root.join("src/a.rs"), "").unwrap();

    assert_eq!(paths(&files(&root, "./src/")), ["./src/a.rs"]);
    assert_eq!(paths(&files(&root, "src//")), ["src//a.rs"]);
}

#[test]
fn an_unusable_workspace_is_an_invalid_argument() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "").unwrap();
    for bad in [dir.path().join("missing"), dir.path().join("file")] {
        let error = complete_workspace_files(&bad, "", 10).expect_err("unusable");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
}

#[test]
fn a_huge_workspace_directory_is_cut_at_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for n in 0..600 {
        std::fs::write(root.join(format!("f{n:04}.txt")), "").unwrap();
    }

    let found = complete_workspace_files(&root, "f", 25).unwrap();

    assert_eq!(found.entries.len(), 25);
    assert!(found.truncated);
    assert_eq!(found.entries[0].path, "f0000.txt");
}

#[tokio::test]
async fn requests_are_validated_before_anything_is_read() {
    let request = |kind, prefix: &str, workspace: Option<&str>, limit| {
        complete_path(kind, prefix.into(), workspace.map(Into::into), limit)
    };
    let invalid = |result: Result<aether_protocol::Reply, ErrorReply>| {
        assert_eq!(
            result.expect_err("refused").code,
            ErrorCode::InvalidArgument
        );
    };
    invalid(request(PathCompletionKind::Directory, "/tm\0p", None, None).await);
    invalid(request(PathCompletionKind::Directory, "/tmp/\n", None, None).await);
    invalid(
        request(
            PathCompletionKind::Directory,
            &format!("/{}", "a".repeat(5000)),
            None,
            None,
        )
        .await,
    );
    invalid(request(PathCompletionKind::Directory, "relative", None, None).await);
    invalid(request(PathCompletionKind::Directory, "/tmp/", None, Some(0)).await);
    invalid(request(PathCompletionKind::WorkspaceFile, "", None, None).await);
    invalid(
        request(
            PathCompletionKind::WorkspaceFile,
            "",
            Some("relative"),
            None,
        )
        .await,
    );
    invalid(
        request(
            PathCompletionKind::WorkspaceFile,
            "",
            Some("/tmp/\u{7}"),
            None,
        )
        .await,
    );
    invalid(
        request(
            PathCompletionKind::WorkspaceFile,
            "a\u{1b}",
            Some("/tmp"),
            None,
        )
        .await,
    );
}

#[tokio::test]
async fn the_limit_defaults_to_fifty_and_never_exceeds_two_hundred() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for n in 0..300 {
        std::fs::create_dir(root.join(format!("d{n:03}"))).unwrap();
    }
    let prefix = format!("{}/", root.display());
    let count = |limit| {
        let prefix = prefix.clone();
        async move {
            let aether_protocol::Reply::PathCompletions(found) =
                complete_path(PathCompletionKind::Directory, prefix, None, limit)
                    .await
                    .expect("completions")
            else {
                panic!("wrong reply");
            };
            (found.entries.len(), found.truncated)
        }
    };

    assert_eq!(count(None).await, (50, true));
    assert_eq!(count(Some(7)).await, (7, true));
    assert_eq!(count(Some(10_000)).await, (200, true));
}
