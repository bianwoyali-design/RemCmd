/// Adds cwd reporting after the user's normal interactive startup completes.
///
/// The shell-specific branches preserve prompt variables and startup files so
/// prompt engines such as Starship keep their native initialization order. An
/// unsupported shell may reject this first line, but the independent ready line
/// still restores echo and leaves the terminal usable without cwd events.
pub(crate) fn install_command(ready_command: &str) -> String {
    format!(
        " stty -echo; if [[ -n ${{BASH_VERSION-}} ]]; then {BASH_INSTALL_COMMAND}; elif [[ -n ${{ZSH_VERSION-}} ]]; then {ZSH_INSTALL_COMMAND}; fi\r {ready_command}"
    )
}

const BASH_INSTALL_COMMAND: &str = r#"__remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; printf '\033]7;file://%s\007' "$PWD"; fi; }; if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a "* ]]; then PROMPT_COMMAND+=(__remcmd_report_cwd); else PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }__remcmd_report_cwd"; fi; __remcmd_last_cwd=$PWD"#;

const ZSH_INSTALL_COMMAND: &str = r#"autoload -Uz add-zsh-hook; __remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; builtin printf '\033]7;file://%s\007' "$PWD"; fi; }; add-zsh-hook chpwd __remcmd_report_cwd; add-zsh-hook precmd __remcmd_report_cwd; __remcmd_last_cwd=$PWD"#;

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn hooks_report_cwd_without_replacing_prompt_or_startup_files() {
        for command in [BASH_INSTALL_COMMAND, ZSH_INSTALL_COMMAND] {
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
        let combined = install_command("stty echo").replace('\r', "\n");
        assert_shell_syntax("/bin/bash", BASH_INSTALL_COMMAND);
        assert_shell_syntax("/bin/bash", &combined);

        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            assert_shell_syntax("/bin/zsh", ZSH_INSTALL_COMMAND);
            assert_shell_syntax("/bin/zsh", &combined);
        }
    }

    #[test]
    fn generated_hooks_report_directory_changes() {
        let bash_script = format!("{BASH_INSTALL_COMMAND}; cd /tmp; eval \"$PROMPT_COMMAND\"");
        assert_cwd_report("/bin/bash", &bash_script);

        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            let zsh_script = format!("{ZSH_INSTALL_COMMAND}; cd /tmp");
            assert_cwd_report("/bin/zsh", &zsh_script);
        }
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
