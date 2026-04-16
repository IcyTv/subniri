use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExistingInstancePolicy {
	ReplaceExisting,
	ExitIfRunning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
	Acquired,
	Replaced,
	AlreadyRunning,
}

pub fn ensure_single_instance(scope: &str, policy: ExistingInstancePolicy) -> EnsureOutcome {
	let pid_file = pid_file_path(scope);
	let expected_name = expected_process_name();
	let mut replaced_existing = false;

	if let Some(existing_pid) = read_existing_pid(&pid_file)
		&& existing_pid != std::process::id()
		&& is_expected_process(existing_pid, expected_name.as_deref())
	{
		match policy {
			ExistingInstancePolicy::ExitIfRunning => {
				return EnsureOutcome::AlreadyRunning;
			}
			ExistingInstancePolicy::ReplaceExisting => {
				replaced_existing = true;
				let _ = Command::new("kill").args(["-TERM", &existing_pid.to_string()]).status();

				for _ in 0..20 {
					if !Path::new(&format!("/proc/{existing_pid}")).exists() {
						break;
					}
					thread::sleep(Duration::from_millis(100));
				}
			}
		}
	}

	if let Some(parent) = pid_file.parent() {
		let _ = fs::create_dir_all(parent);
	}

	let _ = fs::write(&pid_file, std::process::id().to_string());

	if replaced_existing {
		EnsureOutcome::Replaced
	} else {
		EnsureOutcome::Acquired
	}
}

fn pid_file_path(scope: &str) -> PathBuf {
	if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
		return Path::new(&runtime_dir).join(format!("{scope}.pid"));
	}

	Path::new("/tmp").join(format!("{scope}.pid"))
}

fn read_existing_pid(pid_file: &Path) -> Option<u32> {
	let pid_text = fs::read_to_string(pid_file).ok()?;
	pid_text.trim().parse::<u32>().ok()
}

fn expected_process_name() -> Option<String> {
	let exe = std::env::current_exe().ok()?;
	let file_name = exe.file_name()?;
	Some(file_name.to_string_lossy().into_owned())
}

fn is_expected_process(pid: u32, expected_name: Option<&str>) -> bool {
	let cmdline_path = format!("/proc/{pid}/cmdline");
	let Ok(cmdline) = fs::read(cmdline_path) else {
		return false;
	};

	let expected_name = expected_name.unwrap_or_default();
	if expected_name.is_empty() {
		return true;
	}

	cmdline
		.split(|byte| *byte == 0)
		.filter(|part| !part.is_empty())
		.filter_map(|part| std::str::from_utf8(part).ok())
		.any(|part| part.contains(expected_name))
}
