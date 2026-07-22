#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellKind {
    Bash,
    Zsh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShellIntegration {
    path: String,
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

        Some(Self {
            path: path.to_owned(),
            kind,
        })
    }

    pub(crate) fn launch_command(&self) -> String {
        match self.kind {
            ShellKind::Bash => bash_launch_command(&self.path),
            ShellKind::Zsh => zsh_launch_command(&self.path),
        }
    }
}

fn bash_launch_command(shell_path: &str) -> String {
    format!(
        r#"REMCMD_SHELL_PATH='{shell_path}'
REMCMD_ORIGINAL_ENV=${{ENV-}}
export REMCMD_SHELL_PATH REMCMD_ORIGINAL_ENV
if ! REMCMD_INTEGRATION_DIR=$(mktemp -d "${{TMPDIR:-/tmp}}/remcmd-shell.XXXXXX"); then
    exec "$REMCMD_SHELL_PATH" -l
fi
export REMCMD_INTEGRATION_DIR
REMCMD_ENV="$REMCMD_INTEGRATION_DIR/bash-env"
if ! command cat >"$REMCMD_ENV" <<'REMCMD_BASH_ENV'
if [[ -n ${{REMCMD_ORIGINAL_ENV-}} ]]; then
    export ENV=$REMCMD_ORIGINAL_ENV
else
    unset ENV
fi
unset REMCMD_ORIGINAL_ENV
set +o posix
shopt -u inherit_errexit 2>/dev/null || true

if shopt -q login_shell; then
    [[ -r /etc/profile ]] && source /etc/profile
    for __remcmd_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [[ -r $__remcmd_profile ]]; then
            source "$__remcmd_profile"
            break
        fi
    done
else
    for __remcmd_bashrc in /etc/bash.bashrc /etc/bash/bashrc /etc/bashrc; do
        if [[ -r $__remcmd_bashrc ]]; then
            source "$__remcmd_bashrc"
            break
        fi
    done
    [[ -r $HOME/.bashrc ]] && source "$HOME/.bashrc"
fi
unset __remcmd_profile __remcmd_bashrc

__remcmd_report_cwd() {{
    if [[ ${{__remcmd_last_cwd-}} != "$PWD" ]]; then
        __remcmd_last_cwd=$PWD
        printf '\033]7;file://%s\007' "$PWD"
    fi
}}

if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a "* ]]; then
    PROMPT_COMMAND+=(__remcmd_report_cwd)
elif [[ ";${{PROMPT_COMMAND-}};" != *";__remcmd_report_cwd;"* ]]; then
    PROMPT_COMMAND="${{PROMPT_COMMAND:+$PROMPT_COMMAND; }}__remcmd_report_cwd"
fi
__remcmd_report_cwd

command rm -rf -- "$REMCMD_INTEGRATION_DIR"
unset REMCMD_ENV REMCMD_INTEGRATION_DIR REMCMD_SHELL_PATH
REMCMD_BASH_ENV
then
    command rm -rf -- "$REMCMD_INTEGRATION_DIR"
    exec "$REMCMD_SHELL_PATH" -l
fi
export ENV="$REMCMD_ENV"
exec "$REMCMD_SHELL_PATH" --posix -l"#
    )
}

