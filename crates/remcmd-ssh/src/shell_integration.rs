use std::time::Duration;

pub(crate) const DETECT_SHELL_COMMAND: &str = r#"printf '%s\n' "${SHELL-}" "${0-}"; awk -F: -v u="$(id -un 2>/dev/null)" '$1 == u { print $7; exit }' /etc/passwd 2>/dev/null || true"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellKind {
    Bash,
    Zsh,
    Ash,
}

pub(crate) fn detect_shell(output: &[u8]) -> Option<ShellKind> {
    output
        .split(|byte| byte.is_ascii_whitespace())
        .filter_map(|candidate| std::str::from_utf8(candidate).ok())
        .find_map(|candidate| {
            match candidate
                .rsplit('/')
                .next()
                .unwrap_or(candidate)
                .trim_start_matches('-')
            {
                "bash" => Some(ShellKind::Bash),
                "zsh" => Some(ShellKind::Zsh),
                "ash" => Some(ShellKind::Ash),
                _ => None,
            }
        })
}

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

pub(crate) fn install_command_for_shell(shell: ShellKind, ready_command: &str) -> String {
    let hook = match shell {
        ShellKind::Bash => BASH_INSTALL_COMMAND,
        ShellKind::Zsh => ZSH_INSTALL_COMMAND,
        ShellKind::Ash => ASH_INSTALL_COMMAND,
    };
    // One command line: a second line would wait for another prompt render
    // (for example Starship's precmd) before reporting readiness.
    format!(" eval {}; {ready_command}", shell_single_quote(hook))
}

/// Reports whether the latest bracketed-paste mode change in `output` turned it
/// on. zsh's ZLE and bash 5.1+ readline enable it only while waiting for a
/// command line, so it marks a prompt that is ready to accept hidden input.
pub(crate) fn line_editor_state(output: &[u8]) -> Option<bool> {
    let last = |needle: &[u8]| {
        output
            .windows(needle.len())
            .rposition(|window| window == needle)
    };
    match (last(b"\x1b[?2004h"), last(b"\x1b[?2004l")) {
        (None, None) => None,
        (enabled, disabled) => Some(enabled > disabled),
    }
}

/// Quiet time required before typing the hook when the shell gives no better signal.
const INSTALL_QUIET_PERIOD: Duration = Duration::from_millis(120);
/// Short settle once the line editor is reading input, letting a prompt finish
/// drawing before the hidden hook is typed.
const INSTALL_READY_SETTLE: Duration = Duration::from_millis(30);

/// What the user has done on the command line the hook would be typed into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum UserLine {
    /// The shell's prompt with nothing typed on it.
    #[default]
    Empty,
    /// Text is on the line. `erasable` counts characters that backspace can still
    /// remove; `None` once an editing key makes the line's contents unknowable.
    Typing { erasable: Option<usize> },
    /// A command was run. What reads the terminal now may not be the shell.
    Submitted,
}

/// Decides when the hidden hook may be typed without landing in the user's input.
///
/// The hook is sent as keystrokes, so it joins whatever is already on the line and
/// goes to whatever program is reading the terminal. Until the hook is installed the
/// shell cannot say when it is back at a prompt: programs such as `psql`, a Python
/// REPL, or a nested `ssh` enable bracketed paste exactly as a shell prompt does. So
/// once the user has run a command, that signal is no longer taken as an invitation.
#[derive(Debug, Default)]
pub(crate) struct InstallGate {
    line_editor_seen: bool,
    line_editor_ready: bool,
    user_line: UserLine,
}

impl InstallGate {
    pub(crate) fn observe_output(&mut self, output: &[u8]) {
        if let Some(ready) = line_editor_state(output) {
            self.line_editor_seen = true;
            self.line_editor_ready = ready;
        }
    }

