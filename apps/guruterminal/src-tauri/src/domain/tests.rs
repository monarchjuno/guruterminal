use super::*;

fn write(authority: MemoryChangeAuthority) -> MemoryWrite {
    let (before_markdown, proposed_markdown) = match authority {
        MemoryChangeAuthority::Chat => ("before", "after"),
        MemoryChangeAuthority::User => ("before", "after"),
    };
    MemoryWrite {
        guru_id: "guru-1".into(),
        authority,
        targets: vec![MemoryChangeTarget {
            record_id: "lens:quality".into(),
            relative_path: "guruterminal/lens/quality.md".into(),
            before_markdown: before_markdown.into(),
            proposed_markdown: proposed_markdown.into(),
        }],
        rationale: "Keep one reusable lesson.".into(),
    }
}

#[test]
fn memory_change_authorities_are_closed_to_chat_and_user() {
    assert_eq!(
        serde_json::to_string(&MemoryChangeAuthority::Chat).unwrap(),
        "\"chat\""
    );
    assert_eq!(
        serde_json::to_string(&MemoryChangeAuthority::User).unwrap(),
        "\"user\""
    );
    assert!(serde_json::from_str::<MemoryChangeAuthority>("\"undo\"").is_err());
    assert!(serde_json::from_str::<MemoryChangeAuthority>("\"training\"").is_err());
}

#[test]
fn memory_targets_reject_noncanonical_paths() {
    let mut write = write(MemoryChangeAuthority::Chat);
    write.targets[0].relative_path = "../quality.md".into();
    assert!(write.validate().is_err());
}

#[test]
fn user_authority_allows_delete() {
    let mut user = write(MemoryChangeAuthority::User);
    user.targets[0].proposed_markdown.clear();
    assert!(user.validate().is_ok());
    let mut chat = write(MemoryChangeAuthority::Chat);
    chat.targets[0].proposed_markdown.clear();
    assert!(chat.validate().is_err());
}

#[test]
fn active_title_or_alias_collision_is_detected_and_revoked_pages_are_ignored() {
    let markdown = "---\nid: wiki:tsmc-b\ntitle: TSMC foundry economics\naliases:\n  - Taiwan Semiconductor\n  - 2330.TW\nas_of: 2026-08-19T00:00:00Z\n---\n";
    assert_eq!(
        markdown_frontmatter_scalar(markdown, "title").as_deref(),
        Some("TSMC foundry economics")
    );
    assert_eq!(
        markdown_frontmatter_list(markdown, "aliases"),
        vec!["Taiwan Semiconductor", "2330.TW"]
    );
    let records = vec![
        MemoryIdentityRecord {
            id: "wiki:tsmc-a".into(),
            title: "TSMC Foundry Economics".into(),
            aliases: vec!["2330.TW".into()],
            status: Some("active".into()),
        },
        MemoryIdentityRecord {
            id: "wiki:old".into(),
            title: "Taiwan Semiconductor".into(),
            aliases: Vec::new(),
            status: Some("revoked".into()),
        },
    ];
    assert_eq!(
        colliding_active_memory_id(
            "wiki:tsmc-b",
            "TSMC foundry economics",
            &["Taiwan Semiconductor".into()],
            &records
        )
        .as_deref(),
        Some("wiki:tsmc-a")
    );
    assert!(
        colliding_active_memory_id("wiki:tsmc-a", "TSMC foundry economics", &[], &records)
            .is_none()
    );
    assert!(
        colliding_active_memory_id("wiki:new", "Airline loyalty breakage", &[], &records).is_none()
    );
}

#[test]
fn wiki_proposal_rejects_missing_summary_before_apply() {
    let error = MemoryProposal::new(
        "proposal-1".into(),
        "Wiki".into(),
        "wiki:cobalt".into(),
        MemoryProposalBase::Absent,
        "---\nid: wiki:cobalt\ntitle: WP4 cobalt-foil spare-capacity rule\nas_of: 2026-08-24T00:00:00Z\n---\n\n# Scope\n\nReusable method.\n".into(),
        "Teach the standing method from this turn.".into(),
        vec!["evidence:current".into()],
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("memory proposal Markdown summary is required"),
        "{error}"
    );
}

#[test]
fn wiki_proposal_rejects_date_only_as_of_before_apply() {
    let error = MemoryProposal::new(
        "proposal-1".into(),
        "Wiki".into(),
        "wiki:cobalt".into(),
        MemoryProposalBase::Absent,
        "---\nid: wiki:cobalt\ntitle: WP4 cobalt-foil spare-capacity rule\nsummary: Standing method.\nas_of: 2026-08-24\n---\n\n# Scope\n\nReusable method.\n".into(),
        "Teach the standing method from this turn.".into(),
        vec!["evidence:current".into()],
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("as_of must be RFC3339 with seconds and timezone"),
        "{error}"
    );
}
