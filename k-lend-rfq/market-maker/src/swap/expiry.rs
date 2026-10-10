//! Expiry of fills awaiting the user's signature: a fill whose deadline
//! passes before the user signs is aborted and its reservations released.

use std::time::Instant;

use crate::{
    error::MakerError,
    transactions::{
        confirm::Retry,
        coordinator::{Coordinator, Event},
        steps::{StepId, StepState},
    },
};

impl Coordinator {
    /// Schedules an `Event::Expire(id)` at `expires_at`. The timer is dropped
    /// when the coordinator is cancelled, so shutdown does not wait for it.
    pub fn spawn_expiry(&self, id: StepId, expires_at: Instant) {
        let events = self.runtime.events.clone();
        let cancel = self.runtime.cancel.clone();
        self.runtime.tasks.spawn(async move {
            let deadline = tokio::time::sleep_until(expires_at.into());
            if cancel.run_until_cancelled(deadline).await.is_some() {
                let _ = events.send(Event::Expire(id));
            }
        });
    }

    /// Aborts step `id` with `MakerError::ReservationExpired` and
    /// `Retry::Fail` if it is still `AwaitingSignature` and its fill deadline
    /// has passed. A step that was signed, aborted or rescheduled in the
    /// meantime is left alone, so a stale timer is harmless.
    pub async fn on_expire(&mut self, id: StepId) {
        let expired = self.steps.get(id).is_some_and(|step| {
            step.state == StepState::AwaitingSignature
                && step
                    .fill
                    .as_ref()
                    .and_then(|fill| fill.expires_at)
                    .is_some_and(|deadline| Instant::now() >= deadline)
        });
        if expired {
            self.abort(id, MakerError::ReservationExpired { step: id }, Retry::Fail)
                .await;
        }
    }
}