    pub(crate) fn observe_input(&mut self, input: &[u8]) {
        for &byte in input {
            self.user_line = match (self.user_line, byte) {
                (UserLine::Submitted, _) => UserLine::Submitted,
                // Return on an empty line runs nothing; the shell just prompts again.
                (UserLine::Empty, b'\r' | b'\n') => UserLine::Empty,
                (UserLine::Typing { .. }, b'\r' | b'\n') => UserLine::Submitted,
                // Ctrl-C and Ctrl-U discard the line being edited.
                (_, 0x03 | 0x15) => UserLine::Empty,
                (UserLine::Empty, 0x08 | 0x7f) => UserLine::Empty,
                (UserLine::Typing { erasable: Some(1) }, 0x08 | 0x7f) => UserLine::Empty,
                (UserLine::Typing { erasable }, 0x08 | 0x7f) => UserLine::Typing {
                    erasable: erasable.map(|count| count.saturating_sub(1)),
                },
                // Escape sequences and other controls (history, cursor keys,
                // completion) can change the line in ways that cannot be followed.
                (_, 0x00..=0x1f) => UserLine::Typing { erasable: None },
                // Continuation bytes belong to the character already counted.
                (line @ UserLine::Typing { .. }, 0x80..=0xbf) => line,
                (UserLine::Empty, _) => UserLine::Typing { erasable: Some(1) },
                (UserLine::Typing { erasable }, _) => UserLine::Typing {
                    erasable: erasable.map(|count| count + 1),
                },
            };
        }
    }

    /// How long output must stay quiet before the hook is typed, or `None` while the
    /// line belongs to the user or to a program the user started.
    pub(crate) fn delay(&self) -> Option<Duration> {
        match self.user_line {
            UserLine::Empty if self.line_editor_ready => Some(INSTALL_READY_SETTLE),
            UserLine::Empty => Some(INSTALL_QUIET_PERIOD),
            UserLine::Typing { .. } => None,
            // Seeing bracketed paste switch on again does not prove the shell is back.
            UserLine::Submitted if self.line_editor_seen => None,
            // Shells that never announce a prompt keep the behaviour they always
            // had: wait for the output to go quiet after the command.
            UserLine::Submitted => Some(INSTALL_QUIET_PERIOD),
        }
    }
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

const BASH_INSTALL_COMMAND: &str = r#"__remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; printf '\033]7;file://%s\007' "$PWD"; fi; }; if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a "* ]]; then PROMPT_COMMAND+=(__remcmd_report_cwd); else PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }__remcmd_report_cwd"; fi; __remcmd_last_cwd=$PWD"#;

const ZSH_INSTALL_COMMAND: &str = r#"autoload -Uz add-zsh-hook; __remcmd_report_cwd(){ if [[ ${__remcmd_last_cwd-} != "$PWD" ]]; then __remcmd_last_cwd=$PWD; builtin printf '\033]7;file://%s\007' "$PWD"; fi; }; add-zsh-hook chpwd __remcmd_report_cwd; add-zsh-hook precmd __remcmd_report_cwd; __remcmd_last_cwd=$PWD"#;

const ASH_INSTALL_COMMAND: &str = r#"__remcmd_report_cwd(){ if [ "${__remcmd_last_cwd-}" != "$PWD" ]; then __remcmd_last_cwd=$PWD; printf '\033]7;file://%s\007' "$PWD"; fi; }; cd(){ command cd "$@" && __remcmd_report_cwd; }; __remcmd_last_cwd=$PWD"#;

#[cfg(test)]
mod tests {
    #[cfg(unix)]
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
    fn shell_detection_accepts_login_shell_paths_and_rejects_unknown_shells() {
        assert_eq!(detect_shell(b"\n-bash\n"), Some(ShellKind::Bash));
        assert_eq!(detect_shell(b"\n-zsh\n"), Some(ShellKind::Zsh));
        assert_eq!(detect_shell(b"/bin/ash\n"), Some(ShellKind::Ash));
        assert_eq!(detect_shell(b"/usr/bin/fish\n"), None);
    }

    #[test]
    fn shell_specific_install_only_contains_the_selected_hook() {
        let bash = install_command_for_shell(ShellKind::Bash, ":");
        assert!(bash.contains("PROMPT_COMMAND"));
        assert!(!bash.contains("add-zsh-hook"));

        let zsh = install_command_for_shell(ShellKind::Zsh, ":");
        assert!(zsh.contains("add-zsh-hook"));
        assert!(!zsh.contains("PROMPT_COMMAND"));

        let ash = install_command_for_shell(ShellKind::Ash, ":");
        assert!(ash.contains("cd(){"));
        assert!(!ash.contains("PROMPT_COMMAND"));
    }

