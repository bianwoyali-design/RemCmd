use std::{io, sync::Arc};

use gpui::Global;
use tokio::runtime::{Builder, Handle, Runtime};

/// Owns the Tokio runtime used by SSH network tasks.
///
/// GPUI and Tokio use different async executors. SSH futures must be spawned
/// here instead of being polled directly by GPUI's executor.
#[derive(Clone)]
pub struct SshRuntime {
    // Arc allows GPUI tasks to retain a runtime handle safely.
    runtime: Arc<Runtime>,
}

impl SshRuntime {
    /// Creates a multithreaded Tokio runtime with networking and timers enabled.
    pub fn new() -> io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("remcmd-ssh")
            .build()?;

        Ok(Self {
            runtime: Arc::new(runtime),
        })
    }

    /// Returns a cloneable handle used to start SSH workers.
    pub fn handle(&self) -> Handle {
        self.runtime.handle().clone()
    }
}

/// Allows the runtime to be stored once in GPUI's application globals.
impl Global for SshRuntime {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_executes_spawned_future() {
        let runtime = SshRuntime::new().expect("runtime should be created");

        let task = runtime.handle().spawn(async { 42 });

        let result = runtime.runtime.block_on(task).expect("task should finish");

        assert_eq!(result, 42);
    }
}
