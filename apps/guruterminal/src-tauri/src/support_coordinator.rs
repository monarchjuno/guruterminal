use std::{
    future::pending,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};

use tokio::sync::Notify;

use crate::{
    app::CommandError,
    maintenance::{ActivityLease, MaintenanceBlocker, MaintenanceCoordinator},
};

#[derive(Clone, Debug, Default)]
pub struct ProviderSupportCoordinator {
    occupied: Arc<AtomicBool>,
    oauth: Arc<Mutex<Option<Weak<SupportCancellation>>>>,
    maintenance: MaintenanceCoordinator,
}

#[derive(Debug)]
pub struct ProviderSupportLease {
    occupied: Weak<AtomicBool>,
    oauth: Weak<Mutex<Option<Weak<SupportCancellation>>>>,
    cancellation: Option<Arc<SupportCancellation>>,
    _activity: ActivityLease,
}

#[derive(Debug, Default)]
struct SupportCancellation {
    cancelled: AtomicBool,
    notify: Notify,
    authorization_url: Mutex<Option<String>>,
}

impl SupportCancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

impl Drop for ProviderSupportLease {
    fn drop(&mut self) {
        if let (Some(oauth), Some(cancellation)) = (self.oauth.upgrade(), &self.cancellation) {
            let mut active = oauth
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some_and(|current| Arc::ptr_eq(&current, cancellation))
            {
                *active = None;
            }
        }
        if let Some(occupied) = self.occupied.upgrade() {
            occupied.store(false, Ordering::Release);
        }
    }
}

impl ProviderSupportCoordinator {
    pub fn new(maintenance: MaintenanceCoordinator) -> Self {
        Self {
            occupied: Arc::new(AtomicBool::new(false)),
            oauth: Arc::new(Mutex::new(None)),
            maintenance,
        }
    }

    pub fn try_acquire(&self) -> Result<ProviderSupportLease, CommandError> {
        self.try_acquire_inner(false)
    }

    pub fn try_acquire_oauth(&self) -> Result<ProviderSupportLease, CommandError> {
        self.try_acquire_inner(true)
    }

    fn try_acquire_inner(
        &self,
        cancellable_oauth: bool,
    ) -> Result<ProviderSupportLease, CommandError> {
        let activity = self.maintenance.admit_activity(MaintenanceBlocker {
            id: "provider-support".into(),
            kind: "provider_support".into(),
            label: "Provider connection or model discovery".into(),
        })?;
        self.occupied
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| {
                CommandError::conflict("another provider support operation is already active")
            })?;
        let cancellation = cancellable_oauth.then(|| Arc::new(SupportCancellation::default()));
        if let Some(cancellation) = &cancellation {
            *self
                .oauth
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::downgrade(cancellation));
        }
        Ok(ProviderSupportLease {
            occupied: Arc::downgrade(&self.occupied),
            oauth: Arc::downgrade(&self.oauth),
            cancellation,
            _activity: activity,
        })
    }

    pub fn cancel_oauth(&self) {
        if let Some(cancellation) = self
            .oauth
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
        {
            cancellation.cancel();
        }
    }

    pub fn oauth_authorization_url(&self) -> Option<String> {
        self.oauth
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
            .and_then(|cancellation| {
                cancellation
                    .authorization_url
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            })
    }
}

impl ProviderSupportLease {
    pub fn set_oauth_authorization_url(&self, url: String) -> Result<(), CommandError> {
        let cancellation = self.cancellation.as_ref().ok_or_else(|| {
            CommandError::internal("OAuth authorization URL has no active sign-in lease")
        })?;
        *cancellation
            .authorization_url
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(url);
        Ok(())
    }

    pub async fn cancelled(&self) {
        match &self.cancellation {
            Some(cancellation) => cancellation.cancelled().await,
            None => pending().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn provider_support_is_single_flight_and_raii_releases_every_exit() {
        let coordinator = ProviderSupportCoordinator::default();
        let lease = coordinator.try_acquire().unwrap();

        let conflict = coordinator.try_acquire().unwrap_err();
        assert_eq!(conflict.code, "conflict");
        assert!(conflict.message.contains("provider support operation"));

        drop(lease);
        let replacement = coordinator.try_acquire().unwrap();
        drop(replacement);
        coordinator.try_acquire().unwrap();
    }

    #[test]
    fn concurrent_provider_entrypoints_conflict_before_their_spawn_boundary() {
        let coordinator = ProviderSupportCoordinator::default();
        let active = coordinator.try_acquire().unwrap();
        let spawn_attempts = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|scope| {
            for operation in ["models", "configure", "connect"] {
                let coordinator = coordinator.clone();
                let spawn_attempts = spawn_attempts.clone();
                scope.spawn(move || match coordinator.try_acquire() {
                    Ok(_lease) => {
                        spawn_attempts.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(error) => {
                        assert_eq!(error.code, "conflict", "{operation}");
                    }
                });
            }
        });

        assert_eq!(spawn_attempts.load(Ordering::SeqCst), 0);
        drop(active);

        fn fail_after_admission(
            coordinator: &ProviderSupportCoordinator,
        ) -> Result<(), CommandError> {
            let _lease = coordinator.try_acquire()?;
            Err(CommandError::internal("synthetic support failure"))
        }
        assert!(fail_after_admission(&coordinator).is_err());
        coordinator.try_acquire().unwrap();
    }

    #[tokio::test]
    async fn oauth_cancellation_targets_only_the_active_oauth_lease() {
        let coordinator = ProviderSupportCoordinator::default();
        let oauth = coordinator.try_acquire_oauth().unwrap();
        oauth
            .set_oauth_authorization_url(
                "https://auth.openai.com/oauth/authorize?state=test".into(),
            )
            .unwrap();
        assert_eq!(
            coordinator.oauth_authorization_url().as_deref(),
            Some("https://auth.openai.com/oauth/authorize?state=test")
        );
        coordinator.cancel_oauth();
        tokio::time::timeout(std::time::Duration::from_millis(100), oauth.cancelled())
            .await
            .unwrap();
        drop(oauth);
        assert_eq!(coordinator.oauth_authorization_url(), None);

        coordinator.cancel_oauth();
        let ordinary = coordinator.try_acquire().unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), ordinary.cancelled())
                .await
                .is_err()
        );
    }

    #[test]
    fn update_maintenance_blocks_provider_admission_and_reports_active_support() {
        let maintenance = MaintenanceCoordinator::default();
        let coordinator = ProviderSupportCoordinator::new(maintenance.clone());
        let support = coordinator.try_acquire().unwrap();

        let blockers = maintenance.begin_update().unwrap_err();
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "provider_support");

        drop(support);
        let update = maintenance.begin_update().unwrap();
        let error = coordinator.try_acquire().unwrap_err();
        assert_eq!(error.code, "maintenance_active");
        drop(update);
        coordinator.try_acquire().unwrap();
    }
}
