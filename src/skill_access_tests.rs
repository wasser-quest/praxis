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

/// The kernel host plus the channel's skill commands: activation is paired,
/// scoped and permission-checked (handoff §6D channel seam).
#[test]
fn backend_skill_activation_is_paired_scoped_and_permission_checked() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(&dir.path().join("db")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let skills = dir.path().join("skills/test");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(skills.join("skill.json"), r#"{"name":"test","description":"Test","required_parameters":["code"]}"#).unwrap();
    std::fs::write(skills.join("skill.poml"), "<poml><p>test</p></poml>").unwrap();
    // The host indexes its skill catalog at startup, after the fixtures.
    let host = crate::channels::KernelChannelHost::new(db.clone(), dir.path().to_path_buf()).unwrap();
    assert!(apply_skill_command(&host, "discord-a", None, "channel", "test").is_err());
    db.create_pairing("alice", "discord-a", None).unwrap();
    db.create_pairing("bob", "discord-b", None).unwrap();
    assert!(apply_skill_command(&host, "discord-a", None, "channel", "list").unwrap().contains("test"));
    apply_skill_command(&host, "discord-a", None, "channel", "test").unwrap();
    assert_eq!(db.load_context("alice").unwrap().settings.active_skill.as_deref(), Some("test"));
    assert!(db.load_context("bob").unwrap().settings.active_skill.is_none());
    assert!(apply_skill_command(&host, "discord-a", None, "channel", "unknown").is_err());
    crate::db::tools::set_plugin_tool_enabled(&db, "use_skill", false).unwrap();
    assert!(apply_skill_command(&host, "discord-a", None, "channel", "test").is_err());
    apply_skill_command(&host, "discord-a", None, "channel", "off").unwrap();
    let mut ctx = db.load_context("alice").unwrap();
    ctx.settings.allowed_channels = vec!["private".into()];
    db.save_context(&ctx).unwrap();
    assert!(apply_skill_command(&host, "discord-a", None, "channel", "list").is_err());
    assert!(db.load_context("alice").unwrap().settings.active_skill.is_none());
    ctx.settings.allowed_guilds = vec!["guild-1".into()];
    assert!(!channel_allowed(&serde_json::to_value(&ctx.settings).unwrap_or_default(), Some("guild-2"), "private"));
    assert!(channel_allowed(&serde_json::to_value(&ctx.settings).unwrap_or_default(), Some("guild-1"), "private"));
}
