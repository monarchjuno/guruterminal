use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

use serde::Serialize;

use crate::app::CommandError;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MaintenanceBlocker {
    pub id: String,
    pub kind: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceActivityKind {
    GuruMutation,
    GuruTransfer,
    GuruDeletion,
    ChatMutation,
    MemoryMutation,
    MarketplaceConfiguration,
    MarketplaceCredential,
}

impl MaintenanceActivityKind {
    const fn kind(self) -> &'static str {
        match self {
            Self::GuruMutation => "guru_mutation",
            Self::GuruTransfer => "guru_transfer",
            Self::GuruDeletion => "guru_deletion",
            Self::ChatMutation => "chat_mutation",
            Self::MemoryMutation => "memory_mutation",
            Self::MarketplaceConfiguration => "marketplace_configuration",
            Self::MarketplaceCredential => "marketplace_credential",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::GuruMutation => "Guru change",
            Self::GuruTransfer => "Guru Memory transfer",
            Self::GuruDeletion => "Guru deletion",
            Self::ChatMutation => "Chat change",
            Self::MemoryMutation => "Memory change",
            Self::MarketplaceConfiguration => "Marketplace configuration change",
            Self::MarketplaceCredential => "Marketplace credential change",
        }
    }
}

#[derive(Debug, Default)]
struct MaintenanceState {
    installing_update: bool,
    next_activity_id: u64,
    activities: BTreeMap<u64, MaintenanceBlocker>,
}

#[derive(Clone, Debug, Default)]
pub struct MaintenanceCoordinator {
    inner: Arc<Mutex<MaintenanceState>>,
}

#[derive(Debug)]
pub struct ActivityLease {
    activity_id: u64,
    coordinator: Weak<Mutex<MaintenanceState>>,
}

impl Drop for ActivityLease {
    fn drop(&mut self) {
        let Some(coordinator) = self.coordinator.upgrade() else {
            return;
        };
        if let Ok(mut state) = coordinator.lock() {
            state.activities.remove(&self.activity_id);
        };
    }
}

#[derive(Debug)]
pub struct MaintenanceLease {
    coordinator: Weak<Mutex<MaintenanceState>>,
}

impl Drop for MaintenanceLease {
    fn drop(&mut self) {
        let Some(coordinator) = self.coordinator.upgrade() else {
            return;
        };
        if let Ok(mut state) = coordinator.lock() {
            state.installing_update = false;
        };
    }
}

impl MaintenanceCoordinator {
    pub fn admit_kind(&self, kind: MaintenanceActivityKind) -> Result<ActivityLease, CommandError> {
        self.admit(|activity_id| MaintenanceBlocker {
            id: format!("native-{activity_id}"),
            kind: kind.kind().into(),
            label: kind.label().into(),
        })
    }

    pub fn admit_activity(
        &self,
        blocker: MaintenanceBlocker,
    ) -> Result<ActivityLease, CommandError> {
        self.admit(|_| blocker)
    }

    fn admit(
        &self,
        blocker: impl FnOnce(u64) -> MaintenanceBlocker,
    ) -> Result<ActivityLease, CommandError> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| CommandError::internal("maintenance coordinator lock was poisoned"))?;
        if state.installing_update {
            return Err(CommandError::new(
                "maintenance_active",
                "Guru Terminal is installing an update; wait for the restart before starting new work",
            ));
        }
        state.next_activity_id = state.next_activity_id.wrapping_add(1).max(1);
        let activity_id = state.next_activity_id;
        state.activities.insert(activity_id, blocker(activity_id));
        Ok(ActivityLease {
            activity_id,
            coordinator: Arc::downgrade(&self.inner),
        })
    }

    pub fn begin_update(&self) -> Result<MaintenanceLease, Vec<MaintenanceBlocker>> {
        let mut state = self
            .inner
            .lock()
            .expect("maintenance coordinator lock was poisoned");
        if state.installing_update {
            return Err(vec![MaintenanceBlocker {
                id: "update-install".into(),
                kind: "update".into(),
                label: "Another update installation is already active".into(),
            }]);
        }
        if !state.activities.is_empty() {
            return Err(state.activities.values().cloned().collect());
        }
        state.installing_update = true;
        Ok(MaintenanceLease {
            coordinator: Arc::downgrade(&self.inner),
        })
    }

    #[cfg(test)]
    pub fn is_installing_update(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.installing_update)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn blocker(id: &str, kind: &str) -> MaintenanceBlocker {
        MaintenanceBlocker {
            id: id.into(),
            kind: kind.into(),
            label: format!("active {kind}"),
        }
    }

    #[test]
    fn update_and_activity_admission_are_linearized_by_one_gate() {
        let coordinator = MaintenanceCoordinator::default();
        let activity = coordinator
            .admit_activity(blocker("run-1", "chat"))
            .unwrap();

        let blockers = coordinator.begin_update().unwrap_err();
        assert_eq!(blockers, vec![blocker("run-1", "chat")]);

        drop(activity);
        let maintenance = coordinator.begin_update().unwrap();
        assert!(coordinator.is_installing_update());
        let error = coordinator
            .admit_activity(blocker("run-2", "memory mutation"))
            .unwrap_err();
        assert_eq!(error.code, "maintenance_active");

        drop(maintenance);
        assert!(!coordinator.is_installing_update());
        coordinator
            .admit_activity(blocker("run-3", "chat"))
            .unwrap();
    }

    #[test]
    fn update_lease_blocks_representative_native_command_kinds() {
        for kind in [
            MaintenanceActivityKind::GuruDeletion,
            MaintenanceActivityKind::MemoryMutation,
            MaintenanceActivityKind::MarketplaceCredential,
            MaintenanceActivityKind::MarketplaceConfiguration,
        ] {
            let coordinator = MaintenanceCoordinator::default();
            let update = coordinator.begin_update().unwrap();
            let contender = coordinator.clone();
            let error = thread::spawn(move || contender.admit_kind(kind).unwrap_err())
                .join()
                .unwrap();
            assert_eq!(error.code, "maintenance_active");
            drop(update);
        }
    }

    #[test]
    fn representative_native_command_kinds_block_update_lease() {
        for kind in [
            MaintenanceActivityKind::GuruDeletion,
            MaintenanceActivityKind::MemoryMutation,
            MaintenanceActivityKind::MarketplaceCredential,
            MaintenanceActivityKind::MarketplaceConfiguration,
        ] {
            let coordinator = MaintenanceCoordinator::default();
            let activity = coordinator.admit_kind(kind).unwrap();
            let contender = coordinator.clone();
            let blockers = thread::spawn(move || contender.begin_update().unwrap_err())
                .join()
                .unwrap();
            assert_eq!(blockers.len(), 1);
            assert_eq!(blockers[0].kind, kind.kind());
            assert_eq!(blockers[0].label, kind.label());
            drop(activity);
        }
    }
}
