use qimu_core::store::Store;
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn legacy_branch_stays_unknown_and_new_fork_keeps_its_explicit_source() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("qimu-index-lineage-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let conn = Connection::open(root.join("qimu.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE index_branches(index_id TEXT NOT NULL, name TEXT NOT NULL,
          head_commit TEXT, PRIMARY KEY(index_id, name));
          INSERT INTO index_branches VALUES('legacy-index', 'legacy', NULL);",
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&root).unwrap();
    let graph = store.index_graph("legacy-index").unwrap();
    assert!(graph.branches[0].source_branch.is_none());

    let group = store.create_group("Company").unwrap();
    let index = store
        .create_index(
            "Repository",
            None,
            &group.id,
            "Maintainer",
            "company",
            vec![],
        )
        .unwrap();
    let fork = store.branch_head(&index.id, "main").unwrap().unwrap();
    store.create_branch(&index.id, "parent", "main").unwrap();
    // The inherited commit belongs to main, while parent is the branch selected at creation.
    store.create_branch(&index.id, "child", "parent").unwrap();
    let head = store
        .create_commit(&index.id, "child", "Maintainer", "Child edit", vec![])
        .unwrap();
    store
        .create_commit(&index.id, "parent", "Maintainer", "Parent edit", vec![])
        .unwrap();
    assert!(store
        .create_branch(&index.id, "invalid", "missing")
        .is_err());
    drop(store);

    let store = Store::open(&root).unwrap();
    let graph = store.index_graph(&index.id).unwrap();
    let child = graph
        .branches
        .iter()
        .find(|branch| branch.name == "child")
        .unwrap();
    assert_eq!(child.source_branch.as_deref(), Some("parent"));
    assert_eq!(child.source_index_id.as_deref(), Some(index.id.as_str()));
    assert_eq!(child.fork_commit_id.as_deref(), Some(fork.as_str()));
    assert_eq!(child.head_commit.as_deref(), Some(head.id.as_str()));
    assert!(!graph.branches.iter().any(|branch| branch.name == "invalid"));

    let derived = store
        .create_english_index(&index, "child", &head.id, "English", "Maintainer", vec![])
        .unwrap();
    let graph = store.index_graph(&derived.id).unwrap();
    assert_eq!(
        graph.branches[0].source_index_id.as_deref(),
        Some(index.id.as_str())
    );
    assert_eq!(graph.branches[0].source_branch.as_deref(), Some("child"));
    assert_eq!(
        graph.branches[0].fork_commit_id.as_deref(),
        Some(head.id.as_str())
    );

    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
