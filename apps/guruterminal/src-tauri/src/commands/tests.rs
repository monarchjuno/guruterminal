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