fn zsh_launch_command(shell_path: &str) -> String {
    format!(
        r#"REMCMD_SHELL_PATH='{shell_path}'
REMCMD_ORIGINAL_ZDOTDIR=${{ZDOTDIR:-$HOME}}
export REMCMD_SHELL_PATH REMCMD_ORIGINAL_ZDOTDIR
if ! REMCMD_INTEGRATION_DIR=$(mktemp -d "${{TMPDIR:-/tmp}}/remcmd-shell.XXXXXX"); then
    exec "$REMCMD_SHELL_PATH" -l
fi
export REMCMD_INTEGRATION_DIR
if ! command cat >"$REMCMD_INTEGRATION_DIR/.zshenv" <<'REMCMD_ZSHENV'
export ZDOTDIR="$REMCMD_INTEGRATION_DIR"
REMCMD_ZSHENV
then
    command rm -rf -- "$REMCMD_INTEGRATION_DIR"
    exec "$REMCMD_SHELL_PATH" -l
fi
if ! command cat >"$REMCMD_INTEGRATION_DIR/.zprofile" <<'REMCMD_ZPROFILE'
[[ -r "$REMCMD_ORIGINAL_ZDOTDIR/.zprofile" ]] && builtin source "$REMCMD_ORIGINAL_ZDOTDIR/.zprofile"
export ZDOTDIR="$REMCMD_INTEGRATION_DIR"
REMCMD_ZPROFILE
then
    command rm -rf -- "$REMCMD_INTEGRATION_DIR"
    exec "$REMCMD_SHELL_PATH" -l
fi
if ! command cat >"$REMCMD_INTEGRATION_DIR/.zshrc" <<'REMCMD_ZSHRC'
[[ -r "$REMCMD_ORIGINAL_ZDOTDIR/.zshrc" ]] && builtin source "$REMCMD_ORIGINAL_ZDOTDIR/.zshrc"
autoload -Uz add-zsh-hook
__remcmd_report_cwd() {{
    if [[ ${{__remcmd_last_cwd-}} != "$PWD" ]]; then
        __remcmd_last_cwd=$PWD
        builtin printf '\033]7;file://%s\007' "$PWD"
    fi
}}
add-zsh-hook chpwd __remcmd_report_cwd
add-zsh-hook precmd __remcmd_report_cwd
__remcmd_report_cwd
export ZDOTDIR="$REMCMD_ORIGINAL_ZDOTDIR"
command rm -rf -- "$REMCMD_INTEGRATION_DIR"
unset REMCMD_ORIGINAL_ZDOTDIR REMCMD_INTEGRATION_DIR REMCMD_SHELL_PATH
REMCMD_ZSHRC
then
    command rm -rf -- "$REMCMD_INTEGRATION_DIR"
    exec "$REMCMD_SHELL_PATH" -l
fi
export ZDOTDIR="$REMCMD_INTEGRATION_DIR"
exec "$REMCMD_SHELL_PATH" -l"#
    )
}

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
    fn launch_commands_report_cwd_and_fall_back_to_login_shell() {
        for shell in [
            ShellIntegration::detect(b"/bin/bash").unwrap(),
            ShellIntegration::detect(b"/bin/zsh").unwrap(),
        ] {
            let command = shell.launch_command();
            assert!(command.contains("7;file://%s"));
            assert!(command.contains("exec \"$REMCMD_SHELL_PATH\" -l"));
            assert!(command.contains("remcmd-shell.XXXXXX"));
        }
    }

    #[test]
    fn generated_shell_code_passes_native_syntax_checks() {
        let bash = ShellIntegration::detect(b"/bin/bash").unwrap();
        let bash_command = bash.launch_command();
        assert_shell_syntax("/bin/bash", &bash_command);
        assert_shell_syntax("/bin/bash", heredoc_body(&bash_command, "REMCMD_BASH_ENV"));

        let zsh = ShellIntegration::detect(b"/bin/zsh").unwrap();
        let zsh_command = zsh.launch_command();
        if Command::new("/bin/zsh").arg("--version").output().is_ok() {
            assert_shell_syntax("/bin/zsh", &zsh_command);
            for delimiter in ["REMCMD_ZSHENV", "REMCMD_ZPROFILE", "REMCMD_ZSHRC"] {
                assert_shell_syntax("/bin/zsh", heredoc_body(&zsh_command, delimiter));
            }
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

    fn heredoc_body<'a>(command: &'a str, delimiter: &str) -> &'a str {
        let header = format!("<<'{delimiter}'\n");
        let terminator = format!("\n{delimiter}\n");
        command
            .split_once(&header)
            .and_then(|(_, remainder)| remainder.split_once(&terminator))
            .map(|(body, _)| body)
            .expect("generated command should contain the requested heredoc")
    }
}
