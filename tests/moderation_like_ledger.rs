use live_archive_core::moderation::journal::ProbeJournal;
#[test]
fn periods_are_persistent_isolated_by_tab_and_room_and_do_not_depend_on_run_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.sqlite3");
    let mut ledger = ProbeJournal::open(&path).unwrap();
    assert!(
        ledger
            .claim_like_period("room-a#tab-a", 10, 600000)
            .unwrap()
    );
    assert!(
        !ledger
            .claim_like_period("room-a#tab-a", 10, 600000)
            .unwrap()
    );
    assert!(
        ledger
            .claim_like_period("room-a#tab-b", 10, 600000)
            .unwrap()
    );
    assert!(
        ledger
            .claim_like_period("room-b#tab-a", 10, 600000)
            .unwrap()
    );
    drop(ledger);
    let mut restarted = ProbeJournal::open(&path).unwrap();
    assert!(
        !restarted
            .claim_like_period("room-a#tab-a", 10, 600000)
            .unwrap()
    );
    assert!(
        restarted
            .claim_like_period("room-a#tab-a", 10, 1200000)
            .unwrap()
    );
}
