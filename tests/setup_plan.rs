use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use vrg::setup::{FilePlan, PlannedFile, SetupOptions, plan_setup, recover_transaction};

fn fixture() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "vrg-setup-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(p.join("project")).unwrap();
    fs::create_dir_all(p.join("notes")).unwrap();
    p
}
#[test]
fn dry_plan_and_idempotence() {
    let p = fixture();
    let options = SetupOptions {
        project: p.join("project"),
        roots: vec![p.join("notes")],
        config_path: p.join("config/vrg.toml"),
        data_dir: p.join("data"),
        model_dir: None,
        memory_dir: None,
        synthetic_root: false,
    };
    let plan = plan_setup(&options, vec![]).unwrap();
    assert_eq!(plan.changes().len(), 1);
    assert!(!p.join("config").exists());
    plan.apply(
        &p.join("private-backups"),
        &p.join("project"),
        &options.config_path,
    )
    .unwrap();
    let bytes = fs::read(&options.config_path).unwrap();
    let second = plan_setup(&options, vec![]).unwrap();
    assert!(second.changes().is_empty());
    second
        .apply(
            &p.join("private-backups"),
            &p.join("project"),
            &options.config_path,
        )
        .unwrap();
    assert_eq!(bytes, fs::read(&options.config_path).unwrap());
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn conflict_and_safe_recovery() {
    let p = fixture();
    let path = p.join("project/file");
    fs::write(&path, b"old").unwrap();
    let plan = FilePlan::new(vec![PlannedFile {
        path: path.clone(),
        content: b"new".to_vec(),
    }])
    .unwrap();
    fs::write(&path, b"external").unwrap();
    assert!(
        plan.apply(
            &p.join("backup"),
            &p.join("project"),
            &p.join("config.toml")
        )
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"external");
    let plan = FilePlan::new(vec![PlannedFile {
        path: path.clone(),
        content: b"new".to_vec(),
    }])
    .unwrap();
    let backup = plan
        .apply(
            &p.join("backup"),
            &p.join("project"),
            &p.join("config.toml"),
        )
        .unwrap()
        .backup
        .unwrap();
    fs::write(&path, b"later").unwrap();
    assert!(recover_transaction(&backup).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"later");
    fs::write(&path, b"new").unwrap();
    assert_eq!(recover_transaction(&backup).unwrap(), 1);
    assert_eq!(fs::read(&path).unwrap(), b"external");
    fs::remove_dir_all(p).unwrap();
}

#[test]
fn project_lock_blocks_setup_and_recovery_even_with_another_backup_directory() {
    let p = fixture();
    let project = p.join("project");
    let path = project.join("file");
    let first = FilePlan::new(vec![PlannedFile {
        path: path.clone(),
        content: b"one".to_vec(),
    }])
    .unwrap();
    let backup = first
        .apply(&p.join("backup-one"), &project, &p.join("config.toml"))
        .unwrap()
        .backup
        .unwrap();
    let second = FilePlan::new(vec![PlannedFile {
        path: path.clone(),
        content: b"two".to_vec(),
    }])
    .unwrap();
    let lock = fs::File::open(&project).unwrap();
    lock.try_lock().unwrap();
    assert!(
        second
            .apply(&p.join("backup-two"), &project, &p.join("config.toml"))
            .is_err()
    );
    assert!(recover_transaction(&backup).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"one");
    drop(lock);
    assert_eq!(recover_transaction(&backup).unwrap(), 1);
    fs::remove_dir_all(p).unwrap();
}

#[test]
fn config_lock_serializes_distinct_projects_and_recovery() {
    let p = fixture();
    let project = p.join("project");
    let other = p.join("other");
    fs::create_dir(&other).unwrap();
    let config = p.join("shared-config");
    let plan = FilePlan::new(vec![PlannedFile {
        path: config.clone(),
        content: b"one".to_vec(),
    }])
    .unwrap();
    let backup = plan
        .apply(&p.join("backup-one"), &project, &config)
        .unwrap()
        .backup
        .unwrap();
    let next = FilePlan::new(vec![PlannedFile {
        path: config.clone(),
        content: b"two".to_vec(),
    }])
    .unwrap();
    let lock_path = fs::read_dir(&p)
        .unwrap()
        .map(|x| x.unwrap().path())
        .find(|x| {
            x.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".vrg-config-")
        })
        .unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    lock.try_lock().unwrap();
    assert!(next.apply(&p.join("backup-two"), &other, &config).is_err());
    assert!(recover_transaction(&backup).is_err());
    assert_eq!(fs::read(&config).unwrap(), b"one");
    drop(lock);
    assert_eq!(recover_transaction(&backup).unwrap(), 1);
    fs::remove_dir_all(p).unwrap();
}

#[test]
fn external_precondition_change_blocks_application() {
    let p = fixture();
    let project = p.join("project");
    let evidence = project.join("hooks.json");
    fs::write(&evidence, b"approved").unwrap();
    let target = project.join("observer");
    let plan = FilePlan::new(vec![PlannedFile {
        path: target.clone(),
        content: b"resource".to_vec(),
    }])
    .unwrap()
    .with_preconditions(vec![(evidence.clone(), b"approved".to_vec())])
    .unwrap();
    fs::write(evidence, b"changed").unwrap();
    assert!(
        plan.apply(&p.join("backup"), &project, &p.join("config.toml"))
            .is_err()
    );
    assert!(!target.exists());
    fs::remove_dir_all(p).unwrap();
}

#[test]
fn synthetic_profile_is_planned_without_writes_and_admits_only_sample() {
    let p = fixture();
    let project = p.join("project");
    let source = project.join(".vrg-synthetic-notes");
    let options = SetupOptions {
        project: project.clone(),
        roots: vec![source.clone()],
        config_path: p.join("config/vrg.toml"),
        data_dir: p.join("data"),
        model_dir: None,
        memory_dir: None,
        synthetic_root: true,
    };
    let plan = plan_setup(&options, vec![]).unwrap();
    assert!(!source.exists());
    plan.apply(&p.join("backup"), &project, &options.config_path)
        .unwrap();
    assert!(source.join("sample.md").is_file());
    let config = vrg::VrgConfig::load(&options.config_path).unwrap();
    assert_eq!(
        config.roots[0].allowed_files.as_ref().unwrap(),
        &vec!["sample.md".to_owned()]
    );
    assert!(plan_setup(&options, vec![]).unwrap().changes().is_empty());
    fs::write(source.join("sample.md"), b"changed by user").unwrap();
    assert!(plan_setup(&options, vec![]).is_err());
    fs::remove_dir_all(p).unwrap();
}
