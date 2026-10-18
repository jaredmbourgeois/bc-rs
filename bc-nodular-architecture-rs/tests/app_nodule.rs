use bc_nodular_architecture::{
    AppIdentifier, AppInput, AppInputType, AppRunState, AppState, Device, NodeResult,
    OperatingSystem, OperatingSystemVersion, receive_app_input,
};

#[test]
fn operating_system_version_from_string() {
    let version = "14.2.1".parse::<OperatingSystemVersion>().unwrap();

    assert_eq!((version.major, version.minor, version.patch), (14, 2, 1));
}

#[test]
fn operating_system_version_from_string_major_only() {
    let version = "10".parse::<OperatingSystemVersion>().unwrap();

    assert_eq!((version.major, version.minor, version.patch), (10, 0, 0));
}

#[test]
fn operating_system_version_from_string_major_minor() {
    let version = "10.15".parse::<OperatingSystemVersion>().unwrap();

    assert_eq!((version.major, version.minor, version.patch), (10, 15, 0));
}

#[test]
fn operating_system_version_to_string() {
    let version = OperatingSystemVersion {
        major: 14,
        minor: 2,
        patch: 1,
    };

    assert_eq!(version.to_string(), "14.2.1");
}

#[test]
fn operating_system_version_comparison() {
    let major = OperatingSystemVersion {
        major: 14,
        minor: 0,
        patch: 0,
    };
    let minor = OperatingSystemVersion { minor: 2, ..major };
    let patch = OperatingSystemVersion { patch: 1, ..minor };
    let next_major = OperatingSystemVersion {
        major: 15,
        minor: 0,
        patch: 0,
    };

    assert!(major < minor);
    assert!(minor < patch);
    assert!(patch < next_major);
    assert!(major < next_major);
    assert!(minor >= major);
    // The source asserts both directions and reflexivity because its operators
    // are hand-written; Rust derives them, so these pin the derive.
    assert!(minor > major);
    assert!(major <= major);
    assert!(major >= major);
    assert!(next_major > major);
}

#[test]
fn operating_system_version_equality() {
    let version = OperatingSystemVersion {
        major: 14,
        minor: 2,
        patch: 1,
    };
    let equal = OperatingSystemVersion { ..version };
    let different = OperatingSystemVersion {
        patch: 0,
        ..version
    };

    assert_eq!(version, equal);
    assert_ne!(version, different);
}

#[test]
fn operating_system_version_rejects_invalid_strings() {
    for invalid in [
        "",
        ".",
        "14.",
        ".2",
        "14..1",
        "-1",
        "1.-2",
        "v14",
        "1 2",
        "1.2.3.4",
        "4294967296",
    ] {
        assert!(
            invalid.parse::<OperatingSystemVersion>().is_err(),
            "accepted {invalid:?}"
        );
    }
}

fn app_state(run_state: AppRunState) -> AppState {
    AppState {
        device: Device {
            identifier: "id".to_owned(),
            model: "model".to_owned(),
            name: "name".to_owned(),
        },
        identifier: AppIdentifier::new("com.test"),
        name: "TestApp".to_owned(),
        operating_system: OperatingSystem {
            name: "macOS".to_owned(),
            version: OperatingSystemVersion {
                major: 14,
                minor: 0,
                patch: 0,
            },
        },
        run_state,
    }
}

fn app_input(input_type: AppInputType, seq: u64) -> AppInput {
    AppInput {
        app_identifier: AppIdentifier::new("com.test"),
        seq,
        input_type,
        exit_code: 0,
    }
}

#[test]
fn app_nodule_receive_app_input_started() {
    let mut state = app_state(AppRunState::NotStarted);

    let result = receive_app_input(&app_input(AppInputType::Started, 1), &mut state);

    assert!(matches!(result, NodeResult::None));
    assert_eq!(state.run_state, AppRunState::Running);
}

#[test]
fn app_nodule_receive_app_input_stopped() {
    let mut state = app_state(AppRunState::Running);

    let result = receive_app_input(&app_input(AppInputType::Stopped, 2), &mut state);

    assert!(matches!(result, NodeResult::None));
    assert_eq!(state.run_state, AppRunState::WillTerminate);
}

#[test]
fn app_nodule_receive_app_input_terminated() {
    let mut state = app_state(AppRunState::WillTerminate);

    let result = receive_app_input(&app_input(AppInputType::Terminated, 3), &mut state);

    assert!(matches!(result, NodeResult::None));
    assert_eq!(state.run_state, AppRunState::Terminated);
}

#[test]
fn device_defaults_to_empty_source_values() {
    let device = Device::default();

    assert_eq!(device.identifier, "");
    assert_eq!(device.model, "");
    assert_eq!(device.name, "");
}

#[test]
fn device_equality() {
    let device = Device {
        identifier: "id1".to_owned(),
        model: "model1".to_owned(),
        name: "name1".to_owned(),
    };
    let equal = device.clone();
    let different = Device {
        identifier: "id2".to_owned(),
        ..device.clone()
    };

    assert_eq!(device, equal);
    assert_ne!(device, different);
}

#[test]
fn app_input_sequence() {
    let input = app_input(AppInputType::Started, 42);

    assert_eq!(input.sequence(), 42);
}

#[test]
fn app_input_equality() {
    let input = app_input(AppInputType::Started, 1);
    let equal = input.clone();
    let different = app_input(AppInputType::Started, 2);

    assert_eq!(input, equal);
    assert_ne!(input, different);
}

#[test]
fn app_state_equality() {
    let state = app_state(AppRunState::NotStarted);
    let equal = state.clone();
    let different = app_state(AppRunState::Running);

    assert_eq!(state, equal);
    assert_ne!(state, different);
}

#[test]
fn app_input_defaults_match_the_source_aggregate() {
    let input = AppInput::default();

    assert_eq!(input.app_identifier.as_str(), "");
    assert_eq!(input.seq, 0);
    assert_eq!(input.input_type, AppInputType::Started);
    assert_eq!(input.exit_code, 0);
}

#[test]
fn app_state_defaults_match_the_source_aggregate() {
    let state = AppState::default();

    assert_eq!(state.identifier.as_str(), "");
    assert_eq!(state.name, "");
    assert_eq!(
        state.operating_system.version,
        OperatingSystemVersion::default()
    );
    assert_eq!(state.run_state, AppRunState::NotStarted);
}
