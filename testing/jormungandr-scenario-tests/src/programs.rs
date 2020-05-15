use error_chain::ChainedError as _;
use std::path::PathBuf;
use tokio::process::Command;

/// internal function to prepare a bawawa `Command` for `jormungandr` and `jcli`
///
/// if the program could not be found in the $PATH or the current path then this
/// function will print the error reported by `bawawa` and then will `panic!` so
/// the tests are not executed.
pub async fn prepare_command(exe: PathBuf) -> Command {
    let cmd = Command::new(exe.display().to_string());

    check_command_version(Command::new(exe.display().to_string())).await;

    cmd
}

async fn check_command_version(mut cmd: Command) {
    use tokio::prelude::*;
    use tokio::process::Child as Process;

    let cmd = cmd.arg("--version");

    let exit_status = cmd
        .spawn()
        .expect("error initializing command")
        .await
        .expect("error running command");

    assert!(
        exit_status.success(),
        "cannot execute the command successfully '{:?}'",
        cmd
    );
}
