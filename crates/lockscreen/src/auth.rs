use std::{
	ffi::{CStr, CString, c_char, c_int, c_void},
	fs, io,
	os::unix::fs::PermissionsExt,
	path::Path,
	ptr,
};

use nix::unistd::{User, geteuid, getuid};
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroize;

use crate::secret::Secret;

const SUCCESS: c_int = 0;
const SYSTEM_ERROR: c_int = 4;
const BUFFER_ERROR: c_int = 5;
const CONVERSATION_ERROR: c_int = 19;
const DISALLOW_NULL_TOKEN: c_int = 1;
const USER_ITEM: c_int = 2;

// Linux-PAM application ABI. Keep the opaque handle private to this module.
#[repr(C)]
struct PamHandle {
	_private: [u8; 0],
}

#[repr(C)]
struct PamMessage {
	style: c_int,
	text: *const c_char,
}

#[repr(C)]
struct PamResponse {
	text: *mut c_char,
	code: c_int,
}

#[repr(C)]
struct PamConversation {
	callback: unsafe extern "C" fn(
		c_int,
		*const *const PamMessage,
		*mut *mut PamResponse,
		*mut c_void,
	) -> c_int,
	data: *mut c_void,
}

#[link(name = "pam")]
unsafe extern "C" {
	fn pam_start(
		service: *const c_char, user: *const c_char, conversation: *const PamConversation,
		handle: *mut *mut PamHandle,
	) -> c_int;

	fn pam_authenticate(handle: *mut PamHandle, flags: c_int) -> c_int;

	fn pam_acct_mgmt(handle: *mut PamHandle, flags: c_int) -> c_int;

	fn pam_get_item(handle: *const PamHandle, item: c_int, value: *mut *const c_void) -> c_int;

	fn pam_end(handle: *mut PamHandle, status: c_int) -> c_int;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
	Frostguard,
	Login,
}

impl Service {
	// One selected service, one operation. Rejection never selects another stack.
	fn run(self, authenticate: impl FnOnce(Self) -> AuthResult) -> AuthResult {
		authenticate(self)
	}

	fn name(self) -> &'static CStr {
		match self {
			Self::Frostguard => c"frostguard",
			Self::Login => c"login",
		}
	}
}

// Linux-PAM has no public configured-service query. Match its directory order;
// the package supplies its pinned PAM vendor directory only to this crate.
fn select_service(
	directories: &[&str], mut inspect: impl FnMut(&Path) -> io::Result<bool>,
) -> io::Result<Service> {
	for directory in directories {
		if inspect(&Path::new(directory).join("frostguard"))? {
			return Ok(Service::Frostguard);
		}
	}

	Ok(Service::Login)
}

