//! Killable converters see only the pinned input, private output, and Nix runtime.
use std::{
	ffi::{OsStr, OsString},
	fs::{File, OpenOptions},
	io::Read,
	os::unix::process::CommandExt,
	path::{Path, PathBuf},
	process::Command,
	sync::atomic::AtomicBool,
	time::{Duration, Instant},
};

use iced::widget::image;

use super::load::{check_cancel, open_regular, raster};

pub(super) fn svg(input: &File, cancel: &AtomicBool) -> Result<image::Handle, String> {
	let workspace = tempfile::Builder::new()
		.prefix("fileberg-preview-")
		.tempdir()
		.map_err(|e| e.to_string())?;
	let font = std::env::var_os("FILEBERG_PREVIEW_FONT").ok_or("Preview font unavailable")?;
	if !Path::new(&font).starts_with("/nix/store") {
		return Err("Preview font must come from the Nix runtime".into());
	}

	run(
		input,
		workspace.path(),
		"FILEBERG_RESVG",
		&[
			"--skip-system-fonts".into(),
			"--use-font-file".into(),
			font,
			"--font-family".into(),
			"DM Sans".into(),
			"--width".into(),
			"1600".into(),
			"--height".into(),
			"1600".into(),
			"/input/file".into(),
			"/output/svg.png".into(),
		],
		cancel,
	)?;
	check_cancel(cancel)?;

	raster(open_regular(&workspace.path().join("svg.png"))?)
}

pub(super) fn pdf(
	input: &File, page: u32, cancel: &AtomicBool,
) -> Result<(image::Handle, u32), String> {
	if input.metadata().map_err(|e| e.to_string())?.len() > 128 * 1024 * 1024 {
		return Err("PDF exceeds the 128 MiB preview limit".into());
	}

	let workspace = tempfile::Builder::new()
		.prefix("fileberg-preview-")
		.tempdir()
		.map_err(|e| e.to_string())?;
	let output = run(
		input,
		workspace.path(),
		"FILEBERG_PDFINFO",
		&["/input/file".into()],
		cancel,
	)?;
	let pages = page_count(&output)?;
	if page == 0 || page > pages {
		return Err("PDF page is out of range".into());
	}

	run(
		input,
		workspace.path(),
		"FILEBERG_PDFTOPPM",
		&[
			"-f".into(),
			page.to_string().into(),
			"-l".into(),
			page.to_string().into(),
			"-singlefile".into(),
			"-scale-to".into(),
			"1600".into(),
			"-png".into(),
			"/input/file".into(),
			"/output/page".into(),
		],
		cancel,
	)?;
	check_cancel(cancel)?;
	let image = raster(open_regular(&workspace.path().join("page.png"))?)?;

	Ok((image, pages))
}

pub(super) fn video(input: &File, cancel: &AtomicBool) -> Result<image::Handle, String> {
	let workspace = tempfile::Builder::new()
		.prefix("fileberg-preview-")
		.tempdir()
		.map_err(|e| e.to_string())?;

	for timestamp in ["1", "0"] {
		run(
			input,
			workspace.path(),
			"FILEBERG_FFMPEG",
			&[
				"-nostdin".into(),
				"-hide_banner".into(),
				"-loglevel".into(),
				"error".into(),
				"-protocol_whitelist".into(),
				"file".into(),
				"-threads".into(),
				"1".into(),
				"-ss".into(),
				timestamp.into(),
				"-i".into(),
				"/input/file".into(),
				"-map".into(),
				"0:V:0".into(),
				"-frames:v".into(),
				"1".into(),
				"-an".into(),
				"-sn".into(),
				"-dn".into(),
				"-threads".into(),
				"1".into(),
				"-vf".into(),
				"scale=1280:720:force_original_aspect_ratio=decrease".into(),
				"-f".into(),
				"image2".into(),
				"-y".into(),
				"/output/frame.png".into(),
			],
			cancel,
		)?;

		if let Ok(file) = open_regular(&workspace.path().join("frame.png")) {
			return raster(file);
		}
	}

	Err("No video frame could be extracted".into())
}

fn page_count(output: &str) -> Result<u32, String> {
	output
		.lines()
		.find_map(|line| line.strip_prefix("Pages:")?.trim().parse::<u32>().ok())
		.filter(|count| *count > 0)
		.ok_or_else(|| {
			"Cannot read PDF page count; encrypted or damaged PDFs cannot be previewed".into()
		})
}

fn executable(variable: &str) -> Result<PathBuf, String> {
	let path = std::env::var_os(variable).ok_or_else(|| {
		format!(
			"Preview tool unavailable ({variable}); use the packaged Fileberg or development shell"
		)
	})?;
	let path = PathBuf::from(path)
		.canonicalize()
		.map_err(|e| format!("Preview tool unavailable: {e}"))?;
	if !path.starts_with("/nix/store") {
		return Err("Preview tools must come from the Nix runtime".into());
	}

	Ok(path)
}

