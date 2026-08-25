use std::{sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use tauri::ipc::Channel;

use super::{chat_runtime::parse_memory_kind, *};
use crate::{
    app::GuruRecoveryAction,
    browser::validated_http_url as validated_external_url,
    domain::{
        memory_refs_digest, CanonicalMemoryKind, ChatAttachment, ChatMessage, ChatMessageStatus,
        ChatRole, ChatSession, GuruStorageKind, MemoryPolicy,
    },
    hashing::sha256,
    run_coordinator::{RunKind, RunTarget},
};

#[path = "tests/chat.rs"]
mod chat_behavior;
mod contracts;
mod guru;
mod memory;
mod support;

#[cfg(unix)]
pub(super) use support::write_knowledge_runtime;
pub(super) use support::{
    bound_root, chat, initialized_workspace, lens_markdown, profile, seed_profile, wiki_markdown,
};

#[test]
fn canonical_memory_kind_projections_fail_closed() {
    for kind in CanonicalMemoryKind::ALL {
        let record_id = format!("{}:record", kind.slug());
        assert_eq!(memory_kind_from_id(&record_id).unwrap(), kind.label());
        assert_eq!(parse_memory_kind(kind.label()).unwrap(), kind.slug());
    }
    assert_eq!(
        memory_kind_from_id("skill:record").unwrap_err().code,
        "internal"
    );
    assert_eq!(
        memory_kind_from_id("wiki:../record").unwrap_err().code,
        "internal"
    );
    assert_eq!(
        parse_memory_kind("skill").unwrap_err().code,
        "invalid_request"
    );
}

#[test]
fn runtime_memory_summary_rejects_inconsistent_identity() {
    let error = runtime_record_summary(&serde_json::json!({
        "id": "wiki:record",
        "kind": "lens",
        "title": "Record",
    }))
    .unwrap_err();
    assert_eq!(error.code, "internal");
}

#[test]
fn guru_recovery_request_accepts_only_public_actions() {
    let request: GuruRecoverRequest = serde_json::from_value(serde_json::json!({
        "guru_id": "guru-a",
        "action": "recover_memory",
    }))
    .unwrap();
    assert_eq!(request.action, GuruRecoveryAction::RecoverMemory);
    assert!(
        serde_json::from_value::<GuruRecoverRequest>(serde_json::json!({
            "guru_id": "guru-a",
            "action": "recover_deletion",
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<GuruRecoverRequest>(serde_json::json!({
            "guru_id": "guru-a",
            "action": "recover_memory",
            "source": "unexpected",
        }))
        .is_err()
    );
}

#[test]
fn chat_connector_authority_seal_is_canonical_and_uses_only_revisions() {
    use crate::marketplace::connector_config::ConnectorConfigRevision;

    let raw_config_value = "must-not-enter-the-cache-seal@example.invalid";
    let mut first = ChatConnectorAuthoritySeal {
        version: CHAT_CONNECTOR_AUTHORITY_SEAL_VERSION,
        bindings: vec![
            ChatConnectorBindingSeal {
                entry_id: "zeta.connector".into(),
                enabled: true,
                execute: true,
                updated_at_ms: 20,
            },
            ChatConnectorBindingSeal {
                entry_id: "alpha.connector".into(),
                enabled: false,
                execute: false,
                updated_at_ms: 10,
            },
        ],
        connectors: vec![
            ChatConnectorSeal {
                entry_id: "zeta.connector".into(),
                config_revision: ConnectorConfigRevision::Revision(
                    "11111111-1111-4111-8111-111111111111".into(),
                ),
                active_credential_revision: Some("22222222-2222-4222-8222-222222222222".into()),
            },
            ChatConnectorSeal {
                entry_id: "alpha.connector".into(),
                config_revision: ConnectorConfigRevision::Absent,
                active_credential_revision: None,
            },
        ],
    };
    let mut reordered = ChatConnectorAuthoritySeal {
        version: CHAT_CONNECTOR_AUTHORITY_SEAL_VERSION,
        bindings: first.bindings.iter().cloned().rev().collect(),
        connectors: first.connectors.iter().cloned().rev().collect(),
    };
    canonicalize_chat_connector_authority_seal(&mut first);
    canonicalize_chat_connector_authority_seal(&mut reordered);
    assert_eq!(
        chat_connector_authority_sha256(&first).unwrap(),
        chat_connector_authority_sha256(&reordered).unwrap(),
    );
    let serialized = serde_json::to_string(&first).unwrap();
    assert!(!serialized.contains(raw_config_value));

    reordered.connectors[1].active_credential_revision =
        Some("33333333-3333-4333-8333-333333333333".into());
    assert_ne!(
        chat_connector_authority_sha256(&first).unwrap(),
        chat_connector_authority_sha256(&reordered).unwrap(),
    );
}
