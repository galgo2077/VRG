use std::{
    fs, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use vrg::{VrgConfig, VrgEngine};

#[test]
fn watcher_reconciles_changes_made_while_no_monitor_was_running() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp =
        std::env::temp_dir().join(format!("vrg-watch-recovery-{}-{nonce}", std::process::id()));
    let root = tmp.join("root");
    fs::create_dir_all(&root).unwrap();
    let source = root.join("source.md");
    fs::write(&source, "# Synthetic\nBefore\n").unwrap();
    let config_path = tmp.join("config.toml");
    fs::write(
        &config_path,
        format!(
            "data_dir = {:?}\n\n[[roots]]\nid = \"synthetic\"\npath = {:?}\n",
            tmp.join("derived"),
            root
        ),
    )
    .unwrap();
    let config = VrgConfig::load(&config_path).unwrap();
    let initial = VrgEngine::open(config.clone()).unwrap();
    let before = initial.index().unwrap().generation_id.unwrap();
    drop(initial);

    let watch_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(tmp.join("derived/vrg-watch.lock"))
        .unwrap();
    let lock_deadline = Instant::now() + Duration::from_secs(3);
    while watch_lock.try_lock().is_err() {
        assert!(Instant::now() < lock_deadline, "prior monitor did not stop");
        thread::sleep(Duration::from_millis(10));
    }
    drop(watch_lock);

    // The change happens while no monitor is running and the last status is recent.
    fs::write(&source, "# Synthetic\nAfter\n").unwrap();
    let resumed = VrgEngine::open(config).unwrap();
    resumed.start_change_monitor().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && resumed.status().generation_id.as_deref() == Some(&before) {
        thread::sleep(Duration::from_millis(50));
    }
    let status = resumed.status();
    let after = status.generation_id.clone();
    drop(resumed);
    let _ = fs::remove_dir_all(&tmp);
    assert_ne!(
        after.as_deref(),
        Some(before.as_str()),
        "fresh watcher missed an offline edit; status: {status:?}"
    );
}

#[test]
fn engine_reconciles_when_it_takes_over_a_lost_monitor() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp =
        std::env::temp_dir().join(format!("vrg-watch-takeover-{}-{nonce}", std::process::id()));
    let root = tmp.join("root");
    let data = tmp.join("derived");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&data).unwrap();
    let source = root.join("source.md");
    fs::write(&source, "# Synthetic\nBefore\n").unwrap();
    let config_path = tmp.join("config.toml");
    fs::write(
        &config_path,
        format!("data_dir = {data:?}\n\n[[roots]]\nid = \"synthetic\"\npath = {root:?}\n"),
    )
    .unwrap();
    let config = VrgConfig::load(&config_path).unwrap();

    // Emulate another process owning the monitor while this engine indexes.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("vrg-watch.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let engine = VrgEngine::open(config).unwrap();
    let before = engine.index().unwrap().generation_id.unwrap();
    drop(lock);
    fs::write(&source, "# Synthetic\nAfter\n").unwrap();

    // The same engine takes over the monitor after the unobserved change.
    engine.start_change_monitor().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && engine.status().generation_id.as_deref() == Some(&before) {
        thread::sleep(Duration::from_millis(50));
    }
    let status = engine.status();
    drop(engine);
    let _ = fs::remove_dir_all(&tmp);
    assert_ne!(
        status.generation_id.as_deref(),
        Some(before.as_str()),
        "watcher takeover missed an offline edit; status: {status:?}"
    );
}