fn run(
	input: &File, workspace: &Path, tool: &str, args: &[OsString], cancel: &AtomicBool,
) -> Result<String, String> {
	run_with_timeout(input, workspace, tool, args, cancel, Duration::from_secs(5))
}

fn run_with_timeout(
	input: &File, workspace: &Path, tool: &str, args: &[OsString], cancel: &AtomicBool,
	timeout: Duration,
) -> Result<String, String> {
	check_cancel(cancel)?;

	let executable = executable(tool)?;
	let sandbox = executable_path()?;
	let stdout_path = workspace.join("stdout");
	let stderr_path = workspace.join("stderr");
	let stdout = OpenOptions::new()
		.create(true)
		.truncate(true)
		.write(true)
		.open(&stdout_path)
		.map_err(|e| e.to_string())?;
	let stderr = OpenOptions::new()
		.create(true)
		.truncate(true)
		.write(true)
		.open(&stderr_path)
		.map_err(|e| e.to_string())?;

	let mut command = Command::new(sandbox);
	command
		.args([
			"--unshare-all",
			"--die-with-parent",
			"--new-session",
			"--clearenv",
			"--ro-bind",
			"/nix/store",
			"/nix/store",
			"--proc",
			"/proc",
			"--dev",
			"/dev",
			"--size",
			"16777216",
			"--tmpfs",
			"/tmp",
			"--dir",
			"/input",
			"--ro-bind-fd",
			"0",
			"/input/file",
			"--dir",
			"/output",
			"--bind",
		])
		.arg(workspace)
		.args([
			"/output", "--chdir", "/output", "--setenv", "HOME", "/tmp", "--setenv", "LC_ALL", "C",
		]);
	if let Some(config) = std::env::var_os("FILEBERG_FONTCONFIG_FILE") {
		let path = Path::new(&config);
		if path.starts_with("/nix/store") {
			command.args([
				OsStr::new("--setenv"),
				OsStr::new("FONTCONFIG_FILE"),
				&config,
			]);
		}
	}
	command
		.arg("--")
		.arg(executable)
		.args(args)
		.stdin(input.try_clone().map_err(|e| e.to_string())?)
		.stdout(stdout)
		.stderr(stderr);

	// Safety: only async-signal-safe libc calls run between fork and exec.
	unsafe {
		command.pre_exec(|| {
			for (resource, limit) in [
				(libc::RLIMIT_AS, 1024 * 1024 * 1024),
				(libc::RLIMIT_FSIZE, 16 * 1024 * 1024),
				(libc::RLIMIT_CPU, 8),
				(libc::RLIMIT_NOFILE, 64),
			] {
				let value = libc::rlimit {
					rlim_cur: limit,
					rlim_max: limit,
				};
				if libc::setrlimit(resource, &value) != 0 {
					return Err(std::io::Error::last_os_error());
				}
			}

			Ok(())
		});
	}

	let mut child = command
		.spawn()
		.map_err(|e| format!("Cannot start preview sandbox: {e}"))?;
	let started = Instant::now();
	let status = loop {
		match child.try_wait() {
			Ok(Some(status)) => break status,
			Ok(None) => {}
			Err(error) => {
				let _ = child.kill();
				let _ = child.wait();
				return Err(format!("Cannot monitor preview converter: {error}"));
			}
		}

		if check_cancel(cancel).is_err() || started.elapsed() > timeout {
			// Bubblewrap's PID namespace init kills all remaining descendants on exit.
			let _ = child.kill();
			let _ = child.wait();
			check_cancel(cancel)?;
			return Err(format!(
				"Preview converter exceeded its {:.1}-second time limit",
				timeout.as_secs_f32()
			));
		}

		std::thread::sleep(Duration::from_millis(25));
	};

	if !status.success() {
		let detail = bounded_output(&stderr_path)?;
		let detail = detail.lines().take(3).collect::<Vec<_>>().join(" ");
		return Err(format!(
			"Preview unavailable: converter or sandbox failed. {detail}"
		));
	}

	bounded_output(&stdout_path)
}

fn executable_path() -> Result<PathBuf, String> {
	executable("FILEBERG_BWRAP")
}

