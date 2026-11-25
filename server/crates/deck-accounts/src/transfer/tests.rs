use super::*;
use std::time::Duration;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "deck-transfer-{name}-{}-{}-{}",
        std::process::id(),
        crate::now_ms(),
        crate::random_id().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn places(root: &Path) -> Places {
    Places {
        work: root.join("work"),
        claude_home: root.join(".claude"),
        worktrees: root.join("worktrees"),
        removable: Some(root.to_path_buf()),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_text(dir, args).unwrap()
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            msg,
        ],
    );
}

fn wait(t: &Transfer, job: &str) -> Value {
    for _ in 0..600 {
        let s = t.status(job).unwrap();
        if s["state"] != json!("running") {
            return s;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("job {job} did not end");
}

/// Export on `from`, relay in small chunks as Deck does, import on `to`.
fn relay(
    from: &Transfer,
    export: &Value,
    to: &Transfer,
    kind: &str,
    target: Option<PathBuf>,
    on_conflict: &str,
) -> Value {
    let job = from.export_start(export).unwrap()["job"]
        .as_str()
        .unwrap()
        .to_string();
    let packed = wait(from, &job);
    assert_eq!(packed["state"], json!("done"), "{packed}");
    let size = packed["size"].as_u64().unwrap();
    let imp = to.import_start_at(kind, target).unwrap()["job"]
        .as_str()
        .unwrap()
        .to_string();
    let mut offset = 0u64;
    loop {
        let chunk = from
            .export_read(&json!({ "job": job, "offset": offset, "length": 1000 }))
            .unwrap();
        let data = chunk["data"].as_str().unwrap();
        let n = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap()
            .len() as u64;
        to.import_write(&json!({ "job": imp, "offset": offset, "data": data }))
            .unwrap();
        offset += n;
        if chunk["eof"] == json!(true) {
            break;
        }
    }
    assert_eq!(offset, size);
    // A chunk out of order is refused.
    assert!(to
        .import_write(&json!({ "job": imp, "offset": 1, "data": "" }))
        .is_err());
    to.import_commit(&json!({ "job": imp, "size": size, "on_conflict": on_conflict }))
        .unwrap();
    let done = wait(to, &imp);
    from.finish(&job).unwrap();
    assert!(!from.pack_path(&job).exists(), "the pack goes once relayed");
    done
}

#[test]
fn claude_folder_names_and_branch_slugs() {
    assert_eq!(project_dir_name("/home/me/atlas"), "-home-me-atlas");
    assert_eq!(
        project_dir_name(r"C:\Workspace\5.agents\deck"),
        "C--Workspace-5-agents-deck"
    );
    assert_eq!(branch_slug("feat/Clone UI"), "feat-clone-ui");
    assert_eq!(branch_slug("///"), "worktree");
}

#[test]
fn old_paths_become_new_ones() {
    let map = PathMap::new(&[
        (r"C:\Workspace\atlas".into(), "/home/me/atlas".into()),
        ("C:/w/atlas-feat".into(), "/home/me/wt/feat".into()),
    ]);
    let sep = std::path::MAIN_SEPARATOR;
    let sub = |s: &str| {
        if cfg!(windows) {
            s.replace('/', "\\")
        } else {
            s.to_string()
        }
    };
    assert_eq!(
        map.rewrite(r"C:\Workspace\atlas").as_deref(),
        Some(sub("/home/me/atlas").as_str())
    );
    assert_eq!(
        map.rewrite(r"C:\Workspace\atlas\src\a.rs"),
        Some(sub("/home/me/atlas/src/a.rs"))
    );
    assert_eq!(
        map.rewrite("c:/Workspace/atlas/x"),
        Some(sub("/home/me/atlas/x"))
    );
    // Inside a longer text only the folder is replaced.
    assert_eq!(
        map.rewrite(r"cd C:\Workspace\atlas && ls").as_deref(),
        Some(r"cd /home/me/atlas && ls")
    );
    // atlas2 is another folder.
    assert_eq!(map.rewrite(r"C:\Workspace\atlas2"), None);
    assert_eq!(map.rewrite("nothing here"), None);
    let _ = sep;
    let line = json!({"type": "user", "cwd": r"C:\Workspace\atlas", "message": {"content": [{"text": r"open C:\Workspace\atlas\a"}]}, "n": 1}).to_string();
    let out: Value = serde_json::from_str(&map.rewrite_line(&line)).unwrap();
    assert_eq!(out["cwd"], json!(sub("/home/me/atlas")));
    assert_eq!(
        out["message"]["content"][0]["text"],
        json!(r"open /home/me/atlas\a")
    );
    assert_eq!(out["n"], json!(1));
    assert_eq!(map.rewrite_line("not json"), "not json");
    assert_eq!(map.rewrite_jsonl("{\"a\":1}\r\n\nx\n"), "{\"a\":1}\n\nx\n");
}

#[test]
fn only_harness_items_travel() {
    for ok in [
        "CLAUDE.md",
        "skills/deploy",
        "agents/reviewer.md",
        "commands/git/sync.md",
        "rules/style.md",
        "output-styles/terse.md",
    ] {
        assert!(harness_item_ok(ok), "{ok}");
    }
    for bad in [
        ".credentials.json",
        "settings.json",
        "settings.local.json",
        "projects/x/y.jsonl",
        "accounts",
        ".claude.json",
        "skills",
        "skills/../.credentials.json",
        "/etc/passwd",
        "CLAUDE.md/x",
        "todos/a.json",
        "skills/x/.env",
        "plugins/x",
    ] {
        assert!(!harness_item_ok(bad), "{bad}");
    }
}

#[test]
fn worktree_lists_are_read() {
    let w = parse_worktrees("worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /wt/feat\nHEAD def\nbranch refs/heads/feat/x\n\nworktree /wt/d\nHEAD 123\ndetached\n");
    assert_eq!(
        w,
        vec![
            Worktree {
                path: "/r".into(),
                branch: Some("main".into())
            },
            Worktree {
                path: "/wt/feat".into(),
                branch: Some("feat/x".into())
            },
            Worktree {
                path: "/wt/d".into(),
                branch: None
            },
        ]
    );
}

const SID: &str = "0b9c8f3e-1234-4abc-9def-001122334455";

/// A repository with a linked worktree, uncommitted changes, untracked and
/// ignored files and a Claude session, moved to another "machine".
#[test]
fn a_repository_with_its_worktrees_and_sessions_moves() {
    let src_root = temp("src");
    let dst_root = temp("dst");
    let src = Transfer::new(places(&src_root));
    let dst = Transfer::new(places(&dst_root));
    let repo = src_root.join("atlas");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &["remote", "add", "origin", "https://example.com/o/atlas.git"],
    );
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    std::fs::write(
        repo.join(".gitignore"),
        "build/\n.claude/settings.local.json\n",
    )
    .unwrap();
    commit(&repo, "first");
    let feat = src_root.join("wt-feat");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/x",
            &feat.to_string_lossy(),
        ],
    );
    // Uncommitted work in both checkouts.
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(repo.join("new.txt"), "untracked\n").unwrap();
    std::fs::create_dir_all(repo.join("build")).unwrap();
    std::fs::write(repo.join("build").join("out.bin"), "ignored").unwrap();
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(repo.join(".claude").join("settings.local.json"), "{}").unwrap();
    std::fs::write(feat.join("feat.txt"), "feat work\n").unwrap();
    git(&feat, &["add", "feat.txt"]);
    // A session that ran in the main checkout.
    let proj = src_root
        .join(".claude")
        .join("projects")
        .join(project_dir_name(&repo.to_string_lossy()));
    std::fs::create_dir_all(proj.join(SID).join("subagents")).unwrap();
    let line = json!({"type": "user", "cwd": repo.to_string_lossy(), "sessionId": SID}).to_string();
    std::fs::write(proj.join(format!("{SID}.jsonl")), format!("{line}\n")).unwrap();
    std::fs::write(
        proj.join(SID).join("subagents").join("a.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();

    let top = git(&repo, &["rev-parse", "--show-toplevel"]);
    let feat_top = git(&feat, &["rev-parse", "--show-toplevel"]);
    let export = json!({
        "kind": "repo", "repo": top, "whole": false,
        "worktrees": [{ "path": top, "branch": "main" }, { "path": feat_top, "branch": "feat/x" }],
        "sessions": [{ "session_id": SID, "cwd": repo.to_string_lossy() }],
    });
    let target = dst_root.join("atlas");
    let done = relay(&src, &export, &dst, "repo", Some(target.clone()), "skip");
    assert_eq!(done["state"], json!("done"), "{done}");

    // History, branch, remote, uncommitted and untracked work.
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "one\ntwo\n"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("new.txt")).unwrap(),
        "untracked\n"
    );
    assert!(
        !target.join("build").exists(),
        "ignored build output stays behind"
    );
    assert!(
        target.join(".claude").join("settings.local.json").is_file(),
        "the repo's harness comes along"
    );
    assert_eq!(
        git(&target, &["remote", "get-url", "origin"]),
        "https://example.com/o/atlas.git"
    );
    assert_eq!(git(&target, &["log", "--format=%s", "main"]), "first");
    let wts = done["worktrees"].as_array().unwrap();
    assert_eq!(wts.len(), 2);
    let feat_new = PathBuf::from(wts[1]["path"].as_str().unwrap());
    assert!(
        feat_new.starts_with(dst_root.join("worktrees").join("atlas")),
        "{}",
        feat_new.display()
    );
    assert_eq!(
        std::fs::read_to_string(feat_new.join("feat.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "feat work\n"
    );
    assert_eq!(
        git(&feat_new, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat/x"
    );

    // The session sits under the new folder's name with the new cwd inside.
    let new_cwd = done["sessions"][0]["cwd"].as_str().unwrap().to_string();
    assert_eq!(new_cwd, target.to_string_lossy());
    let moved = dst_root
        .join(".claude")
        .join("projects")
        .join(project_dir_name(&new_cwd));
    let text = std::fs::read_to_string(moved.join(format!("{SID}.jsonl"))).unwrap();
    let v: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(v["cwd"], json!(new_cwd));
    assert!(moved.join(SID).join("subagents").join("a.jsonl").is_file());

    // MOVE: the source removes the worktree, its branch and the session; the main checkout stays.
    let rm = src
        .remove_start(&json!({ "kind": "repo", "repo": top, "whole": false,
            "worktrees": [{ "path": top }, { "path": feat_top }], "sessions": [SID] }))
        .unwrap();
    let removed = wait(&src, rm["job"].as_str().unwrap());
    assert_eq!(removed["state"], json!("done"), "{removed}");
    assert!(!feat.exists());
    assert!(repo.join("a.txt").exists());
    assert!(git(&repo, &["branch", "--list", "feat/x"]).is_empty());
    assert!(!proj.join(format!("{SID}.jsonl")).exists());

    // A folder taken by something that is not a repository is refused.
    let taken = dst_root.join("notes");
    std::fs::create_dir_all(&taken).unwrap();
    std::fs::write(taken.join("x.md"), "x").unwrap();
    assert!(dst
        .import_start_at("repo", Some(taken))
        .unwrap_err()
        .contains("already taken"));

    // The whole repository goes too.
    let rm = src
        .remove_start(&json!({ "kind": "repo", "repo": top, "whole": true }))
        .unwrap();
    assert_eq!(
        wait(&src, rm["job"].as_str().unwrap())["state"],
        json!("done")
    );
    assert!(!repo.exists());

    let _ = std::fs::remove_dir_all(&src_root);
    let _ = std::fs::remove_dir_all(&dst_root);
}

#[test]
fn a_whole_repository_without_commits_copies() {
    let src_root = temp("src0");
    let dst_root = temp("dst0");
    let src = Transfer::new(places(&src_root));
    let dst = Transfer::new(places(&dst_root));
    let repo = src_root.join("fresh");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("draft.md"), "draft").unwrap();
    let top = git(&repo, &["rev-parse", "--show-toplevel"]);
    let target = dst_root.join("fresh");
    let done = relay(
        &src,
        &json!({ "kind": "repo", "repo": top, "whole": true }),
        &dst,
        "repo",
        Some(target.clone()),
        "skip",
    );
    assert_eq!(done["state"], json!("done"), "{done}");
    assert_eq!(
        std::fs::read_to_string(target.join("draft.md")).unwrap(),
        "draft"
    );
    assert!(
        repo.join("draft.md").exists(),
        "a copy leaves the source as it is"
    );
    let _ = std::fs::remove_dir_all(&src_root);
    let _ = std::fs::remove_dir_all(&dst_root);
}

#[test]
fn personal_harness_items_copy_with_a_conflict_choice() {
    let src_root = temp("hsrc");
    let dst_root = temp("hdst");
    let src = Transfer::new(places(&src_root));
    let dst = Transfer::new(places(&dst_root));
    let home = src_root.join(".claude");
    std::fs::create_dir_all(home.join("skills").join("deploy")).unwrap();
    std::fs::write(
        home.join("skills").join("deploy").join("SKILL.md"),
        "new skill",
    )
    .unwrap();
    std::fs::create_dir_all(home.join("agents")).unwrap();
    std::fs::write(home.join("agents").join("reviewer.md"), "new agent").unwrap();
    std::fs::write(home.join("CLAUDE.md"), "new rules").unwrap();
    std::fs::write(home.join(".credentials.json"), "secret").unwrap();
    let there = dst_root.join(".claude");
    std::fs::create_dir_all(there.join("agents")).unwrap();
    std::fs::write(there.join("agents").join("reviewer.md"), "old agent").unwrap();
    std::fs::write(there.join("CLAUDE.md"), "old rules").unwrap();
    let items = json!(["skills/deploy", "agents/reviewer.md", "CLAUDE.md"]);

    // Credentials are refused outright.
    let bad = src
        .export_start(&json!({ "kind": "harness", "items": [".credentials.json"] }))
        .unwrap();
    assert_eq!(
        wait(&src, bad["job"].as_str().unwrap())["state"],
        json!("failed")
    );

    let conflicts = dst.conflicts(&json!({ "items": items })).unwrap();
    assert_eq!(
        conflicts["existing"],
        json!(["agents/reviewer.md", "CLAUDE.md"])
    );

    let done = relay(
        &src,
        &json!({ "kind": "harness", "items": items }),
        &dst,
        "harness",
        None,
        "skip",
    );
    assert_eq!(done["state"], json!("done"), "{done}");
    assert_eq!(
        std::fs::read_to_string(there.join("skills/deploy/SKILL.md")).unwrap(),
        "new skill"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("CLAUDE.md")).unwrap(),
        "old rules"
    );
    assert_eq!(done["skipped"], json!(["agents/reviewer.md", "CLAUDE.md"]));

    let done = relay(
        &src,
        &json!({ "kind": "harness", "items": items }),
        &dst,
        "harness",
        None,
        "keep_both",
    );
    assert_eq!(
        std::fs::read_to_string(there.join("agents/reviewer-2.md")).unwrap(),
        "new agent"
    );
    assert_eq!(
        std::fs::read_to_string(there.join("agents/reviewer.md")).unwrap(),
        "old agent"
    );
    assert!(there.join("skills/deploy-2/SKILL.md").is_file());
    assert_eq!(done["renamed"].as_array().unwrap().len(), 3);

    relay(
        &src,
        &json!({ "kind": "harness", "items": items }),
        &dst,
        "harness",
        None,
        "overwrite",
    );
    assert_eq!(
        std::fs::read_to_string(there.join("CLAUDE.md")).unwrap(),
        "new rules"
    );
    assert!(!there.join(".credentials.json").exists());

    // MOVE: the items leave the source; credentials are never touched.
    let rm = src
        .remove_start(&json!({ "kind": "harness", "items": items }))
        .unwrap();
    assert_eq!(
        wait(&src, rm["job"].as_str().unwrap())["state"],
        json!("done")
    );
    assert!(!home.join("skills/deploy").exists() && !home.join("CLAUDE.md").exists());
    assert!(home.join(".credentials.json").exists());
    let _ = std::fs::remove_dir_all(&src_root);
    let _ = std::fs::remove_dir_all(&dst_root);
}