fn policy_present(path: &Path) -> io::Result<bool> {
	match fs::symlink_metadata(path) {
		Ok(_) => {
			// Follow and validate an existing entry, including symlinks. A broken,
			// unreadable, or non-file override is an error, never absence.
			if !fs::metadata(path)?.is_file() {
				return Err(io::Error::from(io::ErrorKind::InvalidData));
			}

			fs::File::open(path)?;
			Ok(true)
		}
		Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
		Err(error) => Err(error),
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
	Start,
	Authenticate,
	Account,
	Identity,
	Conversation,
	End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthResult {
	Accepted,
	Rejected,
	Failure { stage: Stage, status: c_int },
	WorkerFailure,
}

impl AuthResult {
	fn pam_failure(stage: Stage, status: c_int) -> Self {
		// PAM_AUTH_ERR and PAM_MAXTRIES indicate rejection, not proof that a
		// password was wrong. Other errors retain their responsible stage.
		if stage == Stage::Authenticate && matches!(status, 7 | 11) {
			Self::Rejected
		} else {
			Self::Failure { stage, status }
		}
	}

	pub(crate) fn accepted(self) -> bool {
		self == Self::Accepted
	}

	pub(crate) fn status(self) -> &'static str {
		match self {
			Self::Accepted => "Unlock not authorized. Please try again.",
			Self::Rejected => "Credentials rejected. Please try again.",
			Self::Failure {
				stage: Stage::Account,
				..
			} => "Account policy did not allow unlock. Check system PAM policy.",
			Self::Failure { .. } => "Authentication unavailable. Check system PAM policy.",
			Self::WorkerFailure => "Authentication worker failed. Please try again.",
		}
	}
}

pub(crate) struct Account {
	name: CString,
	service: Service,
}

impl Account {
	pub(crate) fn current() -> Result<Self, &'static str> {
		let uid = getuid();
		if uid.is_root() || uid != geteuid() {
			return Err("Run frostguard as the unprivileged desktop user, never setuid or root.");
		}

		let user = User::from_uid(uid)
			.map_err(|_| "Cannot resolve the current user.")?
			.ok_or("Cannot resolve the current user.")?;
		let name = CString::new(user.name).map_err(|_| "Invalid current user name.")?;
		if name.as_bytes().is_empty() {
			return Err("Invalid current user name.");
		}

		let mut directories = vec!["/etc/pam.d", "/usr/lib/pam.d"];
		if let Some(vendor) = option_env!("FROSTGUARD_PAM_VENDOR_DIR") {
			directories.push(vendor);
		}
		let service = select_service(&directories, policy_present).map_err(|error| {
			eprintln!(
				"frostguard: PAM policy detection failed, errno={:?}",
				error.raw_os_error()
			);
			"Cannot inspect the dedicated PAM policy; refusing alternate policy."
		})?;
		eprintln!(
			"frostguard: selected PAM service={}",
			service.name().to_string_lossy()
		);

		Ok(Self { name, service })
	}

	/// Refreshes frostguard's own fingerprint/Howdy grace stamp, so the next
	/// lock's probe stays eligible without needing a fresh password every
	/// time. Call this on every successful unlock, password or biometric --
	/// this gate is separate from sudo/polkit-1's (see `lockscreenGateDir` in
	/// pam-security.nix) specifically because it must *not* be closed by
	/// merely locking the screen, unlike theirs.
	///
	/// Best-effort: a failure here only costs the next lock its fast path,
	/// never correctness, so it's never worth treating as fatal.
	pub(crate) fn refresh_unlock_gate(&self) {
		const GATE_DIR: &str = "/run/lockscreen-gate";
		let Ok(name) = self.name.to_str() else {
			return;
		};
		let path = Path::new(GATE_DIR).join(name);
		if fs::File::create(&path).is_ok() {
			let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o666));
		}
	}

	/// `password` is `None` for a passive fingerprint/Howdy probe: no
	/// credential to offer, so the conversation declines any prompt that
	/// asks for one (see `converse`) rather than answering with a blank
	/// password, which could otherwise unlock a `nullok`-configured account.
	/// A probe that succeeds without ever supplying a password is exactly
	/// the point; a submitted attempt that does so is not (see below).
	pub(crate) fn authenticate(
		&self, password: Option<Secret>, prompts: UnboundedSender<String>,
	) -> AuthResult {
		let result = self
			.service
			.run(|service| self.authenticate_once(service, password, prompts));
		if let AuthResult::Failure { stage, status } = result {
			eprintln!("frostguard: PAM stage={stage:?} status={status}");
		}
		result
	}

	fn authenticate_once(
		&self, service: Service, password: Option<Secret>, prompts: UnboundedSender<String>,
	) -> AuthResult {
		if let Some(password) = &password {
			if password.as_bytes().is_empty() {
				return AuthResult::Rejected;
			}
		}

		let mut data = Conversation {
			account: &self.name,
			password,
			supplied_password: false,
			prompts,
		};
		let conversation = PamConversation {
			callback: converse,
			data: ptr::from_mut(&mut data).cast(),
		};
		let mut raw = ptr::null_mut();

		// PAM calls the conversation synchronously. The handle is ended before
		// either conversation or its borrowed data can be dropped.
		let start = unsafe {
			pam_start(
				service.name().as_ptr(),
				self.name.as_ptr(),
				&conversation,
				&mut raw,
			)
		};
		if start != SUCCESS || raw.is_null() {
			return AuthResult::pam_failure(
				Stage::Start,
				if start == SUCCESS {
					SYSTEM_ERROR
				} else {
					start
				},
			);
		}

		let mut handle = Session {
			raw,
			status: SYSTEM_ERROR,
		};
		let mut stage = Stage::Authenticate;
		handle.status = unsafe { pam_authenticate(handle.raw, DISALLOW_NULL_TOKEN) };
		if handle.status == SUCCESS {
			stage = Stage::Account;
			handle.status = unsafe { pam_acct_mgmt(handle.raw, 0) };
		}
		// Only a submitted attempt (a password typed) is required to have
		// actually consumed one; a probe has none to offer, so a passwordless
		// success is the whole point, not a flow we don't understand.
		if handle.status == SUCCESS && data.password.is_some() && !data.supplied_password {
			eprintln!(
				"frostguard: unsupported PAM flow: passwordless success; only single-password conversations are supported"
			);
			stage = Stage::Conversation;
			handle.status = CONVERSATION_ERROR;
		}
		if handle.status == SUCCESS {
			stage = Stage::Identity;
			let mut user = ptr::null();
			handle.status = unsafe { pam_get_item(handle.raw, USER_ITEM, &mut user) };
			if handle.status == SUCCESS
				&& (user.is_null()
					|| unsafe { CStr::from_ptr(user.cast()) } != self.name.as_c_str())
			{
				handle.status = SYSTEM_ERROR;
			}
		}

		let result = if handle.status == SUCCESS {
			AuthResult::Accepted
		} else {
			eprintln!("frostguard: PAM stage={stage:?} status={}", handle.status);
			AuthResult::pam_failure(stage, handle.status)
		};
		let ended = unsafe { pam_end(handle.raw, handle.status) };
		handle.raw = ptr::null_mut();
		if let Some(password) = &mut data.password {
			password.clear();
		}

		if ended != SUCCESS {
			AuthResult::pam_failure(Stage::End, ended)
		} else {
			result
		}
	}
}