    #[test]
    fn shell_specific_install_reports_readiness_on_the_same_command_line() {
        let command = install_command_for_shell(ShellKind::Zsh, "printf ready\r");

        assert_eq!(command.matches('\r').count(), 1);
        assert!(command.ends_with("; printf ready\r"));
    }

    #[test]
    fn line_editor_state_follows_the_latest_bracketed_paste_change() {
        assert_eq!(line_editor_state(b"Last login: today\r\n"), None);
        assert_eq!(line_editor_state(b"\x1b[?2004h~ \xe2\x9d\xaf "), Some(true));
        assert_eq!(
            line_editor_state(b"\x1b[?2004h\r\n\x1b[?2004l"),
            Some(false)
        );
        assert_eq!(
            line_editor_state(b"\x1b[?2004l\r\n\x1b[?2004h$ "),
            Some(true)
        );
    }

    #[test]
    fn install_never_joins_a_line_the_user_is_typing() {
        let mut gate = InstallGate::default();
        assert_eq!(gate.delay(), Some(INSTALL_QUIET_PERIOD));
        gate.observe_output(b"\x1b[?2004h~ $ ");
        assert_eq!(gate.delay(), Some(INSTALL_READY_SETTLE));

        // Typing claims the line: its echo must not let the hook join the command.
        gate.observe_input(b"l");
        gate.observe_output(b"l");
        assert_eq!(gate.delay(), None);
        gate.observe_input(b"s");
        assert_eq!(gate.delay(), None);

        // Erasing everything, or discarding the line, hands the prompt back.
        gate.observe_input(b"\x7f\x7f");
        assert_eq!(gate.delay(), Some(INSTALL_READY_SETTLE));
        gate.observe_input("caf\u{e9}".as_bytes());
        gate.observe_input(b"\x7f\x7f\x7f");
        assert_eq!(gate.delay(), None);
        gate.observe_input(b"\x7f");
        assert_eq!(gate.delay(), Some(INSTALL_READY_SETTLE));
        gate.observe_input(b"ls -l\x15");
        assert_eq!(gate.delay(), Some(INSTALL_READY_SETTLE));

        // Return on an empty line runs nothing, so the prompt is still the shell's.
        gate.observe_input(b"\r");
        gate.observe_output(b"\x1b[?2004l\r\n\x1b[?2004h~ $ ");
        assert_eq!(gate.delay(), Some(INSTALL_READY_SETTLE));

        // History and cursor keys change the line unseen; only discarding it helps.
        gate.observe_input(b"\x1b[A");
        gate.observe_input(b"\x7f\x7f\x7f\x7f");
        assert_eq!(gate.delay(), None);
        gate.observe_input(b"\x03");
        assert_eq!(gate.delay(), Some(INSTALL_READY_SETTLE));
    }

    #[test]
    fn a_program_the_user_started_is_not_mistaken_for_the_shell_prompt() {
        let mut gate = InstallGate::default();
        gate.observe_output(b"\x1b[?2004h~ $ ");
        gate.observe_input(b"psql\r");
        gate.observe_output(b"psql\r\n\x1b[?2004l");
        assert_eq!(gate.delay(), None);

        // The program enables bracketed paste for its own prompt, as a shell would.
        gate.observe_output(b"psql (16.2)\r\n\x1b[?2004hdb=# ");
        assert_eq!(gate.delay(), None);

        // Nothing typed into it, and none of its later prompts, reopens the gate.
        gate.observe_input(b"select 1;\r");
        gate.observe_output(b"\x1b[?2004l 1\r\n\x1b[?2004hdb=# ");
        assert_eq!(gate.delay(), None);
        gate.observe_input(b"\x03");
        gate.observe_output(b"^C\r\n\x1b[?2004hdb=# ");
        assert_eq!(gate.delay(), None);
    }

    #[test]
    fn shells_without_prompt_signals_install_after_the_typed_line_is_gone() {
        let mut gate = InstallGate::default();
        gate.observe_output(b"~ $ ");
        gate.observe_input(b"pw");
        gate.observe_output(b"pw");
        assert_eq!(gate.delay(), None);

        gate.observe_input(b"d\r");
        gate.observe_output(b"d\r\n/root\r\n~ $ ");
        assert_eq!(gate.delay(), Some(INSTALL_QUIET_PERIOD));
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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
