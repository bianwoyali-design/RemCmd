#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellKind {
    Bash,
    Zsh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShellIntegration {
    kind: ShellKind,
}

impl ShellIntegration {
    pub(crate) fn detect(output: &[u8]) -> Option<Self> {
        let path = std::str::from_utf8(output).ok()?.trim();
        if path.is_empty()
            || path.len() > 256
            || !path.starts_with('/')
            || !path
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/_+.-".contains(&byte))
        {
            return None;
        }

        let kind = match path.rsplit('/').next()? {
            "bash" => ShellKind::Bash,
            "zsh" => ShellKind::Zsh,
            _ => return None,
        };

        Some(Self { kind })
    }

    /// Adds cwd reporting after the user's normal interactive startup completes.
    ///
    /// These hooks deliberately preserve prompt variables and startup files so
    /// prompt engines such as Starship keep their native initialization order.
    pub(crate) fn install_command(&self) -> &'static str {
        match self.kind {
            ShellKind::Bash => BASH_INSTALL_COMMAND,
            ShellKind::Zsh => ZSH_INSTALL_COMMAND,
        }
    }
}

const BASH_INSTALL_COMMAND: &str = r#"__remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; printf '\033]7;file://%s\007' "$PWD"; fi; }; if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a "* ]]; then PROMPT_COMMAND+=(__remcmd_report_cwd); else PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }__remcmd_report_cwd"; fi"#;

const ZSH_INSTALL_COMMAND: &str = r#"autoload -Uz add-zsh-hook; __remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; builtin printf '\033]7;file://%s\007' "$PWD"; fi; }; add-zsh-hook chpwd __remcmd_report_cwd; add-zsh-hook precmd __remcmd_report_cwd"#;

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn detects_only_supported_absolute_shell_paths() {
        assert_eq!(
            ShellIntegration::detect(b"/bin/bash\n").map(|shell| shell.kind),
            Some(ShellKind::Bash)
        );
        assert_eq!(
            ShellIntegration::detect(b"/usr/local/bin/zsh").map(|shell| shell.kind),
            Some(ShellKind::Zsh)
        );
        assert!(ShellIntegration::detect(b"/usr/bin/fish").is_none());
        assert!(ShellIntegration::detect(b"bash").is_none());
        assert!(ShellIntegration::detect(b"/bin/bash; touch /tmp/x").is_none());
    }

    #[test]
    fn hooks_report_cwd_without_replacing_prompt_or_startup_files() {
        for shell in [
            ShellIntegration::detect(b"/bin/bash").unwrap(),
            ShellIntegration::detect(b"/bin/zsh").unwrap(),
        ] {
            let command = shell.install_command();
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
        let bash = ShellIntegration::detect(b"/bin/bash").unwrap();
        assert_shell_syntax("/bin/bash", bash.install_command());

        let zsh = ShellIntegration::detect(b"/bin/zsh").unwrap();
        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            assert_shell_syntax("/bin/zsh", zsh.install_command());
        }
    }

    #[test]
    fn generated_hooks_report_directory_changes() {
        let bash = ShellIntegration::detect(b"/bin/bash").unwrap();
        let bash_script = format!(
            "{}; cd /tmp; eval \"$PROMPT_COMMAND\"",
            bash.install_command()
        );
        assert_cwd_report("/bin/bash", &bash_script);

        let zsh = ShellIntegration::detect(b"/bin/zsh").unwrap();
        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            let zsh_script = format!("{}; cd /tmp", zsh.install_command());
            assert_cwd_report("/bin/zsh", &zsh_script);
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
                .any(|window| { window == b"7;file:///tmp" }),
            "{shell} did not report the directory change: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