struct Session {
	raw: *mut PamHandle,
	status: c_int,
}

impl Drop for Session {
	fn drop(&mut self) {
		if !self.raw.is_null() {
			// Covers early returns and unwinding without leaking the PAM handle.
			unsafe { pam_end(self.raw, self.status) };
		}
	}
}

struct Conversation<'a> {
	account: &'a CStr,
	password: Option<Secret>,
	supplied_password: bool,
	prompts: UnboundedSender<String>,
}

unsafe extern "C" fn converse(
	count: c_int, messages: *const *const PamMessage, output: *mut *mut PamResponse,
	data: *mut c_void,
) -> c_int {
	if output.is_null() {
		return CONVERSATION_ERROR;
	}
	unsafe { *output = ptr::null_mut() };
	if !(1..=32).contains(&count) || messages.is_null() || data.is_null() {
		return CONVERSATION_ERROR;
	}

	// PAM owns the input pointers for the duration of this callback. calloc
	// gives PAM-compatible allocation and zero response codes, including info.
	let responses =
		unsafe { libc::calloc(count as usize, size_of::<PamResponse>()) }.cast::<PamResponse>();
	if responses.is_null() {
		return BUFFER_ERROR;
	}
	let conversation = unsafe { &mut *data.cast::<Conversation<'_>>() };

	for index in 0..count as usize {
		let message = unsafe { *messages.add(index) };
		if message.is_null() {
			unsafe { free_responses(responses, count as usize) };
			return CONVERSATION_ERROR;
		}

		let style = unsafe { (*message).style };
		let bytes = match style {
			// A probe has no password to offer: refuse the prompt outright
			// rather than answer it with nothing, which on a `nullok`
			// account could otherwise authenticate a blank credential.
			1 if !conversation.supplied_password && conversation.password.is_none() => {
				unsafe { free_responses(responses, count as usize) };
				return CONVERSATION_ERROR;
			}
			1 if !conversation.supplied_password => {
				conversation.supplied_password = true;
				// Safety: the arm above already returned on `None`.
				conversation.password.as_ref().unwrap().as_bytes()
			}
			2 => conversation.account.to_bytes(),
			3 | 4 => {
				let text_ptr = unsafe { (*message).text };
				if !text_ptr.is_null() {
					let text = unsafe { CStr::from_ptr(text_ptr) }.to_string_lossy().into_owned();
					let _ = conversation.prompts.send(text);
				}
				continue;
			}
			_ => {
				eprintln!(
					"frostguard: unsupported PAM flow: additional secret prompt or unsupported prompt style; only single-password conversations are supported"
				);
				unsafe { free_responses(responses, count as usize) };
				return CONVERSATION_ERROR;
			}
		};
		let text = unsafe { libc::malloc(bytes.len() + 1) }.cast::<u8>();
		if text.is_null() {
			unsafe { free_responses(responses, count as usize) };
			return BUFFER_ERROR;
		}

		unsafe {
			ptr::copy_nonoverlapping(bytes.as_ptr(), text, bytes.len());
			*text.add(bytes.len()) = 0;
			(*responses.add(index)).text = text.cast();
		}
		if style == 1 {
			if let Some(password) = &mut conversation.password {
				password.clear();
			}
		}
	}

	// On success ownership passes to PAM, which must erase and free responses.
	unsafe { *output = responses };
	SUCCESS
}

unsafe fn free_responses(responses: *mut PamResponse, count: usize) {
	for index in 0..count {
		let text = unsafe { (*responses.add(index)).text };
		if !text.is_null() {
			let len = unsafe { libc::strlen(text) };
			unsafe { std::slice::from_raw_parts_mut(text.cast::<u8>(), len) }.zeroize();
			unsafe { libc::free(text.cast()) };
		}
	}

	unsafe { libc::free(responses.cast()) };
}

#[cfg(test)]
mod tests {
	use std::os::unix::fs::{PermissionsExt, symlink};

	use super::*;

	fn policy_fixture() -> tempfile::TempDir {
		let preferred = Path::new("/tmp/opencode");
		let root = if preferred.is_dir() {
			preferred.to_owned()
		} else {
			std::env::temp_dir()
		};

		tempfile::Builder::new()
			.prefix("frostguard-policy-")
			.tempdir_in(root)
			.unwrap()
	}

	#[test]
	fn filesystem_absent_policy_selects_login() {
		let directory = policy_fixture();
		let path = directory.path().join("frostguard");

		assert!(!policy_present(&path).unwrap());
		assert_eq!(
			select_service(&[directory.path().to_str().unwrap()], policy_present).unwrap(),
			Service::Login
		);
	}

	#[test]
	fn filesystem_readable_policy_selects_frostguard() {
		let directory = policy_fixture();
		let path = directory.path().join("frostguard");
		fs::write(&path, "auth include login\naccount include login\n").unwrap();

		assert!(policy_present(&path).unwrap());
		assert_eq!(
			select_service(&[directory.path().to_str().unwrap()], policy_present).unwrap(),
			Service::Frostguard
		);
	}

	#[test]
	fn filesystem_valid_policy_symlink_is_present() {
		let directory = policy_fixture();
		let target = directory.path().join("policy");
		let path = directory.path().join("frostguard");
		fs::write(&target, "auth include login\n").unwrap();
		symlink(&target, &path).unwrap();

		assert!(policy_present(&path).unwrap());
	}

	#[test]
	fn filesystem_broken_policy_symlink_never_downgrades() {
		let directory = policy_fixture();
		let path = directory.path().join("frostguard");
		symlink(directory.path().join("missing"), &path).unwrap();

		assert_eq!(
			policy_present(&path).unwrap_err().kind(),
			io::ErrorKind::NotFound
		);
		assert!(select_service(&[directory.path().to_str().unwrap()], policy_present).is_err());
	}

	#[test]
	fn filesystem_directory_policy_is_invalid() {
		let directory = policy_fixture();
		let path = directory.path().join("frostguard");
		fs::create_dir(&path).unwrap();

		assert_eq!(
			policy_present(&path).unwrap_err().kind(),
			io::ErrorKind::InvalidData
		);
	}

	#[test]
	fn filesystem_unreadable_policy_never_downgrades() {
		if geteuid().is_root() {
			eprintln!("SKIP: mode-000 policy permission proof requires an unprivileged user");
			return;
		}

		let directory = policy_fixture();
		let path = directory.path().join("frostguard");
		fs::write(&path, "auth include login\n").unwrap();
		fs::set_permissions(&path, fs::Permissions::from_mode(0)).unwrap();

		let result = policy_present(&path);
		let selection = select_service(&[directory.path().to_str().unwrap()], policy_present);
		fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

		assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
		assert!(selection.is_err());
	}

	#[test]
	fn missing_dedicated_policy_selects_login() {
		let service = select_service(&["/etc/pam.d", "/vendor/pam.d"], |_| Ok(false));

		assert_eq!(service.unwrap(), Service::Login);
	}

	#[test]
	fn explicit_policy_selects_frostguard_in_directory_order() {
		let mut inspected = Vec::new();
		let service = select_service(&["/etc/pam.d", "/vendor/pam.d"], |path| {
			inspected.push(path.to_owned());
			Ok(true)
		});

		assert_eq!(service.unwrap(), Service::Frostguard);
		assert_eq!(inspected, [Path::new("/etc/pam.d/frostguard")]);
	}

	#[test]
	fn vendor_policy_selects_frostguard_when_system_policy_is_absent() {
		let service = select_service(&["/etc/pam.d", "/vendor/pam.d"], |path| {
			Ok(path.starts_with("/vendor"))
		});

		assert_eq!(service.unwrap(), Service::Frostguard);
	}

	#[test]
	fn metadata_errors_and_present_broken_policies_never_downgrade() {
		// NotFound from inspecting a present symlink's target is an error too.
		for kind in [
			io::ErrorKind::PermissionDenied,
			io::ErrorKind::Other,
			io::ErrorKind::NotFound,
			io::ErrorKind::InvalidData,
		] {
			let mut calls = 0;
			let result = select_service(&["/etc/pam.d", "/vendor/pam.d"], |_| {
				calls += 1;
				Err(io::Error::from(kind))
			});

			assert_eq!(result.unwrap_err().kind(), kind);
			assert_eq!(calls, 1);
		}
	}

	#[test]
	fn dedicated_rejection_never_retries_login() {
		let mut services = Vec::new();
		let result = Service::Frostguard.run(|service| {
			services.push(service);
			AuthResult::pam_failure(Stage::Authenticate, 7)
		});

		assert_eq!(result, AuthResult::Rejected);
		assert_eq!(services, [Service::Frostguard]);
	}

	#[test]
	fn only_success_accepts_and_failure_classification_retains_stage() {
		assert!(AuthResult::Accepted.accepted());
		for stage in [
			Stage::Start,
			Stage::Authenticate,
			Stage::Account,
			Stage::Identity,
			Stage::Conversation,
			Stage::End,
		] {
			let result = AuthResult::pam_failure(stage, SYSTEM_ERROR);

			assert_eq!(
				result,
				AuthResult::Failure {
					stage,
					status: SYSTEM_ERROR
				}
			);
			assert!(!result.accepted());
			assert!(!AuthResult::pam_failure(stage, 7).accepted());
		}
		assert!(!AuthResult::WorkerFailure.accepted());
		assert_ne!(
			AuthResult::WorkerFailure.status(),
			AuthResult::Rejected.status()
		);
		assert_ne!(
			AuthResult::pam_failure(Stage::Account, 7).status(),
			AuthResult::Rejected.status()
		);
	}

	#[test]
	fn conversation_rejects_invalid_counts_without_dereferencing_messages() {
		for count in [-1, 0, 33] {
			let mut output = ptr::null_mut();

			let result = unsafe { converse(count, ptr::null(), &mut output, ptr::null_mut()) };

			assert_eq!(result, CONVERSATION_ERROR);
			assert!(output.is_null());
		}
	}

	#[test]
	fn conversation_supplies_one_password_and_rejects_extra_secret_prompts() {
		let mut password = Secret::default();
		assert!(password.append("test password"));
		let mut data = Conversation {
			account: c"test-user",
			password: Some(password),
			supplied_password: false,
			prompts: tokio::sync::mpsc::unbounded_channel().0,
		};
		let message = PamMessage {
			style: 1,
			text: ptr::null(),
		};
		let message_pointer = ptr::from_ref(&message);
		let mut output = ptr::null_mut();

		let result = unsafe {
			converse(
				1,
				&message_pointer,
				&mut output,
				ptr::from_mut(&mut data).cast(),
			)
		};

		assert_eq!(result, SUCCESS);
		assert_eq!(
			unsafe { CStr::from_ptr((*output).text) }.to_bytes(),
			b"test password"
		);
		unsafe { free_responses(output, 1) };

		let result = unsafe {
			converse(
				1,
				&message_pointer,
				&mut output,
				ptr::from_mut(&mut data).cast(),
			)
		};

		assert_eq!(result, CONVERSATION_ERROR);
		assert!(output.is_null());
	}

	#[test]
	fn conversation_rejects_unsupported_prompt_styles() {
		let mut data = Conversation {
			account: c"test-user",
			password: Some(Secret::default()),
			supplied_password: false,
			prompts: tokio::sync::mpsc::unbounded_channel().0,
		};
		let message = PamMessage {
			style: 7,
			text: ptr::null(),
		};
		let message_pointer = ptr::from_ref(&message);
		let mut output = ptr::null_mut();

		let result = unsafe {
			converse(
				1,
				&message_pointer,
				&mut output,
				ptr::from_mut(&mut data).cast(),
			)
		};

		assert_eq!(result, CONVERSATION_ERROR);
		assert!(output.is_null());
	}

	#[test]
	fn probe_conversation_refuses_a_password_prompt_without_answering_it() {
		let mut data = Conversation {
			account: c"test-user",
			password: None,
			supplied_password: false,
			prompts: tokio::sync::mpsc::unbounded_channel().0,
		};
		let message = PamMessage {
			style: 1,
			text: ptr::null(),
		};
		let message_pointer = ptr::from_ref(&message);
		let mut output = ptr::null_mut();

		let result = unsafe {
			converse(
				1,
				&message_pointer,
				&mut output,
				ptr::from_mut(&mut data).cast(),
			)
		};

		assert_eq!(result, CONVERSATION_ERROR);
		assert!(output.is_null());
		assert!(!data.supplied_password);
	}

	#[test]
	fn probe_conversation_still_answers_the_harmless_echo_prompt() {
		// Style 2 never carries a credential (it's answered with the
		// already-known account name), so a probe answers it exactly like a
		// submitted attempt would.
		let mut data = Conversation {
			account: c"test-user",
			password: None,
			supplied_password: false,
			prompts: tokio::sync::mpsc::unbounded_channel().0,
		};
		let message = PamMessage {
			style: 2,
			text: ptr::null(),
		};
		let message_pointer = ptr::from_ref(&message);
		let mut output = ptr::null_mut();

		let result = unsafe {
			converse(
				1,
				&message_pointer,
				&mut output,
				ptr::from_mut(&mut data).cast(),
			)
		};

		assert_eq!(result, SUCCESS);
		assert_eq!(
			unsafe { CStr::from_ptr((*output).text) }.to_bytes(),
			b"test-user"
		);
		unsafe { free_responses(output, 1) };
	}

	#[test]
	fn conversation_forwards_info_and_error_text_without_answering_it() {
		let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
		let mut data = Conversation {
			account: c"test-user",
			password: Some(Secret::default()),
			supplied_password: false,
			prompts: sender,
		};
		let info = PamMessage {
			style: 4,
			text: c"Place your finger on the reader".as_ptr(),
		};
		let error = PamMessage {
			style: 3,
			text: c"Finger not recognized".as_ptr(),
		};
		let message_pointers = [ptr::from_ref(&info), ptr::from_ref(&error)];
		let mut output = ptr::null_mut();

		let result = unsafe {
			converse(
				2,
				message_pointers.as_ptr(),
				&mut output,
				ptr::from_mut(&mut data).cast(),
			)
		};

		assert_eq!(result, SUCCESS);
		// Neither message expects an answer: both response slots stay empty.
		assert!(unsafe { (*output).text }.is_null());
		assert!(unsafe { (*output.add(1)).text }.is_null());
		unsafe { free_responses(output, 2) };

		assert_eq!(
			receiver.try_recv().unwrap(),
			"Place your finger on the reader"
		);
		assert_eq!(receiver.try_recv().unwrap(), "Finger not recognized");
	}
}
