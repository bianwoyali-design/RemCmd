/// Adds cwd reporting after the user's normal interactive startup completes.
///
/// The shell-specific branches preserve prompt variables and startup files so
/// prompt engines such as Starship keep their native initialization order. An
/// unsupported POSIX-compatible shell skips both branches and continues to the
/// independent ready line without requiring utilities such as `stty`.
pub(crate) fn install_command(ready_command: &str) -> String {
    let bash_command = shell_single_quote(BASH_INSTALL_COMMAND);
    let zsh_command = shell_single_quote(ZSH_INSTALL_COMMAND);
    let ash_command = shell_single_quote(ASH_INSTALL_COMMAND);
    format!(
        " if [ -n \"${{BASH_VERSION-}}\" ]; then eval {bash_command}; fi\r if [ -n \"${{ZSH_VERSION-}}\" ]; then eval {zsh_command}; fi\r case \"${{0##*/}}\" in *ash) eval {ash_command};; esac\r {ready_command}"
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

const BASH_INSTALL_COMMAND: &str = r#"__remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; printf '\033]7;file://%s\007' "$PWD"; fi; }; if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a "* ]]; then PROMPT_COMMAND+=(__remcmd_report_cwd); else PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }__remcmd_report_cwd"; fi; __remcmd_last_cwd=$PWD"#;

const ZSH_INSTALL_COMMAND: &str = r#"autoload -Uz add-zsh-hook; __remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; builtin printf '\033]7;file://%s\007' "$PWD"; fi; }; add-zsh-hook chpwd __remcmd_report_cwd; add-zsh-hook precmd __remcmd_report_cwd; __remcmd_last_cwd=$PWD"#;

const ASH_INSTALL_COMMAND: &str = r#"__remcmd_report_cwd(){ if [ "${__remcmd_last_cwd-}" != "$PWD" ]; then __remcmd_last_cwd=$PWD; printf '\033]7;file://%s\007' "$PWD"; fi; }; cd(){ command cd "$@" && __remcmd_report_cwd; }; __remcmd_last_cwd=$PWD"#;

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn hooks_report_cwd_without_replacing_prompt_or_startup_files() {
        for command in [
            BASH_INSTALL_COMMAND,
            ZSH_INSTALL_COMMAND,
            ASH_INSTALL_COMMAND,
        ] {
            assert!(command.contains("7;file://%s"));
            assert!(!command.contains("PS1"));
            assert!(!command.contains("PROMPT="));
            assert!(!command.contains("RPROMPT"));
            assert!(!command.contains("starship"));
            assert!(!command.contains(".bashrc"));
            assert!(!command.contains(".zshrc"));
            assert!(!command.contains("exec "));
        }
    }

    #[test]
    fn generated_hooks_pass_native_syntax_checks() {
        let combined = install_command(":").replace('\r', "\n");
        assert_shell_syntax("/bin/bash", BASH_INSTALL_COMMAND);
        assert_shell_syntax("/bin/bash", &combined);

        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            assert_shell_syntax("/bin/zsh", ZSH_INSTALL_COMMAND);
            assert_shell_syntax("/bin/zsh", &combined);
        }
        assert_shell_syntax("/bin/sh", ASH_INSTALL_COMMAND);
    }

    #[test]
    fn generated_hooks_report_directory_changes() {
        let bash_script = format!("{BASH_INSTALL_COMMAND}; cd /tmp; eval \"$PROMPT_COMMAND\"");
        assert_cwd_report("/bin/bash", &bash_script);

        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            let zsh_script = format!("{ZSH_INSTALL_COMMAND}; cd /tmp");
            assert_cwd_report("/bin/zsh", &zsh_script);
        }
        assert_cwd_report("/bin/sh", &format!("{ASH_INSTALL_COMMAND}; cd /tmp"));
    }

    #[test]
    fn generated_install_command_dispatches_and_reaches_ready_line() {
        let command = install_command("printf 'remcmd-ready'").replace('\r', "\n");

        assert_install_command("/bin/bash", &command);
        assert_cwd_report(
            "/bin/bash",
            &format!("{command}; cd /tmp; eval \"$PROMPT_COMMAND\""),
        );
        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            assert_install_command("/bin/zsh", &command);
            assert_cwd_report("/bin/zsh", &format!("{command}; cd /tmp"));
        }
    }

    #[test]
    fn generated_install_command_dispatches_to_ash_hook() {
        let command = install_command(":").replace('\r', "\n");
        let script = format!("unset BASH_VERSION ZSH_VERSION; {command}; cd /tmp");
        let output = Command::new("/bin/sh")
            .args(["-c", &script, "ash"])
            .output()
            .expect("POSIX shell should start");

        assert!(output.status.success());
        assert!(
            output
                .stdout
                .windows(b"7;file:///tmp".len())
                .any(|window| window == b"7;file:///tmp")
        );
    }

    #[test]
    fn unsupported_posix_shell_reaches_ready_line_without_stty() {
        let command = install_command("printf 'remcmd-ready'").replace('\r', "\n");
        let script = format!("unset BASH_VERSION ZSH_VERSION; {command}");
        let output = Command::new("/bin/sh")
            .args(["-c", &script, "sh"])
            .output()
            .expect("POSIX shell should start");

        assert!(output.status.success());
        assert!(output.stdout.ends_with(b"remcmd-ready"));
        assert!(!command.contains("stty"));
        assert!(command.starts_with(" if ["));
    }

    fn assert_shell_syntax(shell: &str, script: &str) {
        let output = Command::new(shell)
            .args(["-n", "-c", script])
            .output()
            .expect("syntax-check shell should start");
        assert!(
            output.status.success(),
            "{shell} rejected generated shell integration: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn assert_cwd_report(shell: &str, script: &str) {
        let output = Command::new(shell)
            .args(["-f", "-c", script])
            .output()
            .expect("cwd-report shell should start");
        assert!(
            output.status.success(),
            "{shell} rejected cwd hook: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output
                .stdout
                .windows(b"7;file:///tmp".len())
                .any(|window| window == b"7;file:///tmp"),
            "{shell} did not report the directory change: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    fn assert_install_command(shell: &str, script: &str) {
        let output = Command::new(shell)
            .args(["-f", "-c", script])
            .output()
            .expect("integrated shell should start");
        assert!(
            output.status.success(),
            "{shell} rejected combined integration: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stdout.ends_with(b"remcmd-ready"),
            "{shell} did not reach the independent ready line: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
