//! Bind-path contract: every wrapper must be directly executable (the
//! Hyprland bind execs it, no interpreter prefix) and must self-wrap into a
//! kitty popup when launched outside one (live soak caught launch/center
//! silently never starting because the exec bit was lost).

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

const WRAPPERS: &[(&str, &str)] = &[
    ("flex-power.sh", "menu"),
    ("flex-shot.sh", "menu"),
    ("flex-theme.sh", "menu"),
    ("flex-wifi.sh", "menu"),
    ("flex-launch.sh", "menu-wide"),
    ("flex-clip.sh", "menu-wide"),
    ("flex-center.sh", "menu-wide"),
    ("flex-wallpaper.sh", "menu-wide"),
];

#[test]
fn wrappers_are_executable_and_popup_wrapped() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wrappers");
    for (name, variant) in WRAPPERS {
        let path = dir.join(name);
        let mode = std::fs::metadata(&path)
            .unwrap_or_else(|_| panic!("wrapper present: {name}"))
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "{name} must be executable (bind execs it directly): {:o}",
            mode & 0o777
        );
        let body = std::fs::read_to_string(&path).expect("wrapper readable");
        assert!(
            body.contains(&format!(
                "exec \"$HOME/.config/scripts/popup.sh\" {variant} \"$0\" \"$@\""
            )),
            "{name} must re-exec into popup.sh {variant} when POPUP_KITTY is unset",
        );
    }
}