/// A branch sent to a machine that already has the repository joins it as a
/// worktree in that server's worktrees folder; the checkout there is untouched.
#[test]
fn a_branch_joins_the_same_repository_already_there() {
    let src_root = temp("jsrc");
    let dst_root = temp("jdst");
    let src = Transfer::new(places(&src_root));
    let dst = Transfer::new(places(&dst_root));
    let repo = src_root.join("atlas");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    commit(&repo, "first");
    let top = git(&repo, &["rev-parse", "--show-toplevel"]);
    // The target got the whole repository earlier, and has local work on main.
    let target = dst_root.join("atlas");
    let first = relay(
        &src,
        &json!({ "kind": "repo", "repo": top, "whole": true }),
        &dst,
        "repo",
        Some(target.clone()),
        "skip",
    );
    assert_eq!(first["state"], json!("done"), "{first}");
    std::fs::write(target.join("local.txt"), "mine\n").unwrap();
    // A new branch on the source, with a session.
    let feat = src_root.join("wt-feat");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/y",
            &feat.to_string_lossy(),
        ],
    );
    std::fs::write(feat.join("f.txt"), "feat\n").unwrap();
    commit(&feat, "feat");
    std::fs::write(feat.join("wip.txt"), "wip\n").unwrap();
    let proj = src_root
        .join(".claude")
        .join("projects")
        .join(project_dir_name(&feat.to_string_lossy()));
    std::fs::create_dir_all(&proj).unwrap();
    let line = json!({"type": "user", "cwd": feat.to_string_lossy()}).to_string();
    std::fs::write(proj.join(format!("{SID}.jsonl")), format!("{line}\n")).unwrap();
    let feat_top = git(&feat, &["rev-parse", "--show-toplevel"]);
    let export = json!({
        "kind": "repo", "repo": top, "whole": false,
        "worktrees": [{ "path": feat_top, "branch": "feat/y" }],
        "sessions": [{ "session_id": SID, "cwd": feat.to_string_lossy() }],
    });
    // Herdr's worktrees folder on the target.
    let herdr_wt = dst_root.join("herdr-worktrees");
    let job = src.export_start(&export).unwrap()["job"]
        .as_str()
        .unwrap()
        .to_string();
    let size = wait(&src, &job)["size"].as_u64().unwrap();
    let imp = dst
        .import_start(&json!({ "kind": "repo", "path": target.to_string_lossy(), "worktrees": herdr_wt.to_string_lossy() }))
        .unwrap()["job"].as_str().unwrap().to_string();
    let mut off = 0u64;
    loop {
        let c = src
            .export_read(&json!({ "job": job, "offset": off }))
            .unwrap();
        let d = c["data"].as_str().unwrap();
        dst.import_write(&json!({ "job": imp, "offset": off, "data": d }))
            .unwrap();
        off += base64::engine::general_purpose::STANDARD
            .decode(d)
            .unwrap()
            .len() as u64;
        if c["eof"] == json!(true) {
            break;
        }
    }
    dst.import_commit(&json!({ "job": imp, "size": size }))
        .unwrap();
    let done = wait(&dst, &imp);
    assert_eq!(done["state"], json!("done"), "{done}");
    assert_eq!(done["joined"], json!(true));
    let wt = PathBuf::from(done["worktrees"][0]["path"].as_str().unwrap());
    assert!(wt.starts_with(herdr_wt.join("atlas")), "{}", wt.display());
    assert_eq!(
        std::fs::read_to_string(wt.join("f.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "feat\n"
    );
    assert!(wt.join("wip.txt").is_file(), "uncommitted work came along");
    assert!(
        target.join("local.txt").is_file(),
        "the checkout there is untouched"
    );
    let listed = git(&target, &["worktree", "list", "--porcelain"]);
    assert!(listed.contains("branch refs/heads/feat/y"), "{listed}");
    assert!(git(&target, &["for-each-ref", "refs/deck-transfer"]).is_empty());
    let new_cwd = done["sessions"][0]["cwd"].as_str().unwrap();
    assert!(dst_root
        .join(".claude")
        .join("projects")
        .join(project_dir_name(new_cwd))
        .join(format!("{SID}.jsonl"))
        .is_file());

    // Another repository under that name is refused.
    let other = dst_root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    std::fs::write(other.join("x"), "x").unwrap();
    commit(&other, "x");
    let again = relay(&src, &export, &dst, "repo", Some(other.clone()), "skip");
    assert_eq!(again["state"], json!("failed"));
    assert!(
        again["message"]
            .as_str()
            .unwrap()
            .contains("another repository"),
        "{again}"
    );
    let _ = std::fs::remove_dir_all(&src_root);
    let _ = std::fs::remove_dir_all(&dst_root);
}
