use peercast_core::errors::QuitReason;

#[test]
fn test_p11_quit_code_exact_preservation() {
    let test_codes = [
        (1000, "Normal disconnect"),
        (1001, "Channel skip / bandwidth exceeded"),
        (1003, "Relay unavailable / node full"),
        (1008, "Channel off-air"),
        (1009, "Servent shutting down"),
        (3000, "Socket read error (3000)"),
        (4000, "Socket write error (4000)"),
        (4042, "Unknown quit code"),
        (9999, "Unknown quit code"),
    ];

    for (code, expected_desc) in test_codes {
        let reason = QuitReason::from_raw(code);
        assert_eq!(
            reason.raw(),
            code,
            "Raw wire integer must be preserved exactly"
        );
        assert_eq!(reason.description(), expected_desc);
        // Display and Debug formatting must not panic
        let disp = format!("{}", reason);
        let dbg = format!("{:?}", reason);
        assert!(!disp.is_empty());
        assert!(!dbg.is_empty());
    }
}
