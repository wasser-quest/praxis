use crate::{CallBinding, ProviderContinuation, WireContinuation, WireLimits, WireValidationError};

fn binding() -> CallBinding {
    CallBinding {
        provider: "worker".into(),
        worker_generation: 4,
        call_id: [7; 16],
    }
}

#[test]
fn host_carries_worker_state_without_parsing_and_redacts_it() {
    let limits = WireLimits::default();
    let wire =
        WireContinuation::new(binding(), b"PRIVATE opaque not JSON".to_vec(), limits).unwrap();
    let state = ProviderContinuation::Worker { state: wire };
    assert_eq!(state.estimated_tokens(), 22 / 3);
    assert_eq!(format!("{state:?}"), "ProviderContinuation { .. }");
    let cloned = state.clone();
    let returned = WireContinuation::from_provider(cloned, binding(), limits).unwrap();
    assert_eq!(returned.binding(), &binding());
    assert_eq!(
        returned.payload(&binding(), limits).unwrap(),
        b"PRIVATE opaque not JSON"
    );
    assert!(matches!(
        returned.to_provider(&binding(), limits),
        Err(WireValidationError::InvalidContinuation)
    ));
}

#[test]
fn opaque_state_cannot_be_rebound_or_exceed_a_smaller_budget() {
    for mode in ["provider", "generation", "call", "limit"] {
        let wire =
            WireContinuation::new(binding(), b"PRIVATE".to_vec(), WireLimits::default()).unwrap();
        let mut expected = binding();
        let mut limits = WireLimits::default();
        match mode {
            "provider" => expected.provider = "foreign".into(),
            "generation" => expected.worker_generation += 1,
            "call" => expected.call_id = [8; 16],
            _ => limits.continuation_bytes = 2,
        }
        let error = WireContinuation::from_provider(
            ProviderContinuation::Worker { state: wire },
            expected,
            limits,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            WireValidationError::WrongBinding | WireValidationError::ContinuationTooLarge
        ));
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
}
