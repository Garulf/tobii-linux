//! `tobii-linux setup`: the GTK setup app, a separate program, run with the same arguments.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const SETUP_PROGRAM: &str = "tobii-linux-setup";

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// `tobii-linux-setup` next to this executable, else on `PATH`.
pub fn find_setup(current_exe: Option<PathBuf>, path: Option<OsString>) -> Option<PathBuf> {
    let beside = current_exe.and_then(|exe| exe.parent().map(|dir| dir.join(SETUP_PROGRAM)));
    let on_path = path.into_iter().flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>());
    beside.into_iter().chain(on_path.map(|dir| dir.join(SETUP_PROGRAM))).find(|candidate| executable(candidate))
}

pub fn main(args: &[String]) -> ExitCode {
    let current = std::env::current_exe().ok();
    let Some(program) = find_setup(current.clone(), std::env::var_os("PATH")) else {
        let beside =
            current.as_deref().and_then(Path::parent).map_or("this program".into(), |d| d.display().to_string());
        let message =
            format!("{SETUP_PROGRAM} was not found in {beside} or on PATH; install the setup app to use setup");
        crate::cli::report_error(&crate::error::Error::Runtime(message));
        return ExitCode::FAILURE;
    };
    let error = std::process::Command::new(&program).args(args).exec();
    crate::cli::report_error(&crate::error::Error::Runtime(format!("{}: {error}", program.display())));
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_program_beside_the_binary_comes_first() {
        let root = std::env::temp_dir().join(format!("tobii-linux-setup-test-{}", std::process::id()));
        let (bin, other) = (root.join("bin"), root.join("other"));
        for dir in [&bin, &other] {
            std::fs::create_dir_all(dir).unwrap();
            let program = dir.join(SETUP_PROGRAM);
            std::fs::write(&program, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([&other]).unwrap();
        assert_eq!(find_setup(Some(bin.join("tobii-linux")), Some(path.clone())), Some(bin.join(SETUP_PROGRAM)));
        assert_eq!(find_setup(Some(root.join("tobii-linux")), Some(path)), Some(other.join(SETUP_PROGRAM)));
        assert_eq!(find_setup(Some(root.join("tobii-linux")), None), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
