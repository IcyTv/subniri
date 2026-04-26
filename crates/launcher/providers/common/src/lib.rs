use std::sync::Arc;

use futures::stream::BoxStream;

mod runtime_handle;
pub use runtime_handle::RuntimeHandle;

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
	fn id(&self) -> ProviderId;

	fn name(&self) -> &'static str {
		self.id().0
	}

	async fn init(
		&self, ctx: Arc<dyn ProviderContext>, rt: RuntimeHandle,
	) -> anyhow::Result<async_channel::Receiver<ProviderEvent>>;

	async fn update_query(
		&self, session: SessionHandle, query: Query, ctx: Arc<dyn ProviderContext>, rt: RuntimeHandle,
	) -> anyhow::Result<()>;

	async fn activate(
		&self, session: SessionHandle, candidate_id: &CandidateId, activation: &ActivationKey, rt: RuntimeHandle,
	) -> anyhow::Result<Activation>;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct CandidateId(pub Arc<str>);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct SessionId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Revision(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProviderId(pub &'static str);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
	pub raw: Arc<str>,
	pub cursor: usize,
}

impl Query {
	pub fn new(raw: impl Into<Arc<str>>, cursor: usize) -> Self {
		Self {
			raw: raw.into(),
			cursor,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "glib", derive(glib::Enum))]
#[cfg_attr(feature = "glib", enum_type(name = "CandidateKind"))]
pub enum CandidateKind {
	App,
	Calc,
	Action,
	File,
	Window,
	Workspace,
	Command,
	#[default]
	Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "glib", derive(glib::Enum))]
#[cfg_attr(feature = "glib", enum_type(name = "MatchKind"))]
pub enum MatchKind {
	Exact,
	Prefix,
	Fuzzy,
	Substring,
	#[default]
	Unknown,
}

impl MatchKind {
	pub fn priority(&self) -> i32 {
		match self {
			Self::Exact => 0,
			Self::Prefix => 1,
			Self::Fuzzy => 2,
			Self::Substring => 3,
			Self::Unknown => 4,
		}
	}
}

/// Soft grouping hint; launcher may ignore.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "glib", derive(glib::Enum))]
#[cfg_attr(feature = "glib", enum_type(name = "SectionHint"))]
pub enum SectionHint {
	Apps,
	Calculations,
	Actions,
	Files,
	Windows,
	Other,
	#[default]
	None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconRef {
	IconName(Arc<str>),
	ThemedName(Arc<str>),
	AbsolutePath(Arc<str>),
}

/// Provider-owned opaque payload key used for activation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActivationKey(pub Arc<str>);

#[derive(Clone, Debug)]
pub struct Candidate {
	pub provider: ProviderId,
	pub id: CandidateId,
	pub activation: ActivationKey,

	pub title: Arc<str>,
	pub subtitle: Option<Arc<str>>,
	pub right_text: Option<Arc<str>>,
	pub icon: Option<IconRef>,

	pub kind: CandidateKind,
	pub section_hint: Option<SectionHint>,
	pub match_kind: MatchKind,

	pub provider_score: f32,
}

#[derive(Clone, Debug)]
pub enum PreviewModel {
	Text {
		title: Arc<str>,
		body: Arc<str>,
	},
	Lines {
		title: Option<Arc<str>>,
		lines: Arc<[Arc<str>]>,
	},
}

#[derive(Clone, Debug)]
pub enum ProviderStatus {
	Loading,
	Ready,
	Error(Arc<str>),
}

#[derive(Clone, Debug)]
pub enum ProviderEvent {
	CandidateUpsert(Candidate),
	CandidateRemove { id: CandidateId },
	PreviewUpdate(PreviewModel),
	Status(ProviderStatus),
	// Removes all existing candidates from the list...
	Reset,
	Done,
}

// TODO: Allow for combinations?
#[derive(Clone, Debug)]
pub enum Activation {
	Noop,
	CloseLauncher,
	HideLauncher,
	KeepOpen,
	SetInput(String),
	SetResponse(String),
}

#[async_trait::async_trait]
pub trait ProviderContext: Send + Sync {
	async fn hide(&self);
	async fn close(&self);
	async fn set_input(&self, input: String);
	async fn set_preview(&self, preview: PreviewModel);
	async fn set_response(&self, response: String);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionHandle {
	pub session_id: SessionId,
	pub revision: Revision,
}
