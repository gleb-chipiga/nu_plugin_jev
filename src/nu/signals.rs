//! Registers interrupt cancellation before checking current Nushell signal state.

use nu_plugin::EngineInterface;
use nu_protocol::{Handler, HandlerGuard, LabeledError, SignalAction, Signals, Span};

use crate::api::cancel::{CancelHandle, CancelSignal};

/// Keeps interrupt protection and local cancellation alive for an operation or returned stream.
/// Engine interrupts are shared; this cancellation pair remains invocation-local.
pub(crate) struct InterruptRegistration {
    /// Unregisters the interrupt handler only when protected work ends.
    pub(crate) guard: HandlerGuard,
    /// Cancels only this invocation without resetting shared engine signals.
    pub(crate) cancel: CancelHandle,
    /// Observes interrupt or local cancellation in asynchronous work.
    pub(crate) signal: CancelSignal,
}

/// Registers first, then checks signal state so an interrupt cannot fall between those steps.
/// Uses the SDK's shared signal state but creates a separate cancellation pair for this caller.
pub(crate) fn register_interrupt(
    engine: &EngineInterface,
    span: Span,
) -> Result<InterruptRegistration, LabeledError> {
    register_with(engine.signals(), span, |handler| {
        engine
            .register_signal_handler(handler)
            .map_err(LabeledError::from)
    })
}

/// Shares the exact registration order with deterministic signal-boundary tests.
pub(super) fn register_with(
    signals: &Signals,
    span: Span,
    register: impl FnOnce(Handler) -> Result<HandlerGuard, LabeledError>,
) -> Result<InterruptRegistration, LabeledError> {
    let (cancel, signal) = CancelHandle::new();
    let handler = cancel.clone();
    // The SDK runs handlers on its protocol reader thread. Only notify watch subscribers here;
    // waiting for HTTP or joining tasks would stop the reader from processing further messages.
    let guard = register(Box::new(move |action| {
        // Reset clears Nu's shared signal state, but must never revive cancelled local work.
        if action == SignalAction::Interrupt {
            handler.cancel();
        }
    }))?;
    // nu-plugin 0.116 updates Signals before invoking handlers. This post-registration check
    // catches earlier interrupts; later ones reach the handler. Checking first leaves a gap.
    // On check failure, guard drops and unregisters the handler without starting work.
    signals.check(&span).map_err(LabeledError::from)?;
    Ok(InterruptRegistration {
        guard,
        cancel,
        signal,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicBool};

    use nu_protocol::{Handlers, LabeledError, ShellError, SignalAction, Signals, Span};

    use super::register_with;

    /// Detects interrupts at each registration boundary before protected work can start.
    #[test]
    fn interrupts_before_and_during_registration_prevent_work() {
        for boundary in [0, 1, 2] {
            let signals = Signals::new(Arc::new(AtomicBool::new(false)));
            let handlers = Handlers::new();
            if boundary == 0 {
                signals.trigger();
            }
            let result = register_with(&signals, Span::test_data(), |handler| {
                if boundary == 1 {
                    signals.trigger();
                    handlers.run(SignalAction::Interrupt);
                }
                let guard = handlers.register(handler)?;
                if boundary == 2 {
                    signals.trigger();
                    handlers.run(SignalAction::Interrupt);
                }
                Ok(guard)
            });
            assert!(result.is_err(), "missed registration boundary {boundary}");
        }
    }

    /// Later interrupts wake local cancellation and Reset cannot revive that operation.
    #[test]
    fn registered_interrupt_is_remembered_after_reset() {
        let signals = Signals::new(Arc::new(AtomicBool::new(false)));
        let handlers = Handlers::new();
        let registration = register_with(&signals, Span::test_data(), |handler| {
            handlers.register(handler).map_err(LabeledError::from)
        })
        .unwrap();
        assert!(!registration.signal.is_cancelled());
        signals.trigger();
        handlers.run(SignalAction::Interrupt);
        assert!(registration.signal.is_cancelled());
        signals.reset();
        handlers.run(SignalAction::Reset);
        assert!(registration.signal.is_cancelled());
    }

    /// Guard ownership survives helper return and ends with its protected operation.
    #[test]
    fn dropping_registration_removes_its_handler() {
        let handlers = Handlers::new();
        let registration = register_with(&Signals::empty(), Span::test_data(), |handler| {
            handlers.register(handler).map_err(LabeledError::from)
        })
        .unwrap();
        let cancel = registration.cancel.clone();
        let signal = registration.signal.clone();
        drop(registration);
        handlers.run(SignalAction::Interrupt);
        assert!(!signal.is_cancelled());
        drop(cancel);
    }

    /// Failed registration or a failed post-registration check never starts protected work.
    #[test]
    fn registration_and_check_errors_are_propagated() {
        let result = register_with(&Signals::empty(), Span::test_data(), |_| {
            Err(LabeledError::from(ShellError::Interrupted {
                span: Span::test_data(),
            }))
        });
        assert!(result.is_err());
        let signals = Signals::new(Arc::new(AtomicBool::new(true)));
        let handlers = Handlers::new();
        let mut captured_handler = None;
        let result = register_with(&signals, Span::test_data(), |handler| {
            let handler = Arc::new(handler);
            captured_handler = Some(Arc::downgrade(&handler));
            handlers
                .register(Box::new(move |action| handler(action)))
                .map_err(LabeledError::from)
        });
        assert!(result.is_err());
        assert!(captured_handler.unwrap().upgrade().is_none());
    }
}
