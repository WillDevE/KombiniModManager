//! Detects whether Deadlock is running.

use sysinfo::{ProcessesToUpdate, System};

pub struct ProcessProbe {
    system: System,
}

impl ProcessProbe {
    pub fn new() -> Self {
        Self {
            system: System::new(),
        }
    }

    /// Pid of the first `deadlock.exe` (or `deadlock` under Proton), if any.
    pub fn deadlock_pid(&mut self) -> Option<u32> {
        self.system.refresh_processes(ProcessesToUpdate::All, true);
        self.system
            .processes()
            .iter()
            .find(|(_, p)| is_deadlock_name(&p.name().to_string_lossy()))
            .map(|(pid, _)| pid.as_u32())
    }

    pub fn is_running(&mut self) -> bool {
        self.deadlock_pid().is_some()
    }
}

impl Default for ProcessProbe {
    fn default() -> Self {
        Self::new()
    }
}

pub fn is_deadlock_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(crate::PROCESS_NAME) || name.eq_ignore_ascii_case("deadlock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_deadlock_name("Deadlock.exe"));
        assert!(is_deadlock_name("deadlock"));
        assert!(!is_deadlock_name("deadlock_launcher.exe"));
    }
}
