//! Layering tripwires complement the behavioral delivery/restart tests.
use std::path::Path;

fn source(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name)).unwrap()
}

#[test]
fn session_primitives_do_not_depend_on_business_policy() {
    let runtime = source("session_runtime.rs");
    for module in ["tmux", "restart", "shell_state", "scheduler", "voice"] {
        assert!(
            !runtime.contains(&format!("crate::{module}::")),
            "runtime depends on {module}"
        );
    }
    for module in [
        "tmux.rs",
        "shell_state.rs",
        "prompt_delivery.rs",
        "voice.rs",
        "pty.rs",
    ] {
        assert!(
            !source(module).contains("crate::restart::"),
            "{module} depends on restart policy"
        );
    }
}
