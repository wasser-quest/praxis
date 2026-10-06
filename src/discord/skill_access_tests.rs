use super::*;

#[test]
fn skill_command_denies_all_unpaired_operations_before_reading_skills() {
    let directory = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(&directory.path().join("db")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let host = crate::channels::KernelChannelHost::new(db.clone(), directory.path().to_path_buf()).unwrap();
    db.create_pairing("alice", "discord-alice", None).unwrap();
    let mut owner = db.load_context("alice").unwrap();
    owner.settings.active_skill = Some("keep-this".into());
    db.save_context(&owner).unwrap();
    db.create_pending_pairing("pending-code", "pending-user", "2999-01-01T00:00:00Z")
        .unwrap();
    let broken = directory.path().join("private-unreadable-skill-directory");
    std::fs::write(&broken, "not a directory").unwrap();
    for discord_id in ["stranger", "pending-user", "alice"] {
        for argument in ["", "list", "off", "test", "unknown"] {
            for guild in [None, Some("guild")] {
                let error = apply_skill_command_from_dir(
                    &host, discord_id, guild, "channel", argument, &broken,
                )
                .unwrap_err()
                .to_string();
                assert!(error.contains("Please pair first"), "{error}");
                assert!(!error.contains("private-unreadable"));
            }
        }
    }
    assert_eq!(
        db.load_context("alice").unwrap().settings.active_skill,
        owner.settings.active_skill
    );
    assert_eq!(db.list_all_pairings().unwrap().len(), 1);
    assert!(db.get_pending_pairing("pending-code").unwrap().is_some());
}

#[test]
fn skill_command_rechecks_pairing_revocation_and_channel_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(&directory.path().join("db")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let host = crate::channels::KernelChannelHost::new(db.clone(), directory.path().to_path_buf()).unwrap();
    db.create_pairing("alice", "discord-alice", None).unwrap();
    let broken = directory.path().join("not-a-skill-directory");
    std::fs::write(&broken, "not a directory").unwrap();
    let mut owner = db.load_context("alice").unwrap();
    owner.settings.active_skill = Some("keep-this".into());
    owner.settings.allowed_channels = vec!["allowed".into()];
    owner.settings.allowed_guilds = vec!["allowed-guild".into()];
    db.save_context(&owner).unwrap();
    for (guild, channel) in [(None, "denied"), (Some("denied"), "allowed")] {
        let error =
            apply_skill_command_from_dir(&host, "discord-alice", guild, channel, "list", &broken)
                .unwrap_err();
        assert!(error.to_string().contains("not allowed"));
    }
    apply_skill_command_from_dir(&host, "discord-alice", None, "allowed", "off", &broken).unwrap();
    assert!(db
        .load_context("alice")
        .unwrap()
        .settings
        .active_skill
        .is_none());
    db.conn()
        .execute("DELETE FROM pairings WHERE user_id = 'alice'", [])
        .unwrap();
    for arg in ["list", "off", "test"] {
        assert!(
            apply_skill_command_from_dir(&host, "discord-alice", None, "allowed", arg, &broken)
                .unwrap_err()
                .to_string()
                .contains("Please pair first")
        );
    }
}