fn bounded_output(path: &Path) -> Result<String, String> {
	let mut output = Vec::new();
	File::open(path)
		.map_err(|e| e.to_string())?
		.take(64 * 1024)
		.read_to_end(&mut output)
		.map_err(|e| e.to_string())?;

	Ok(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(test)]
mod tests {
	#![allow(clippy::unwrap_used)]
	use super::*;
	use crate::preview::tests::test_dir;
	use std::sync::{Arc, atomic::Ordering};

	fn write_pdf(path: &Path) {
		let objects = [
			"<< /Type /Catalog /Pages 2 0 R >>",
			"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
			"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 5 0 R /Resources << >> >>",
			"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] /Contents 5 0 R /Resources << >> >>",
			"<< /Length 22 >>\nstream\n1 0 0 rg 0 0 50 50 re f\nendstream",
		];
		let mut pdf = String::from("%PDF-1.4\n");
		let mut offsets = Vec::new();

		for (index, object) in objects.iter().enumerate() {
			offsets.push(pdf.len());
			pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
		}

		let xref = pdf.len();
		pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
		for offset in offsets {
			pdf.push_str(&format!("{offset:010} 00000 n \n"));
		}
		pdf.push_str(&format!(
			"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
		));

		std::fs::write(path, pdf).unwrap();
	}

	#[test]
	fn parses_page_counts_without_accepting_missing_or_zero_counts() {
		assert_eq!(page_count("Title: test\nPages:          12\n").unwrap(), 12);
		assert!(page_count("Pages: 0").is_err());
		assert!(page_count("Pages: -2").is_err());
		assert!(page_count("Encrypted: yes").is_err());
	}

	#[test]
	#[ignore = "requires Nix preview tools and unprivileged user namespaces"]
	fn real_pdf_renders_requested_pages_and_rejects_bad_input() {
		let dir = test_dir();
		let path = dir.path().join("two-pages.pdf");
		write_pdf(&path);
		let file = open_regular(&path).unwrap();

		let (first, pages) = pdf(&file, 1, &AtomicBool::new(false)).unwrap();

		assert_eq!(pages, 2);
		assert!(matches!(
			first,
			image::Handle::Rgba {
				width: 1600,
				height: 800,
				..
			}
		));

		let (second, _) = pdf(&file, 2, &AtomicBool::new(false)).unwrap();

		assert!(matches!(
			second,
			image::Handle::Rgba {
				width: 800,
				height: 1600,
				..
			}
		));
		assert!(pdf(&file, 3, &AtomicBool::new(false)).is_err());

		std::fs::write(&path, "broken PDF").unwrap();
		assert!(pdf(&file, 1, &AtomicBool::new(false)).is_err());
	}

	#[test]
	#[ignore = "requires Nix preview tools and unprivileged user namespaces"]
	fn real_video_still_and_converter_timeout_cancellation_and_confinement() {
		let dir = test_dir();
		let path = dir.path().join("sample.mp4");

		let generated = Command::new(executable("FILEBERG_FFMPEG").unwrap())
			.args([
				"-nostdin",
				"-hide_banner",
				"-loglevel",
				"error",
				"-f",
				"lavfi",
				"-i",
				"color=c=red:s=64x32:d=0.2",
				"-c:v",
				"mpeg4",
				"-threads",
				"1",
				"-y",
			])
			.arg(&path)
			.status()
			.unwrap();
		assert!(generated.success());
		let input = open_regular(&path).unwrap();

		let frame = video(&input, &AtomicBool::new(false)).unwrap();

		assert!(matches!(frame, image::Handle::Rgba { .. }));

		let long_running = [
			"-nostdin",
			"-loglevel",
			"error",
			"-re",
			"-f",
			"lavfi",
			"-i",
			"color=s=64x32",
			"-t",
			"20",
			"-f",
			"null",
			"-",
		]
		.map(OsString::from);
		let started = Instant::now();
		let error = run_with_timeout(
			&input,
			dir.path(),
			"FILEBERG_FFMPEG",
			&long_running,
			&AtomicBool::new(false),
			Duration::from_millis(150),
		)
		.unwrap_err();

		assert!(error.contains("time limit"), "{error}");
		assert!(started.elapsed() < Duration::from_secs(3));

		let cancelled = Arc::new(AtomicBool::new(false));
		let signal = cancelled.clone();
		let cancel_worker = std::thread::spawn(move || {
			std::thread::sleep(Duration::from_millis(150));
			signal.store(true, Ordering::Relaxed);
		});

		let error = run(
			&input,
			dir.path(),
			"FILEBERG_FFMPEG",
			&long_running,
			&cancelled,
		)
		.unwrap_err();
		cancel_worker.join().unwrap();

		assert_eq!(error, "Preview cancelled");

		// The original pathname is not present inside the converter's filesystem.
		let error = run(
			&input,
			dir.path(),
			"FILEBERG_FFMPEG",
			&[
				"-nostdin".into(),
				"-loglevel".into(),
				"error".into(),
				"-i".into(),
				path.as_os_str().into(),
				"-f".into(),
				"null".into(),
				"-".into(),
			],
			&AtomicBool::new(false),
		)
		.unwrap_err();

		assert!(error.contains("No such file or directory"), "{error}");
	}
}
